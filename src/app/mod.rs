use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

use anyhow::Result;
use base64::Engine as _;
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::{
    backend::{Backend, StreamEvent},
    config::{self, AuthKind, Connection, ProviderKind, SavedModel},
    diff::FileDiff,
    session::{self, ImageAttachment, Message, Role, Session, TaskState, ToolCall},
    skills, tools,
};

mod action;
mod commands;
mod connect;
mod delegate;
mod format;
mod generate;
mod images;
mod input;
mod keys;
mod mentions;
mod mode;
mod palette;
mod pickers;
mod settings;
mod stats;
mod stream;
mod tabs;
mod talk;
mod turn;

#[cfg(test)]
mod orchestration_tests;

pub use action::TextSelection;
pub(crate) use action::{Action, ChangeReport, Notice, NoticeKind, RetryNotice, ToolEvent};
pub(crate) use commands::{
    COMMANDS, EFFORT_CHOICES, SET_KEYS, Suggestion, argument_suggestions, matching_commands,
};
pub(crate) use delegate::{
    DelegateUpdate, SUBAGENT_BRIEF, SUBAGENT_NAME_BRIEF, Subagent, SubagentStatus, WRITING_TOOLS,
    ZERO_WRITE_NUDGE_ROUNDS,
};
pub(crate) use input::Input;
pub(crate) use pickers::{ListPicker, ModelChoice, ModelRow, SessionEntry};
pub(crate) use settings::SETTING_ROWS;

#[cfg(test)]
pub(crate) use turn::tests::test_app;

use format::estimated_output_tokens;

pub(crate) use connect::state::{
    AddProviderField, ConnectStage, ConnectState, ListRow, list_row_of,
};
pub use format::{
    format_bytes, format_context, format_cost, format_duration_ms, format_rtt, format_tokens,
};
pub use mode::{Mode, ToolsMode};

/// The screenshot a tool result points at, for the model to see.
///
/// `read_image` names its file in the arguments and `computer_use` in its
/// result text; both resolve against the scoped workspace, so the caller sets
/// that scope first.
pub(crate) fn tool_result_attachment(call: &ToolCall, content: &str) -> Option<ImageAttachment> {
    images::tool_image_attachment(call, content)
}

use tabs::TabState;

/// Fraction of the model context window where the compaction chance begins.
const COMPACT_CHANCE_FRAC: u64 = 5; // chance starts at ~4/5 of window
/// Fraction of the model context window kept intact after compaction.
const COMPACTION_KEEP_FRAC: u64 = 8; // keep ~1/8 of window
/// Default context window (256K) when no specific model context size is known.
pub(crate) const DEFAULT_CONTEXT_WINDOW: u64 = 256_000;
/// How many compaction requests one turn may run before the full history is
/// sent regardless. Without a cap a provider that answers empty, or a summary
/// that never shrinks the load, loops the turn forever.
const MAX_COMPACTION_ATTEMPTS: u32 = 2;
/// How many times a stalled stream automatically resumes before the stall is
/// reported. Each resume re-sends the request; without a cap a provider that
/// stalls every time would loop the turn forever.
const MAX_STREAM_RESUMES: u32 = 3;
/// How many times a reply that hit the output token cap continues itself
/// before the cap is reported. Each continuation is a fresh request, so the
/// cap keeps a provider that truncates every reply from looping the turn.
const MAX_AUTO_CONTINUES: u32 = 3;

struct ActiveRequest {
    id: u64,
    started: Instant,
    /// Send to first token, stamped once per request. `None` while the
    /// provider has said nothing yet.
    ttft_ms: Option<u64>,
    prompt_tokens: u64,
    usage: Option<(u64, u64)>,
    tool_progress: Option<(String, String, u64)>,
    /// Set once this request has cleared the failure it recovered from, so the
    /// transcript is swept once per reply rather than once per delta.
    recovered: bool,
    /// Set while this request summarizes older turns instead of replying.
    compacting: Option<Compaction>,
}

/// A compaction in flight: which history prefix it summarizes, the text
/// streamed back so far, and how many rounds already ran this turn.
pub(crate) struct Compaction {
    split: usize,
    text: String,
    attempts: u32,
}

pub struct ToolApproval {
    pub call: ToolCall,
    pub note: Option<String>,
    /// Change previews of a write or edit, drawn in the dialog itself.
    pub diffs: Vec<FileDiff>,
    pub scroll: usize,
    pub page_height: u16,
    pub popup_area: std::cell::Cell<ratatui::layout::Rect>,
}

/// One pasted image waiting in the composer: its real path (file pastes
/// only; clipboard images have none) plus the queued attachment that vision
/// models receive. The composer itself only ever shows the placeholder.
#[derive(Clone, Debug)]
pub struct PastedImage {
    pub path: Option<String>,
    pub attachment: String,
}

pub struct RunningTool {
    pub call: ToolCall,
    pub started: Instant,
    /// File contents from before the call, kept for the outcome diff.
    pub before: Option<String>,
    /// Whether the write or edit target existed before the call ran.
    pub existed: bool,
    /// Every file the call may change and whether each existed before.
    pub targets: Vec<(String, bool)>,
}

/// Session snapshots written off the UI thread.
///
/// Encoding and writing a large history can take seconds, which used to block
/// the event loop on every send and looked like a freeze. Snapshots are queued
/// here instead; only the newest queued one is written, because everything
/// older is already stale.
struct SessionSaver {
    jobs: std::sync::mpsc::Sender<SaveJob>,
    errors: std::sync::mpsc::Receiver<String>,
}

struct SaveJob {
    path: PathBuf,
    session: Session,
    done: Option<std::sync::mpsc::Sender<()>>,
}

impl SessionSaver {
    fn new() -> Self {
        let (jobs, queue) = std::sync::mpsc::channel::<SaveJob>();
        let (error_tx, errors) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            while let Ok(mut job) = queue.recv() {
                let mut superseded = Vec::new();
                while let Ok(newer) = queue.try_recv() {
                    if let Some(done) = job.done.take() {
                        superseded.push(done);
                    }
                    job = newer;
                }
                if let Err(error) = job.session.save(&job.path) {
                    let _ = error_tx.send(format!("{error:#}"));
                }
                for done in superseded {
                    let _ = done.send(());
                }
                if let Some(done) = job.done {
                    let _ = done.send(());
                }
            }
        });
        Self { jobs, errors }
    }

    fn save(&self, path: PathBuf, session: Session) {
        let _ = self.jobs.send(SaveJob {
            path,
            session,
            done: None,
        });
    }

    /// Save and wait for the write, for the final snapshot on exit.
    fn save_and_wait(&self, path: PathBuf, session: Session) -> bool {
        let (done_tx, done) = std::sync::mpsc::channel();
        let job = SaveJob {
            path,
            session,
            done: Some(done_tx),
        };
        if self.jobs.send(job).is_err() {
            return false;
        }
        done.recv_timeout(std::time::Duration::from_secs(10))
            .is_ok()
    }

    /// The last write failure since the previous call, if any.
    fn take_error(&self) -> Option<String> {
        let mut last = None;
        while let Ok(error) = self.errors.try_recv() {
            last = Some(error);
        }
        last
    }
}

/// One rendered markdown block for a finished message: the width it was
/// rendered at, the content hash it was rendered from, the shared lines, their
/// total wrapped rows, and the content length it was rendered from.
/// What a framed tool result depends on. A hit needs every field equal, so a
/// change to the output, the expansion, the diff view or the width rebuilds it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ToolFrameKey {
    pub width: u16,
    pub content_len: usize,
    pub content_hash: u64,
    pub name_hash: u64,
    pub expanded: bool,
    pub has_report: bool,
    pub report_files: usize,
    pub image_rows: usize,
    pub diff_sides: bool,
    /// Whether this result closes its group: the last one hangs on `╰`, an
    /// earlier one keeps the rail running toward the result below it.
    pub last: bool,
}

/// A framed tool result, shared by every frame it is drawn in. `clicks` are
/// the row offsets (from the frame's first line) that toggle expansion.
pub(crate) struct ToolFrame {
    pub key: ToolFrameKey,
    pub lines: std::sync::Arc<[ratatui::text::Line<'static>]>,
    pub rows: usize,
    pub clicks: Vec<usize>,
}

pub(crate) type MarkdownBlock = (
    u16,
    u64,
    std::sync::Arc<[ratatui::text::Line<'static>]>,
    usize,
    usize,
);

/// The post-update changelog popup: one version grouped into Added, Fixed,
/// Removed and Changed, scrolled with the arrow keys and closed with Esc or
/// Enter.
pub struct ChangeLogView {
    pub title: String,
    pub section: crate::changelog::Section,
    pub scroll: usize,
    /// `c`: leave the long Changed tail out of the popup.
    pub hide_changed: bool,
    /// `x`: show every bullet instead of the first few of each group.
    pub expanded: bool,
}

/// `/stats`: what every run on this machine has spent, plus a 13-week heatmap.
/// The ledger is read once when the view opens (and refreshed at most every
/// few seconds), never per frame.
pub struct StatsView {
    pub ledger: crate::usage::Ledger,
    pub scroll: usize,
    /// When the ledger was last read, so a long-open view still follows a turn
    /// that finishes behind it.
    pub loaded: std::time::Instant,
}

impl StatsView {
    /// Open the view on a fresh read of the ledger.
    pub(crate) fn open() -> Self {
        Self {
            ledger: crate::usage::load(),
            scroll: 0,
            loaded: std::time::Instant::now(),
        }
    }

    /// Re-read the ledger when the copy on screen has gone stale.
    pub(crate) fn refresh(&mut self) {
        if self.loaded.elapsed() >= crate::usage::REFRESH {
            self.ledger = crate::usage::load();
            self.loaded = std::time::Instant::now();
        }
    }
}

impl ChangeLogView {
    /// Open the popup for the running build, or nothing when the changelog
    /// has no section to show.
    pub(crate) fn for_current_version() -> Option<Self> {
        let version = crate::update::current_version();
        let section = crate::changelog::section_for(version)?;
        Some(Self {
            title: format!("What's new in v{version}"),
            section,
            scroll: 0,
            hide_changed: false,
            expanded: false,
        })
    }
}

pub struct App {
    pub session: Session,
    pub backend: Backend,
    /// Session snapshots are written here, never on the UI thread.
    session_saver: SessionSaver,
    pub input: Input,
    pub scroll: usize,
    pub input_scroll: usize,
    pub transcript_height: u16,
    pub follow_tail: bool,
    pub show_help: bool,
    /// The post-update changelog popup, shown once per new version.
    pub changelog: Option<ChangeLogView>,
    /// The `/stats` view: token spend across every run, with a heatmap.
    pub stats: Option<StatsView>,
    /// How far the help popup is scrolled down.
    pub help_scroll: usize,
    pub should_quit: bool,
    pub notice: Option<Notice>,
    /// A request being retried, shown in the footer while it waits.
    pub retry: Option<RetryNotice>,
    /// Automatic resumes of a stalled stream this turn chain has used.
    resume_attempts: u32,
    /// Automatic continuations of a reply that hit the output token cap this
    /// turn chain has used.
    continue_attempts: u32,
    /// Whether the terminal has focus. A background terminal draws nothing.
    pub focused: bool,
    pub spinner: usize,
    /// Submitted turn body: the composer prompt flashes for a few ticks.
    pub send_pulse: Option<usize>,
    /// Keystroke that armed the composer caret animation, if it is still live.
    pub typing_pulse: Option<usize>,
    pub tools_mode: ToolsMode,
    /// What the current turn is for: applying changes, planning, or chat.
    pub mode: Mode,
    /// Whether history is summarized automatically when the context fills.
    pub auto_compact: bool,
    /// Discord application id for Rich Presence: the shared application by
    /// default, the user's own when `/set discord <id>` set one, and `None`
    /// once `/set discord off` turned the feature off.
    pub discord_client_id: Option<String>,
    /// Images pasted with Ctrl+V, sent with the next submitted message.
    pub attachments: Vec<ImageAttachment>,
    /// Counter giving each pasted image a unique display name.
    next_image: usize,
    /// Pasted images waiting in the composer: `[Image N]` placeholder ->
    /// real path and attachment. The composer never shows the path.
    image_paths: HashMap<String, PastedImage>,
    /// Pre-rendered half-block rows of pasted images, keyed `name:width`.
    image_rows: HashMap<String, Vec<Line<'static>>>,
    /// Terminal width the cached image rows were rendered for.
    image_width: u16,
    pub approval: Option<ToolApproval>,
    /// Whether the `/settings` popup is open.
    pub settings_open: bool,
    /// Row of the `/settings` popup the arrow keys act on.
    pub settings_index: usize,
    /// Popup picker opened by `/provider models` with no argument.
    pub model_picker: Option<ListPicker>,
    /// The models offered by every connected provider, rebuilt when the
    /// picker opens.
    pub model_choices: Vec<ModelChoice>,
    /// Whether models are actively being fetched in the background.
    pub fetching_models: bool,
    /// An image request is in flight: the footer says so instead of looking
    /// idle for the minute an image model takes.
    pub generating_image: bool,
    /// Popup picker opened by `/session`.
    pub session_picker: Option<ListPicker>,
    /// Loaded rows for the `/session` popup, populated one JSON file per tick
    /// so opening `/session` never blocks the TUI.
    session_entries: Vec<SessionEntry>,
    /// Remaining session JSON paths to parse, newest first.
    session_load_queue: VecDeque<(PathBuf, Option<std::time::SystemTime>)>,
    /// Total session files queued when the `/session` picker opened.
    session_load_total: usize,
    /// Popup picker opened by `/revert`, listing the prompts already sent.
    pub revert_picker: Option<ListPicker>,
    /// Popup picker opened by `/timeline`, jumping to a previous prompt.
    pub timeline_picker: Option<ListPicker>,
    /// Message index to scroll to on the next transcript render.
    pub pending_scroll_to_message: Option<usize>,
    pub running_tool: Option<RunningTool>,
    /// Whether a user command (from `!` or `/run`) is running in the
    /// background: a second one waits, and the Esc interrupt stops it.
    pub user_command_running: bool,
    /// Subagent runs in flight: several delegate calls stream in parallel
    /// while the queue keeps advancing. Tracked per session like
    /// `running_tool`, so results land in the tab that asked.
    pub running_delegates: Vec<RunningTool>,
    /// Finished subagent runs waiting to report: a busy tab parks the
    /// answers, and they deliver at the next idle point so a report never
    /// lands mid-stream and the model is alerted with a fresh turn.
    pub pending_reports: VecDeque<(u64, String)>,
    /// Talk messages from other tabs waiting to land: parked while this tab
    /// is busy, delivered at the next idle point like the subagent reports.
    pub pending_talks: VecDeque<String>,
    /// Runs the user cancelled here: their reports land silently, because a
    /// cancellation must not be answered by a turn that claims success.
    pub cancelling_runs: Vec<u64>,
    /// Diffs for write and edit calls, shown under their results.
    pub changes: HashMap<String, ChangeReport>,
    /// Tool results the user expanded to see the full output.
    pub expanded: HashSet<String>,
    /// Screen row of each collapsible tool output, for click detection.
    pub output_rows: Vec<(u16, String)>,
    /// Whether the Progress checklist is folded to its title row.
    pub progress_collapsed: bool,
    /// The Progress window's top task when the user scrolled it: `None`
    /// follows the first unfinished task.
    pub progress_scroll: Option<usize>,
    /// The window start the last draw used, so a wheel step begins there.
    pub progress_start: usize,
    /// The Progress panel's on-screen area, for wheel hit-testing.
    pub progress_area: Option<(u16, u16, u16, u16)>,
    /// Screen row of the Progress title, for click-to-fold.
    pub progress_row: Option<u16>,
    /// Composer target area for mouse click and drag selection.
    pub composer_area: Option<ratatui::layout::Rect>,
    /// The transcript's on-screen area, refilled each draw: a click or drag
    /// here selects conversation text instead of toggling a tool result.
    pub transcript_area: Option<ratatui::layout::Rect>,
    /// The transcript selection: the first and last cell of the drag, in
    /// transcript rows and screen columns. Text between them is highlighted,
    /// and releasing the button copies it.
    pub text_selection: Option<TextSelection>,
    /// The text the last drawn selection covered, rebuilt every frame from
    /// the cells actually on screen. A mouse release copies this, so what
    /// lands on the clipboard is what the user saw highlighted.
    pub selection_text: Option<String>,
    /// Unified or side-by-side diff view, kept across approvals and results.
    pub diff_sides: bool,
    active: Option<ActiveRequest>,
    next_request_id: u64,
    pending_tools: VecDeque<ToolCall>,
    pub(crate) approved_tools: HashSet<String>,
    session_path: Option<PathBuf>,
    /// Directory holding every saved session file, when saving.
    sessions_dir: Option<PathBuf>,
    config_path: PathBuf,
    custom_providers: Vec<config::CustomProvider>,
    /// Cache of rendered markdown blocks for completed messages, with their
    /// wrap row total: see `MarkdownBlock`.
    pub(crate) markdown_cache: HashMap<usize, MarkdownBlock>,
    /// Framed tool results per message index: see `ToolFrame`.
    pub(crate) tool_frames: HashMap<usize, ToolFrame>,
    /// A message typed while subagents still stream, queued for the
    /// post-delegate continuation (see `TabState::queued_message`).
    pub queued_message: Option<String>,
    pub queued_images: Vec<ImageAttachment>,
    /// Tavily key for the web_search tool.
    tavily_api_key: Option<String>,
    /// Subagent runs from delegate calls, streamed live and viewable with
    /// `/agents`. App-global, so a run stays viewable from every tab.
    pub(crate) subagents: Vec<Subagent>,
    /// Counter giving each subagent run a unique id.
    next_subagent_id: u64,
    /// Popup picker opened by `/agents`.
    pub(crate) agents_picker: Option<ListPicker>,
    /// The subagent run the viewer shows, as an index into `subagents`.
    pub(crate) subagent_view: Option<usize>,
    /// How far the run viewer is scrolled down.
    pub(crate) subagent_scroll: usize,
    /// Click hit-boxes for the sidebar's run rows, refilled each draw:
    /// (x_start, x_end, row, index into `subagents`).
    pub(crate) sidebar_hits: Vec<(u16, u16, u16, usize)>,
    /// How far the sidebar's run list is scrolled up from the newest rows:
    /// zero keeps the tail in view, scrolling reveals the runs above it.
    pub(crate) sidebar_scroll: usize,
    /// The sidebar's on-screen area as (x, y, width, height), refilled each
    /// draw: the wheel scroll hit-tests against it.
    pub(crate) sidebar_area: Option<(u16, u16, u16, u16)>,
    palette_index: usize,
    palette_hidden: bool,
    connect: Option<ConnectState>,
    /// Parked sessions: the active tab's state lives in the fields above,
    /// every other tab is a `TabState` snapshot here. `tabs[tab]` is a blank
    /// placeholder for the active one.
    tabs: Vec<TabState>,
    /// Index of the active tab.
    tab: usize,
    /// Click hit-boxes for the tab bar, refilled each draw:
    /// (x_start, x_end, row, index). The row keeps a click on a lower panel
    /// that shares a tab's column range from switching tabs.
    pub(crate) tab_hits: Vec<(u16, u16, u16, usize)>,
    /// Click hit-boxes for each tab's `×`: (x_start, x_end, row, index).
    pub(crate) tab_close_hits: Vec<(u16, u16, u16, usize)>,
    /// Click hit-box for the `+` new-tab button: (x_start, x_end, row).
    pub(crate) tab_new_hit: Option<(u16, u16, u16)>,
    /// Prompt tokens billed across this session.
    spent_prompt_tokens: u64,
    /// Completion tokens billed across this session.
    spent_completion_tokens: u64,
    /// Cached context-token estimate: (measured at, tokens). The header reads
    /// this every frame, and counting every transcript character per keystroke
    /// is part of what made typing lag while a long reply streamed.
    context_cache: (Option<Instant>, u64),
    pub(crate) prompt_started: Option<Instant>,
    /// This tab's own working directory: relative tool paths resolve against
    /// it. One tab's `/cd` or `move_to` never reaches another conversation,
    /// and the process working directory never changes.
    working_dir: PathBuf,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session: Session,
        backend: Backend,
        session_path: Option<PathBuf>,
        sessions_dir: Option<PathBuf>,
        config_path: PathBuf,
        custom_providers: Vec<config::CustomProvider>,
        tavily_api_key: Option<String>,
        auto_compact: bool,
    ) -> Self {
        Self {
            session,
            backend,
            session_saver: SessionSaver::new(),
            input: Input::default(),
            scroll: 0,
            input_scroll: 0,
            transcript_height: 10,
            follow_tail: true,
            show_help: false,
            changelog: None,
            help_scroll: 0,
            should_quit: false,
            notice: None,
            retry: None,
            resume_attempts: 0,
            continue_attempts: 0,
            focused: true,
            spinner: 0,
            send_pulse: None,
            typing_pulse: None,
            tools_mode: ToolsMode::default(),
            mode: Mode::default(),
            auto_compact,
            discord_client_id: config::load_discord_client_id(&config_path),
            attachments: Vec::new(),
            next_image: 0,
            image_paths: HashMap::new(),
            image_rows: HashMap::new(),
            image_width: 0,
            approval: None,
            settings_open: false,
            stats: None,
            settings_index: 0,
            model_picker: None,
            model_choices: Vec::new(),
            fetching_models: false,
            generating_image: false,
            session_picker: None,
            session_entries: Vec::new(),
            session_load_queue: VecDeque::new(),
            session_load_total: 0,
            revert_picker: None,
            timeline_picker: None,
            pending_scroll_to_message: None,
            running_tool: None,
            user_command_running: false,
            running_delegates: Vec::new(),
            pending_reports: VecDeque::new(),
            pending_talks: VecDeque::new(),
            cancelling_runs: Vec::new(),
            changes: HashMap::new(),
            expanded: HashSet::new(),
            output_rows: Vec::new(),
            progress_collapsed: false,
            progress_scroll: None,
            progress_start: 0,
            progress_area: None,
            progress_row: None,
            composer_area: None,
            transcript_area: None,
            text_selection: None,
            selection_text: None,
            diff_sides: true,
            context_cache: (None, 0),
            active: None,
            next_request_id: 0,
            pending_tools: VecDeque::new(),
            approved_tools: HashSet::new(),
            session_path,
            sessions_dir,
            config_path: config_path.clone(),
            custom_providers,
            markdown_cache: HashMap::new(),
            tool_frames: HashMap::new(),
            queued_message: None,
            queued_images: Vec::new(),
            tavily_api_key,
            subagents: Vec::new(),
            next_subagent_id: 0,
            agents_picker: None,
            subagent_view: None,
            subagent_scroll: 0,
            sidebar_hits: Vec::new(),
            sidebar_scroll: 0,
            sidebar_area: None,
            palette_index: 0,
            palette_hidden: false,
            connect: None,
            tabs: vec![TabState::default()],
            tab: 0,
            tab_hits: Vec::new(),
            tab_close_hits: Vec::new(),
            tab_new_hit: None,
            spent_prompt_tokens: 0,
            spent_completion_tokens: 0,
            prompt_started: None,
            working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }

    /// The tab's own working directory: where its relative paths resolve.
    pub(crate) fn working_dir(&self) -> &std::path::Path {
        &self.working_dir
    }

    pub fn connect_state(&self) -> Option<&ConnectState> {
        self.connect.as_ref()
    }

    /// Mutable access to the open dialog, for the test scenes that render it
    /// at a chosen stage.
    #[cfg(test)]
    pub(crate) fn connect_state_mut(&mut self) -> Option<&mut ConnectState> {
        self.connect.as_mut()
    }

    /// Whether any subagent run is still streaming, so the event loop keeps
    /// ticking and the viewer stays live while a delegate runs in the
    /// background.
    pub(crate) fn any_subagent_running(&self) -> bool {
        self.subagents
            .iter()
            .any(|agent| agent.status() == SubagentStatus::Running)
    }

    pub fn tavily_key(&self) -> Option<&str> {
        self.tavily_api_key.as_deref()
    }

    pub fn connect_open(&self) -> bool {
        self.connect.is_some()
    }

    pub fn is_streaming(&self) -> bool {
        self.active.is_some()
    }

    pub fn tools_busy(&self) -> bool {
        self.approval.is_some()
            || self.running_tool.is_some()
            || !self.running_delegates.is_empty()
            || !self.pending_tools.is_empty()
            || self.user_command_running
    }

    pub fn is_busy(&self) -> bool {
        self.is_streaming() || self.tools_busy()
    }

    /// Path of the session currently open, for the `/session` popup marker.
    pub fn active_session_path(&self) -> Option<&PathBuf> {
        self.session_path.as_ref()
    }

    pub fn running_tool_elapsed(&self) -> Option<f64> {
        self.running_tool
            .as_ref()
            .map(|running| running.started.elapsed().as_secs_f64())
    }

    pub fn active_request_id(&self) -> Option<u64> {
        self.active.as_ref().map(|active| active.id)
    }

    pub fn elapsed_seconds(&self) -> Option<f64> {
        self.active
            .as_ref()
            .map(|active| active.started.elapsed().as_secs_f64())
    }

    /// True while the active request is summarizing older turns.
    pub fn compaction_active(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.compacting.is_some())
    }

    /// The summary streamed so far, shown live in the transcript.
    pub fn compaction_text(&self) -> Option<&str> {
        self.active
            .as_ref()?
            .compacting
            .as_ref()
            .map(|compaction| compaction.text.as_str())
    }

    pub fn stream_tool_progress(&self) -> Option<(String, String, u64)> {
        self.active
            .as_ref()
            .and_then(|active| active.tool_progress.clone())
    }

    /// (in, out) tokens for the active request: server-reported usage, else chars/4 estimates.
    pub fn stream_tokens(&self) -> Option<(u64, u64)> {
        let active = self.active.as_ref()?;
        match active.usage {
            Some((prompt, completion)) => Some((prompt.max(active.prompt_tokens), completion)),
            None => Some((active.prompt_tokens, estimated_output_tokens(&self.session))),
        }
    }

    /// Session spend so far, priced from the active model. A provider catalog
    /// price wins over the built-in list; a model neither of them prices
    /// returns an empty string, so the header can drop the segment.
    pub fn spent_cost(&self) -> String {
        let price = self
            .backend
            .model_price()
            .or_else(|| format::known_model_price(self.backend.model()));
        format_cost(
            self.spent_prompt_tokens,
            self.spent_completion_tokens,
            price,
        )
    }

    /// (in, out) tokens billed this session, per the setter's own usage
    /// reports. `None` until a provider reports any, so an estimate never
    /// poses as a bill.
    pub fn spent_tokens(&self) -> Option<(u64, u64)> {
        (self.spent_prompt_tokens + self.spent_completion_tokens > 0)
            .then_some((self.spent_prompt_tokens, self.spent_completion_tokens))
    }

    /// (in, out) tokens for the footer arrows: what is billed so far plus the
    /// request in flight, so the numbers move while the reply streams instead
    /// of jumping once the turn ends. Falls back to the billed totals when no
    /// request is running.
    pub fn display_tokens(&self) -> Option<(u64, u64)> {
        let billed = (self.spent_prompt_tokens, self.spent_completion_tokens);
        match self.stream_tokens() {
            Some((prompt, completion)) => Some((
                billed.0.saturating_add(prompt),
                billed.1.saturating_add(completion),
            )),
            None => self.spent_tokens(),
        }
    }

    /// Estimated tokens the next request would send, the context window load.
    ///
    /// Cached for a quarter second: the header draws this every frame, and
    /// recounting the whole transcript per keystroke is lag nobody needs.
    pub fn context_tokens(&mut self) -> u64 {
        if self
            .context_cache
            .0
            .is_some_and(|at| at.elapsed() < Duration::from_millis(250))
        {
            return self.context_cache.1;
        }
        let tokens = Session::estimate_tokens(&self.session.messages);
        self.context_cache = (Some(Instant::now()), tokens);
        tokens
    }

    /// Context-window size of the active model from its saved metadata, built-in
    /// model table, or cached catalog, if known.
    pub fn model_context_window(&self) -> Option<u64> {
        let model_id = self.backend.model();
        // The backend's active model list carries the limit saved at connect
        // time, so this survives self.connect being cleared, and it is scoped
        // to the active provider: model ids are not globally unique.
        if let Some(window) = self
            .backend
            .saved_models()
            .iter()
            .find(|saved| {
                saved.id.eq_ignore_ascii_case(model_id)
                    || (!saved.name.is_empty() && saved.name.eq_ignore_ascii_case(model_id))
            })
            .and_then(|saved| saved.context_window)
        {
            return Some(window);
        }

        // Built-in model table and family heuristics
        if let Some(window) = crate::models::context_window(model_id) {
            return Some(window);
        }

        None
    }

    pub fn custom_providers(&self) -> &[config::CustomProvider] {
        &self.custom_providers
    }

    pub fn tick(&mut self) {
        self.spinner = self.spinner.wrapping_add(1);
        self.step_session_load();
        // A notice is transient: it dismisses itself after a few seconds, so
        // an old error stops sitting over output that arrived after it. The
        // ticker runs while a notice is up (see main's loop condition), which
        // is what fires this check and redraws.
        const NOTICE_LIFETIME: std::time::Duration = std::time::Duration::from_secs(6);
        if self
            .notice
            .as_ref()
            .is_some_and(|notice| notice.started.elapsed() >= NOTICE_LIFETIME)
        {
            self.notice = None;
        }
    }

    /// Ticks the composer caret keeps animating after a keystroke.
    pub(crate) const TYPING_TICKS: usize = 8;

    /// How many ticks ago the last keystroke armed the caret animation, while
    /// that animation still has frames left to draw.
    pub(crate) fn typing_tick(&self) -> Option<usize> {
        let elapsed = self.spinner.wrapping_sub(self.typing_pulse?);
        (elapsed < Self::TYPING_TICKS).then_some(elapsed)
    }

    /// Whether the 100ms ticker should redraw this frame: only when something
    /// visible animates or a timer has to run. A queued tool or a background
    /// `/run` command draws nothing animated, so they no longer keep the
    /// ticker alive; an approval dialog pulses its border and a running tool
    /// ticks its footer spinner, so those still do.
    pub(crate) fn ticker_wanted(&self) -> bool {
        self.any_streaming()
            || self.connect_open()
            // An open list animates its selection: the glow and the sliding
            // band need frames, or the selection sits still.
            || self.settings_open
            || self.model_picker.is_some()
            || self.session_picker.is_some()
            || self.revert_picker.is_some()
            || self.timeline_picker.is_some()
            || self.agents_picker.is_some()
            || (!self.palette_hidden && !self.palette_matches().is_empty())
            || self.approval.is_some()
            || self.running_tool.is_some()
            || self.any_subagent_running()
            // A notice dismisses itself after a few seconds: the ticker runs
            // that expiry.
            || self.notice.is_some()
            // A retry counts down its delay in the footer.
            || self.retry.is_some()
            // Servers connect in the background: the landing page follows each
            // one from `starting` to its tool count.
            || crate::mcp::pending()
            // The submitted-prompt flash animates for a few ticks on its own.
            || self
                .send_pulse
                .is_some_and(|mark| self.spinner.wrapping_sub(mark) < 5)
            // A keystroke throbs the composer caret for a moment.
            || self.typing_tick().is_some()
            // The model fetch spinner animates while it waits.
            || self.fetching_models
            || self.generating_image
            // `auto` repaints on the minute so a quiet session still moves
            // from morning to night.
            || crate::theme::auto_redraw_due()
            // Rain and snow fall: the ticker slides them every frame.
            || crate::weather::current().wet()
            // A forecast request is in flight: the frame that lands with the
            // answer still has to be drawn, without waiting for the minute.
            || crate::weather::fetching()
    }

    /// Record whether the terminal has focus.
    ///
    /// A background terminal shows nothing, so the ticker stops and the
    /// spinner freezes instead of burning power on frames nobody sees. The
    /// retry notice stays: the retry runs in the background, and dropping its
    /// footer text makes a waiting request look like a silent hang.
    pub fn set_focused(&mut self, focused: bool) {
        // Coming back to the terminal is when the sky on screen is most likely
        // stale: ask the forecast again instead of waiting out its poll.
        if focused && !self.focused {
            crate::weather::refresh();
        }
        self.focused = focused;
    }
}
