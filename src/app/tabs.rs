//! Session tabs: several conversations live side by side, each with its own
//! streaming request, tool queue, and history.
//!
//! The active tab's state IS the `App`: its fields are the session on screen.
//! Background tabs are parked here as `TabState` snapshots; switching tabs
//! stashes the active fields and restores the target's, which is a move of
//! a handful of owned values per switch.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::text::Line;

use anyhow::Result;

use super::{ActiveRequest, App, ChangeReport, Input, Mode, RunningTool, ToolApproval, ToolsMode};
use crate::session::{ImageAttachment, Session, ToolCall};

/// Upper bound: one tab per Alt+1..9 jump.
const MAX_TABS: usize = 9;

/// Everything that belongs to one conversation, stashed while its tab is in
/// the background.
pub(crate) struct TabState {
    pub(crate) session: Session,
    pub(crate) input: Input,
    pub(crate) scroll: usize,
    pub(crate) input_scroll: usize,
    pub(crate) transcript_height: u16,
    pub(crate) follow_tail: bool,
    pub(crate) send_pulse: Option<usize>,
    pub(crate) tools_mode: ToolsMode,
    pub(crate) mode: Mode,
    pub(crate) auto_compact: bool,
    pub(crate) attachments: Vec<ImageAttachment>,
    next_image: usize,
    image_paths: HashMap<String, super::PastedImage>,
    image_rows: HashMap<String, Vec<Line<'static>>>,
    image_width: u16,
    pub(crate) approval: Option<ToolApproval>,
    pub(crate) running_tool: Option<RunningTool>,
    pub(crate) changes: HashMap<String, ChangeReport>,
    pub(crate) expanded: HashSet<String>,
    pub(crate) output_rows: Vec<(u16, String)>,
    pub(crate) progress_collapsed: bool,
    pub(crate) progress_row: Option<u16>,
    pub(crate) composer_area: Option<Rect>,
    pub(crate) diff_sides: bool,
    pub(crate) active: Option<ActiveRequest>,
    pub(crate) pending_tools: VecDeque<ToolCall>,
    pub(crate) approved_tools: HashSet<String>,
    pub(crate) session_path: Option<PathBuf>,
    spent_prompt_tokens: u64,
    spent_completion_tokens: u64,
    pub(crate) prompt_started: Option<Instant>,
    markdown_cache: HashMap<usize, (u16, usize, Vec<Line<'static>>)>,
}

impl Default for TabState {
    fn default() -> Self {
        Self {
            session: Session::new(),
            input: Input::default(),
            scroll: 0,
            input_scroll: 0,
            transcript_height: 10,
            follow_tail: true,
            send_pulse: None,
            tools_mode: ToolsMode::default(),
            mode: Mode::default(),
            auto_compact: true,
            attachments: Vec::new(),
            next_image: 0,
            image_paths: HashMap::new(),
            image_rows: HashMap::new(),
            image_width: 0,
            approval: None,
            running_tool: None,
            changes: HashMap::new(),
            expanded: HashSet::new(),
            output_rows: Vec::new(),
            progress_collapsed: false,
            progress_row: None,
            composer_area: None,
            diff_sides: true,
            active: None,
            pending_tools: VecDeque::new(),
            approved_tools: HashSet::new(),
            session_path: None,
            spent_prompt_tokens: 0,
            spent_completion_tokens: 0,
            prompt_started: None,
            markdown_cache: HashMap::new(),
        }
    }
}

impl TabState {
    /// Move the active session's state out of `app`, leaving defaults behind.
    fn take_from(app: &mut App) -> Self {
        Self {
            session: std::mem::take(&mut app.session),
            input: std::mem::take(&mut app.input),
            scroll: std::mem::take(&mut app.scroll),
            input_scroll: std::mem::take(&mut app.input_scroll),
            transcript_height: std::mem::take(&mut app.transcript_height),
            follow_tail: std::mem::take(&mut app.follow_tail),
            send_pulse: app.send_pulse.take(),
            tools_mode: std::mem::take(&mut app.tools_mode),
            mode: std::mem::take(&mut app.mode),
            auto_compact: std::mem::take(&mut app.auto_compact),
            attachments: std::mem::take(&mut app.attachments),
            next_image: std::mem::take(&mut app.next_image),
            image_paths: std::mem::take(&mut app.image_paths),
            image_rows: std::mem::take(&mut app.image_rows),
            image_width: std::mem::take(&mut app.image_width),
            approval: app.approval.take(),
            running_tool: app.running_tool.take(),
            changes: std::mem::take(&mut app.changes),
            expanded: std::mem::take(&mut app.expanded),
            output_rows: std::mem::take(&mut app.output_rows),
            progress_collapsed: std::mem::take(&mut app.progress_collapsed),
            progress_row: app.progress_row.take(),
            composer_area: app.composer_area.take(),
            diff_sides: std::mem::take(&mut app.diff_sides),
            active: app.active.take(),
            pending_tools: std::mem::take(&mut app.pending_tools),
            approved_tools: std::mem::take(&mut app.approved_tools),
            session_path: app.session_path.take(),
            spent_prompt_tokens: std::mem::take(&mut app.spent_prompt_tokens),
            spent_completion_tokens: std::mem::take(&mut app.spent_completion_tokens),
            prompt_started: app.prompt_started.take(),
            markdown_cache: std::mem::take(&mut app.markdown_cache),
        }
    }

    /// Put this tab's state back into `app` as the active one.
    fn apply(self, app: &mut App) {
        app.session = self.session;
        app.input = self.input;
        app.scroll = self.scroll;
        app.input_scroll = self.input_scroll;
        app.transcript_height = self.transcript_height;
        app.follow_tail = self.follow_tail;
        app.send_pulse = self.send_pulse;
        app.tools_mode = self.tools_mode;
        app.mode = self.mode;
        app.auto_compact = self.auto_compact;
        app.attachments = self.attachments;
        app.next_image = self.next_image;
        app.image_paths = self.image_paths;
        app.image_rows = self.image_rows;
        app.image_width = self.image_width;
        app.approval = self.approval;
        app.running_tool = self.running_tool;
        app.changes = self.changes;
        app.expanded = self.expanded;
        app.output_rows = self.output_rows;
        app.progress_collapsed = self.progress_collapsed;
        app.progress_row = self.progress_row;
        app.composer_area = self.composer_area;
        app.diff_sides = self.diff_sides;
        app.active = self.active;
        app.pending_tools = self.pending_tools;
        app.approved_tools = self.approved_tools;
        app.session_path = self.session_path;
        app.spent_prompt_tokens = self.spent_prompt_tokens;
        app.spent_completion_tokens = self.spent_completion_tokens;
        app.prompt_started = self.prompt_started;
        app.markdown_cache = self.markdown_cache;
    }

    fn is_streaming(&self) -> bool {
        self.active.is_some()
    }

    fn is_busy(&self) -> bool {
        self.approval.is_some()
            || self.running_tool.is_some()
            || !self.pending_tools.is_empty()
            || self.active.is_some()
    }

    /// Write this background tab's history to disk, as `persist` does for the
    /// active one.
    fn persist(&self) {
        let Some(path) = self.session_path.clone() else {
            return;
        };
        if self.session.messages.is_empty() && !path.exists() {
            return;
        }
        let _ = self.session.save(&path);
    }
}

impl App {
    fn stash(&mut self) {
        self.tabs[self.tab] = TabState::take_from(self);
    }

    fn unstash(&mut self) {
        let state = std::mem::take(&mut self.tabs[self.tab]);
        state.apply(self);
    }

    /// Run `f` with tab `index` swapped in as the active session, whichever
    /// tab the user is looking at. Background work lands this way.
    pub(crate) fn with_tab<R>(&mut self, index: usize, f: impl FnOnce(&mut App) -> R) -> R {
        if index >= self.tabs.len() || index == self.tab {
            return f(self);
        }
        let home = self.tab;
        self.tabs[home] = TabState::take_from(self);
        self.tab = index;
        let state = std::mem::take(&mut self.tabs[index]);
        state.apply(self);
        let result = f(self);
        self.tabs[index] = TabState::take_from(self);
        self.tab = home;
        let state = std::mem::take(&mut self.tabs[home]);
        state.apply(self);
        result
    }

    pub(crate) fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub(crate) fn tab_index(&self) -> usize {
        self.tab
    }

    pub(crate) fn open_tab(&mut self) {
        if self.tabs.len() >= MAX_TABS {
            self.info(format!("At most {MAX_TABS} tabs; close one first."));
            return;
        }
        let auto_compact = self.auto_compact;
        self.stash();
        let mut state = TabState::default();
        if let Some(dir) = &self.sessions_dir {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_millis() as u64);
            state.session_path = Some(dir.join(format!("session-{stamp}.json")));
        }
        self.tabs.push(state);
        self.tab = self.tabs.len() - 1;
        self.tabs[self.tab].auto_compact = auto_compact;
        self.unstash();
        self.info(format!(
            "Tab {} — {} · alt+w closes · alt+arrows switch",
            self.tab + 1,
            self.session.title
        ));
    }

    /// Close the active tab, saving its history first.
    pub(crate) fn close_tab(&mut self) {
        self.close_tab_at(self.tab);
    }

    /// Close tab `index`, saving its history first. Mouse clicks on `×`.
    pub(crate) fn close_tab_at(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        if self.tabs.len() == 1 {
            self.info("The last tab cannot close; ctrl+c quits instead.");
            return;
        }
        if index == self.tab {
            if self.is_busy() {
                self.info("Finish the current turn before closing this tab.");
                return;
            }
            self.save_session();
            self.stash();
            self.tabs.remove(index);
            self.tab = self.tab.min(self.tabs.len() - 1);
            self.unstash();
            return;
        }
        if self.tabs[index].is_busy() {
            self.info("That tab is busy; finish its turn before closing it.");
            return;
        }
        self.with_tab(index, App::save_session);
        self.tabs.remove(index);
        if self.tab > index {
            self.tab -= 1;
        }
    }

    pub(crate) fn cycle_tab(&mut self, delta: isize) {
        if self.tabs.len() < 2 {
            return;
        }
        let count = self.tabs.len() as isize;
        let next = (self.tab as isize + delta).rem_euclid(count) as usize;
        self.select_tab(next);
    }

    pub(crate) fn select_tab(&mut self, index: usize) {
        if index >= self.tabs.len() || index == self.tab {
            return;
        }
        if self.tabs[index].is_busy() {
            self.info("That tab is busy; switch anyway with /tab? It streams on.");
        }
        self.stash();
        self.tab = index;
        self.unstash();
    }

    /// Which tab owns the streaming request `id`?
    pub(crate) fn tab_for_request(&self, id: u64) -> Option<usize> {
        if self.active.as_ref().is_some_and(|active| active.id == id) {
            return Some(self.tab);
        }
        self.tabs
            .iter()
            .position(|state| state.active.as_ref().is_some_and(|active| active.id == id))
    }

    /// Which tab owns the running tool call `call_id`?
    pub(crate) fn tab_for_tool(&self, call_id: &str) -> Option<usize> {
        if self
            .running_tool
            .as_ref()
            .is_some_and(|running| running.call.id == call_id)
        {
            return Some(self.tab);
        }
        self.tabs.iter().position(|state| {
            state
                .running_tool
                .as_ref()
                .is_some_and(|running| running.call.id == call_id)
        })
    }

    /// Which tab holds the session with id `id`?
    pub(crate) fn tab_for_session(&self, id: u64) -> Option<usize> {
        if self.session.id == id {
            return Some(self.tab);
        }
        self.tabs.iter().position(|state| state.session.id == id)
    }

    /// Whether tab `index` is mid-request, for the tab bar spinner.
    pub(crate) fn tab_streaming(&self, index: usize) -> bool {
        if index == self.tab {
            self.is_streaming()
        } else {
            self.tabs[index].is_streaming()
        }
    }

    /// Whether tab `index` has a pending approval or running tool, for the
    /// tab bar marker.
    pub(crate) fn tab_busy(&self, index: usize) -> bool {
        if index == self.tab {
            self.approval.is_some() || self.running_tool.is_some() || !self.pending_tools.is_empty()
        } else {
            let state = &self.tabs[index];
            state.approval.is_some()
                || state.running_tool.is_some()
                || !state.pending_tools.is_empty()
        }
    }

    /// Whether any tab still streams: the event loop keeps ticking while it
    /// does, so background spinners stay alive.
    pub(crate) fn any_streaming(&self) -> bool {
        self.is_streaming() || self.tabs.iter().any(TabState::is_streaming)
    }

    /// Whether any tab is busy, ticking the spinner for background work.
    pub(crate) fn any_tools_busy(&self) -> bool {
        (0..self.tabs.len()).any(|index| self.tab_busy(index))
    }

    pub(crate) fn tab_title(&self, index: usize) -> String {
        let title = if index == self.tab {
            self.session.title.trim()
        } else {
            self.tabs[index].session.title.trim()
        };
        let mut title: String = title.chars().take(25).collect();
        if title.chars().count() > 24 {
            title = title.chars().take(24).collect();
            title.push('…');
        }
        title
    }

    /// Save every tab's history: the active one through the snapshot writer,
    /// the parked ones directly, because this runs on the way out.
    pub(crate) fn persist_all(&self) -> Result<()> {
        let result = self.persist();
        for (index, state) in self.tabs.iter().enumerate() {
            if index != self.tab {
                state.persist();
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        crate::app::turn::tests::test_app()
    }

    #[test]
    fn open_and_close_tabs_swap_sessions() {
        let mut app = test_app();
        app.open_tab();
        assert_eq!(app.tab_count(), 2);
        assert_eq!(app.tab_index(), 1);
        app.session.update_title("second tab");
        app.select_tab(0);
        assert_eq!(app.session.title, "New session");
        app.select_tab(1);
        assert_eq!(app.session.title, "second tab");
        app.close_tab();
        assert_eq!(app.tab_count(), 1);
        assert_eq!(app.session.title, "New session");
    }

    #[test]
    fn with_tab_runs_against_the_named_tab() {
        let mut app = test_app();
        app.open_tab();
        let before = app.session.id;
        app.with_tab(0, |app| {
            app.session.update_title("background");
        });
        assert_eq!(app.tab_index(), 1);
        assert_eq!(app.session.id, before);
        app.select_tab(0);
        assert_eq!(app.session.title, "background");
    }

    #[test]
    fn open_tab_keeps_the_auto_compact_setting() {
        let mut app = test_app();
        app.auto_compact = false;
        app.open_tab();
        assert!(!app.auto_compact);
        app.auto_compact = true;
        app.open_tab();
        assert!(app.auto_compact);
    }

    #[test]
    fn close_tab_at_closes_a_background_tab() {
        let mut app = test_app();
        app.open_tab();
        app.session.update_title("second");
        app.close_tab_at(0);
        assert_eq!(app.tab_count(), 1);
        assert_eq!(app.tab_index(), 0);
        assert_eq!(app.session.title, "second");
    }

    #[test]
    fn tab_lookups_route_by_request_and_tool() {
        let mut app = test_app();
        app.open_tab();
        app.with_tab(0, |app| {
            app.next_request_id = app.next_request_id.wrapping_add(1);
            let id = app.next_request_id;
            app.active = Some(ActiveRequest {
                id,
                started: Instant::now(),
                prompt_tokens: 0,
                usage: None,
                tool_progress: None,
                compacting: None,
            });
        });
        let request_id = app.tabs[0].active.as_ref().unwrap().id;
        assert_eq!(app.tab_for_request(request_id), Some(0));
        assert_eq!(app.tab_for_request(u64::MAX), None);
    }
}
