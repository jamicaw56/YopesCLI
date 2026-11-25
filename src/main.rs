mod app;
mod backend;
mod changelog;
mod clipboard;
mod config;
mod credentials;
mod delegation;
mod diff;
mod discord;
mod hooks;
mod image_api;
mod mcp;
mod model_api;
mod models;
mod notify;
mod session;
mod skills;
mod temp;
mod terminal;
mod theme;
mod tools;
mod ui;
mod update;
mod usage;
mod weather;

use std::{collections::HashMap, future::Future, pin::Pin, time::Instant};

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{
    Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
use futures_util::StreamExt;
#[cfg(unix)]
use tokio::signal::unix::{SignalKind, signal};
use tokio::{sync::mpsc, task::JoinHandle, time::Duration};

use clipboard::read_clipboard;
use terminal::{TerminalGuard, ctrl_c_interrupt};

use crate::{
    app::{Action, App, Mode, ToolEvent, ToolsMode},
    backend::{Backend, StreamEvent},
    config::{AuthKind, Cli, Command, Config, load_session},
    session::{Message, Role, ToolCall},
};

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("yopes: {error:#}");
        std::process::exit(1);
    }
}

/// The message `yopes --resume` sends into a session that was cut off
/// mid-turn. It is a real user turn in the transcript, so the model sees
/// exactly what a person typing "continue" would have sent.
const RESUME_PROMPT: &str = "Continue from where you left off.";

/// Every background task talks to the loop through one bundle of senders.
struct Channels {
    events: mpsc::UnboundedSender<StreamEvent>,
    tools: mpsc::UnboundedSender<ToolEvent>,
    delegates: mpsc::UnboundedSender<crate::app::DelegateUpdate>,
    user_commands: mpsc::UnboundedSender<(u64, Result<String, String>)>,
    models: mpsc::UnboundedSender<(Option<String>, Result<Vec<(String, String)>, String>)>,
    images: mpsc::UnboundedSender<Result<Vec<u8>, String>>,
    paste: mpsc::UnboundedSender<Result<(String, String), String>>,
    titles: mpsc::UnboundedSender<(u64, String)>,
}

async fn run() -> Result<()> {
    terminal::install_panic_hook();
    let cli = Cli::parse();
    if matches!(cli.command, Some(Command::Update)) {
        return update::run().await;
    }
    // `yopes run "task"`: one task, no interface, exit when the agent is idle.
    let headless_task = match &cli.command {
        Some(Command::Run { task }) => Some(task.clone()),
        _ => None,
    };
    let headless = headless_task.is_some();
    // Read before `from_cli` consumes the flags: a resume that finds an
    // interrupted session continues it without a typed prompt.
    let resume = cli.resume_requested();
    let config = Config::from_cli(cli)?;
    // The theme is global state read by every draw call, so set it first.
    crate::theme::set(&config.theme);
    // Weather is opt-in: it polls only with a saved place and theme `auto`.
    crate::weather::follow(
        config::load_weather(&config.config_path).as_ref(),
        crate::theme::current_name() == "auto",
    );
    let session_path = config.session_path.clone();
    let sessions_dir = config.sessions_dir.clone();
    let config_path = config.config_path.clone();
    let session = load_session(session_path.as_deref())?;
    let backend = Backend::new(&config)?;
    let custom_providers = config.custom_providers.clone();
    let tavily_api_key = config.tavily_api_key.clone();
    let auto_compact = config.auto_compact;
    let saved_permission = config.permission.clone();
    let saved_diff_sides = config.diff_sides;
    let mut app = App::new(
        session,
        backend,
        session_path,
        sessions_dir,
        config_path,
        custom_providers,
        tavily_api_key,
        auto_compact,
    );
    if let Some(mode) = saved_permission.as_deref().and_then(ToolsMode::parse) {
        app.tools_mode = mode;
    }
    if let Some(sides) = saved_diff_sides {
        app.diff_sides = sides;
    }
    // The changelog opens once per build: a fresh version shows what changed,
    // and closing it records the version so the next start stays quiet.
    if config::last_seen_version(&config.config_path)
        .as_deref()
        .is_none_or(|seen| seen != update::current_version())
        && let Some(view) = crate::app::ChangeLogView::for_current_version()
    {
        app.changelog = Some(view);
    }
    // One-shot runs need no terminal: raw mode and the alternate screen want
    // a tty, and a CI runner has none.
    let mut terminal = if headless {
        None
    } else {
        Some(TerminalGuard::new()?)
    };
    let (sender, mut receiver) = mpsc::unbounded_channel::<StreamEvent>();
    let (tool_sender, mut tool_receiver) = mpsc::unbounded_channel::<ToolEvent>();
    let (delegate_sender, mut delegate_receiver) =
        mpsc::unbounded_channel::<crate::app::DelegateUpdate>();
    let (user_cmd_sender, mut user_cmd_receiver) =
        mpsc::unbounded_channel::<(u64, Result<String, String>)>();
    let (models_sender, mut models_receiver) =
        mpsc::unbounded_channel::<(Option<String>, Result<Vec<(String, String)>, String>)>();
    let (image_sender, mut image_receiver) = mpsc::unbounded_channel::<Result<Vec<u8>, String>>();
    let (paste_sender, mut paste_receiver) =
        mpsc::unbounded_channel::<Result<(String, String), String>>();
    let (title_sender, mut title_receiver) = mpsc::unbounded_channel::<(u64, String)>();
    let (bash_sender, mut bash_receiver) = mpsc::unbounded_channel::<tools::BashNotice>();
    // The talk tools (check_session, talk_to) send their requests through
    // this channel: the app loop owns every tab, so only it can resolve a
    // tab number and deliver a message into another session.
    let (talk_sender, mut talk_receiver) = mpsc::unbounded_channel::<tools::TalkRequest>();
    tools::install_talk(talk_sender);
    let channels = Channels {
        events: sender,
        tools: tool_sender,
        delegates: delegate_sender,
        user_commands: user_cmd_sender,
        models: models_sender,
        images: image_sender,
        paste: paste_sender,
        titles: title_sender,
    };
    // One watcher scans the background bash jobs: finished (or timed-out)
    // ones become notices the owning conversation reads without ever
    // spending a model turn polling `bash_output`.
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        loop {
            interval.tick().await;
            for notice in tools::poll_bash_jobs() {
                let _ = bash_sender.send(notice);
            }
        }
    });
    let mut terminal_events = if headless {
        None
    } else {
        Some(EventStream::new())
    };
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Ctrl+C is a key in raw mode, but a real interrupt must also work when the
    // key event is swallowed, or while a blocking tool is running.
    let interrupt = ctrl_c_interrupt();
    tokio::pin!(interrupt);
    let mut term_signal: Pin<Box<dyn Future<Output = ()> + Send>> = {
        #[cfg(unix)]
        {
            let mut term = signal(SignalKind::terminate())
                .context("installing the termination signal handler")?;
            Box::pin(async move {
                let _ = term.recv().await;
            })
        }
        #[cfg(not(unix))]
        {
            Box::pin(std::future::pending::<()>())
        }
    };
    // Streaming requests live across tabs: one task per request id, so a
    // background tab keeps streaming while another is on screen.
    let mut active_tasks: HashMap<u64, JoinHandle<()>> = HashMap::new();

    // One-shot mode submits the task like a typed message, then lets the loop
    // below run the turn chain to completion. `headless_start` marks where the
    // run's messages begin, so a resumed session's older errors cannot fail it.
    let mut headless_started = false;
    let mut headless_start = 0;
    if let Some(task) = headless_task {
        // Nothing can answer an approval prompt, so tools run unattended.
        app.tools_mode = ToolsMode::Auto;
        headless_start = app.session.messages.len();
        app.input.set(task);
        let action = app.submit();
        headless_started = matches!(action, Action::Start { .. });
        dispatch(action, &mut app, &channels, &mut active_tasks);
        if !headless_started {
            // The text never became a turn: no provider is connected, or it
            // parsed as a command instead of a task.
            let reason = app.notice.take().map_or_else(
                || "the task did not start a turn".to_owned(),
                |notice| notice.message,
            );
            return Err(anyhow::anyhow!("{reason}"));
        }
    }

    // `yopes --resume` picks up where the last run was cut off. The session
    // file only carries the interrupted marker while a request was in flight,
    // so a session that finished still waits for a real prompt.
    if resume && !headless && app.session.interrupted {
        app.input.set(RESUME_PROMPT.to_owned());
        let action = app.submit();
        dispatch(action, &mut app, &channels, &mut active_tasks);
    }

    // Minimum gap between frames generated by streaming text.
    const STREAM_REDRAW_MILLIS: u128 = 50;
    let mut redraw_now = true;
    let mut redraw_streamed = false;
    let mut last_draw = Instant::now();
    // When the previous key arrived: a multi-line paste from a terminal
    // without bracketed paste is recognised by its Enter keys following this
    // closely (see `is_pasted_line_break`).
    let mut last_key: Option<Instant> = None;
    // Frame-time trace: YOPES_TRACE=1 prints the draw count, the average and
    // the worst frame to stderr on exit, so the perf work is measured, not
    // guessed.
    let trace = std::env::var("YOPES_TRACE").is_ok_and(|v| v == "1" || v == "true");
    let mut frames = 0u64;
    let mut total_draw_ms = 0u64;
    let mut worst_draw_ms = 0u64;
    // Discord Rich Presence: on with the shared application id unless
    // `/set discord off` turned it off, so nothing here runs when it is off.
    let mut discord = discord::Runtime::default();

    loop {
        if app.should_quit {
            break;
        }
        // One comparison when nothing changed, and a write only when the card
        // would actually differ (or the periodic re-send is due).
        discord.sync(
            app.discord_client_id
                .clone()
                .map(|id| (id, discord::presence(&app))),
        );
        // Redrawing re-lays-out the whole transcript, so stream text redraws
        // at a fixed cadence while user input still gets an immediate frame.
        // A headless run has no terminal, so it draws nothing at all.
        if let Some(guard) = terminal.as_mut() {
            if redraw_now
                || (redraw_streamed && last_draw.elapsed().as_millis() >= STREAM_REDRAW_MILLIS)
            {
                let draw_started = trace.then(Instant::now);
                guard
                    .terminal
                    .draw(|frame| ui::draw(frame, &mut app))
                    .context("drawing terminal UI")?;
                if let Some(started) = draw_started {
                    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                    frames += 1;
                    total_draw_ms += ms;
                    worst_draw_ms = worst_draw_ms.max(ms);
                }
                redraw_now = false;
                redraw_streamed = false;
                last_draw = Instant::now();
            }
        }

        tokio::select! {
            event = next_terminal_event(&mut terminal_events) => {
                let dirty =
                    handle_terminal_event(&mut app, event, &channels, &mut active_tasks, &mut ticker, &mut last_key)?;
                redraw_now = dirty;
                // Coalesce: every event already queued in the terminal is
                // handled now, so one burst of typing costs one frame instead
                // of one full frame per keystroke. A zero timeout polls the
                // stream once without waiting.
                loop {
                    match tokio::time::timeout(
                        Duration::ZERO,
                        next_terminal_event(&mut terminal_events),
                    )
                    .await
                    {
                        Ok(Some(next)) => {
                            if handle_terminal_event(
                                &mut app,
                                Some(next),
                                &channels,
                                &mut active_tasks,
                                &mut ticker,
                                &mut last_key,
                            )? {
                                redraw_now = true;
                            }
                        }
                        Ok(None) => {
                            return Err(anyhow::anyhow!("terminal event stream closed"))
                        }
                        Err(_) => break,
                    }
                }
            }
            event = receiver.recv() => {
                let Some(event) = event else { break; };
                let request_id = event.request_id();
                // Text deltas arrive in bursts and can be drawn at the fixed
                // cadence; lifecycle changes (usage, tool calls, completion)
                // need an immediate frame.
                let text_event = event.is_text();
                // Route to the tab that owns the request: a background tab
                // streams on while another is on screen, and its events must
                // land in its own session.
                let owner = app.tab_for_request(request_id);
                if event.is_final() {
                    active_tasks.remove(&request_id);
                }
                app.with_owner(owner, |app| {
                    let action = app.on_stream_event(event);
                    dispatch(action, app, &channels, &mut active_tasks);
                });
                if text_event {
                    redraw_streamed = true;
                } else {
                    redraw_now = true;
                }
            }
            result = tool_receiver.recv() => {
                let Some(result) = result else { break; };
                // Tool results also route by owner, so a background tab's
                // tool call finishes in its own session.
                let owner = app.tab_for_tool(&result.call.id);
                app.with_owner(owner, |app| {
                    let action = app.on_tool_result(result);
                    dispatch(action, app, &channels, &mut active_tasks);
                });
                redraw_now = true;
            }
            result = models_receiver.recv() => {
                if let Some((base_url, result)) = result {
                    app.on_custom_models(base_url, result);
                }
                redraw_now = true;
            }
            result = image_receiver.recv() => {
                if let Some(result) = result {
                    app.on_generated_image(result);
                }
                redraw_now = true;
            }
            result = paste_receiver.recv() => {
                let Some(result) = result else { break; };
                app.on_paste_image(result);
                redraw_now = true;
            }
            update = delegate_receiver.recv() => {
                let Some(update) = update else { break; };
                // Subagent deltas stream at the fixed cadence; a lifecycle
                // move or a verdict needs an immediate frame.
                let verdict = match &update {
                    crate::app::DelegateUpdate::Verdict {
                        run,
                        state,
                        reasons,
                        files,
                        report,
                        session_id,
                    } => Some((
                        *run,
                        *state,
                        reasons.clone(),
                        files.clone(),
                        report.clone(),
                        *session_id,
                    )),
                    _ => None,
                };
                let state_move = matches!(&update, crate::app::DelegateUpdate::State { .. });
                app.on_delegate_update(update);
                if let Some((run_id, state, reasons, files, report, session_id)) = verdict {
                    // The verdict lands in the session that delegated the
                    // run. The ledger is the authority, so a switched tab
                    // never reroutes a report into the conversation on
                    // screen; the spawn-time id is the fallback for a run
                    // the ledger no longer holds.
                    let session = app.session_for_run(run_id).unwrap_or(session_id);
                    let owner = app.tab_for_session(session);
                    match owner {
                        Some(owner) => app.with_tab(owner, |app| {
                            let action =
                                app.on_subagent_verdict(run_id, state, reasons, files, report);
                            dispatch(action, app, &channels, &mut active_tasks);
                        }),
                        None => app.info(format!(
                            "Run {run_id} finished, but the session that delegated it is                              closed; its report is in the ledger of that session file."
                        )),
                    }
                    redraw_now = true;
                } else if state_move {
                    redraw_now = true;
                } else {
                    redraw_streamed = true;
                }
            }
            result = user_cmd_receiver.recv() => {
                let Some((session_id, result)) = result else { break; };
                // A command belongs to the session that started it, even when
                // the user switched tabs while it ran.
                let owner = app.tab_for_session(session_id);
                app.with_owner(owner, |app| {
                    let action = app.on_user_command_result(result);
                    dispatch(action, app, &channels, &mut active_tasks);
                });
                redraw_now = true;
            }
            title = title_receiver.recv() => {
                let Some((session_id, title)) = title else { break; };
                // A background tab may have asked for the title: land it in
                // the session that asked, whichever tab is on screen.
                let owner = app.tab_for_session(session_id);
                app.with_owner(owner, |app| app.on_session_title(session_id, title));
                redraw_now = true;
            }
            notice = bash_receiver.recv() => {
                let Some(notice) = notice else { break; };
                // The notice routes to the session that started the job; a
                // closed session falls back to the tab on screen.
                let owner = app.tab_for_session(notice.owner);
                app.with_owner(owner, |app| app.on_bash_notice(notice));
                redraw_now = true;
            }
            request = talk_receiver.recv() => {
                let Some(request) = request else { break; };
                // A talk request is answered from the live tab directory; a
                // message that woke an idle tab returns that tab and the
                // action to run, dispatched with the tab swapped in so the
                // wake uses its own provider and working directory.
                if let Some((owner, action)) = app.on_talk_request(request) {
                    app.with_tab(owner, |app| {
                        dispatch(action, app, &channels, &mut active_tasks);
                    });
                }
                redraw_now = true;
            }
            _ = &mut interrupt => break,
            _ = term_signal.as_mut() => break,
            _ = ticker.tick(),
            // The ticker only redraws when something visible animates or a
            // timer has to run; see `App::ticker_wanted`.
            if app.focused && app.ticker_wanted() => {
                app.tick();
                redraw_now = true;
            }
        }

        // One-shot mode is done once the task's turn chain settles: nothing
        // streams, no tool or subagent runs, and nothing is parked for later.
        if headless_started
            && !app.is_busy()
            && !app.any_subagent_running()
            && app.pending_reports.is_empty()
            && app.pending_talks.is_empty()
        {
            break;
        }
    }

    for (_, task) in active_tasks.drain() {
        task.abort();
    }
    // Any foreground command still running dies with the process: no tool
    // leaves an orphan behind.
    tools::kill_spawned_commands();
    if trace {
        eprintln!(
            "yopes: {frames} frames, avg {} ms, worst {worst_draw_ms} ms",
            total_draw_ms.checked_div(frames).unwrap_or(0)
        );
    }
    drop(terminal);
    app.persist_all()?;
    // One-shot mode exits non-zero when the provider failed: the failure is
    // recorded as an error message in this run's reply block.
    let failure = if headless_started {
        app.session
            .messages
            .get(headless_start..)
            .unwrap_or_default()
            .iter()
            .find_map(|message| message.error.as_deref())
    } else {
        None
    };
    if let Some(error) = failure {
        return Err(anyhow::anyhow!("{error}"));
    }
    Ok(())
}

/// The next terminal event, when there is a terminal to read.
///
/// A headless run has none: the future never resolves, so the select branch
/// stays quiet and the loop runs on the channel events alone.
async fn next_terminal_event(events: &mut Option<EventStream>) -> Option<std::io::Result<Event>> {
    match events.as_mut() {
        Some(events) => events.next().await,
        None => std::future::pending().await,
    }
}

/// How close to the previous key an Enter has to arrive to count as a pasted
/// line break. A terminal that supports bracketed paste never needs this; a
/// legacy one delivers a multi-line paste as characters plus Enter keys, and
/// without the guard each line submits on its own.
const PASTE_LINE_GAP: Duration = Duration::from_millis(20);

/// Whether an Enter key is really the line break of a paste.
fn is_pasted_line_break(key: &KeyEvent, previous: Option<Instant>, now: Instant) -> bool {
    key.kind == KeyEventKind::Press
        && key.code == KeyCode::Enter
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT)
        && previous.is_some_and(|last| now.duration_since(last) < PASTE_LINE_GAP)
}

/// One terminal event through the app: returns whether the frame is dirty.
/// Extracted so the event loop can coalesce a queue of already-arrived
/// events into a single frame. Generic over the stream's error type, which
/// crossterm has changed across versions.
fn handle_terminal_event<E>(
    app: &mut App,
    event: Option<Result<Event, E>>,
    channels: &Channels,
    active_tasks: &mut HashMap<u64, JoinHandle<()>>,
    ticker: &mut tokio::time::Interval,
    last_key: &mut Option<Instant>,
) -> Result<bool>
where
    E: std::error::Error + Send + Sync + 'static,
{
    match event {
        Some(Ok(Event::Key(key))) => {
            // A newline that follows another key within a few milliseconds is
            // a paste from a terminal without bracketed paste: it belongs in
            // the composer as a line break, not as one prompt per line.
            let now = Instant::now();
            let previous = *last_key;
            if key.kind == KeyEventKind::Press {
                *last_key = Some(now);
            }
            if is_pasted_line_break(&key, previous, now) && !app.input.value.trim().is_empty() {
                app.handle_paste("\n");
                return Ok(true);
            }
            let action = app.handle_key(key);
            dispatch(action, app, channels, active_tasks);
            Ok(true)
        }
        Some(Ok(Event::Paste(value))) => {
            // An empty paste usually means the clipboard holds an image: the
            // terminal's own paste sends nothing for a screenshot, so the
            // empty event is the only signal.
            if value.trim().is_empty()
                && app.approval.is_none()
                && !app.connect_open()
                && !app.show_help
            {
                dispatch(Action::PasteImage, app, channels, active_tasks);
            } else {
                app.handle_paste(&value);
            }
            Ok(true)
        }
        Some(Ok(Event::Mouse(mouse))) => {
            // A mouse move changes nothing on screen, so it must not cost a
            // full transcript rebuild.
            if mouse.kind != MouseEventKind::Moved {
                let action = app.handle_mouse(mouse);
                dispatch(action, app, channels, active_tasks);
                Ok(true)
            } else {
                Ok(false)
            }
        }
        Some(Ok(Event::FocusGained)) => {
            ticker.reset();
            app.set_focused(true);
            Ok(true)
        }
        Some(Ok(Event::FocusLost)) => {
            // While the terminal is in the background nothing is visible, so
            // the ticker stops and no frame is drawn.
            app.set_focused(false);
            Ok(false)
        }
        Some(Ok(Event::Resize(_, _))) => Ok(true),
        Some(Err(error)) => Err(error).context("reading terminal event"),
        None => Err(anyhow::anyhow!("terminal event stream closed")),
    }
}

fn dispatch(
    action: Action,
    app: &mut App,
    channels: &Channels,
    active_tasks: &mut HashMap<u64, JoinHandle<()>>,
) {
    match action {
        Action::None | Action::Quit => {}
        Action::Start {
            request_id,
            messages,
            tools_enabled,
            mode_note,
        } => {
            let backend = app.backend.clone();
            let sender = channels.events.clone();
            // The request runs in the tab that asked: its prompt shows that
            // tab's own working directory.
            let workspace = app.working_dir().to_path_buf();
            let task = tokio::spawn(async move {
                // A panic in the request task must not leave the UI streaming a
                // response that will never arrive.
                let stream = std::panic::AssertUnwindSafe(backend.stream(
                    request_id,
                    messages,
                    tools_enabled,
                    mode_note,
                    workspace,
                    sender.clone(),
                ));
                if futures_util::FutureExt::catch_unwind(stream).await.is_err() {
                    let _ = sender.send(StreamEvent::Error {
                        request_id,
                        message: "The response task stopped unexpectedly. Send another message to continue."
                            .to_owned(),
                    });
                }
            });
            active_tasks.insert(request_id, task);
        }
        Action::Cancel(request_id) => cancel_task(active_tasks, request_id),
        Action::KillCommands(session_id) => {
            // Only this session's foreground commands die: the registry is
            // keyed by the session that spawned them.
            let killed = tools::kill_commands_for(session_id);
            if killed > 0 {
                app.info(format!(
                    "Interrupted {killed} running command{}.",
                    if killed == 1 { "" } else { "s" }
                ));
            }
        }
        Action::PasteImage => {
            let sender = channels.paste.clone();
            tokio::spawn(async move {
                // Anyhow's chain needs rendering before crossing the channel.
                let read = match tokio::task::spawn_blocking(read_clipboard).await {
                    Ok(Ok(result)) => Ok(result),
                    Ok(Err(error)) => Err(format!("{error:#}")),
                    Err(join) => Err(format!("paste task failed: {join}")),
                };
                let _ = sender.send(read);
            });
        }
        Action::CopyClipboard(text) => {
            tokio::task::spawn_blocking(move || {
                let _ = crate::clipboard::write_clipboard(&text);
            });
        }
        Action::RunTool { call } => spawn_tool_calls(vec![call], app, channels),
        Action::RunTools { calls } => spawn_tool_calls(calls, app, channels),
        Action::RunUserCommand { command } => {
            let sender = channels.user_commands.clone();
            let session_id = app.session.id;
            let workspace = app.working_dir().to_path_buf();
            tokio::spawn(async move {
                // A long build must not hold the event loop: the command runs
                // on the blocking pool and the result lands in its session.
                let joined = tokio::task::spawn_blocking(move || {
                    tools::run_user_command(&command, &workspace)
                })
                .await;
                let _ = sender.send((session_id, flatten_joined(joined, "command")));
            });
        }
        Action::FetchCustomModels {
            url,
            base_url,
            token,
            auth,
        } => {
            let sender = channels.models.clone();
            tokio::spawn(async move {
                let result = model_api::fetch_models(&url, auth, token.as_deref())
                    .await
                    .map_err(|error| format!("{error:#}"));
                let _ = sender.send((base_url, result));
            });
        }
        Action::GenerateImage {
            base_url,
            auth,
            key,
            model,
            prompt,
        } => {
            let sender = channels.images.clone();
            tokio::spawn(async move {
                let result = image_api::generate(&base_url, auth, key.as_deref(), &model, &prompt)
                    .await
                    .map_err(|error| format!("{error:#}"));
                let _ = sender.send(result);
            });
        }
        Action::NameSession { task } => {
            let backend = app.backend.clone();
            let sender = channels.titles.clone();
            let session_id = app.session.id;
            let workspace = app.working_dir().to_path_buf();
            tokio::spawn(async move {
                let brief = "You name coding sessions. Answer with only a short title, \
                             at most five words, capitalized, no quotes, no final \
                             punctuation. You have no tools.";
                // Errors stay silent: the session simply keeps a plain title.
                if let Ok(raw) = backend.complete_once(brief, &task, workspace).await {
                    let _ = sender.send((session_id, raw));
                }
            });
        }
    }
}

/// Flatten a blocking-pool join into the string result a channel carries.
fn flatten_joined(
    joined: Result<Result<String>, tokio::task::JoinError>,
    what: &str,
) -> Result<String, String> {
    match joined {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(format!("{error:#}")),
        Err(error) => Err(format!("{what} task failed: {error}")),
    }
}

fn cancel_task(active_tasks: &mut HashMap<u64, JoinHandle<()>>, request_id: u64) {
    if let Some(task) = active_tasks.remove(&request_id) {
        task.abort();
    }
}

/// How many times one subagent round retries a stream-level failure before
/// the run gives up: one provider hiccup must not kill hours of work.
const MAX_ROUND_ATTEMPTS: u32 = 3;

/// Spawn every tool call of one action. A delegate becomes a subagent run on
/// the same provider: it works in its own conversation with the same tool
/// surface as the main session, streams live through the delegate channel,
/// and its final answer lands as the tool result. Anything else runs on the
/// blocking pool as before.
fn spawn_tool_calls(calls: Vec<ToolCall>, app: &mut App, channels: &Channels) {
    let backend = app.backend.clone();
    let delegates = channels.delegates.clone();
    let tools = channels.tools.clone();
    let web_key = app.tavily_key().map(str::to_owned);
    let tools_mode = app.tools_mode;
    let mode = app.mode;
    let approved_tools = app.approved_tools.clone();
    let workspace = app.working_dir().to_path_buf();
    // Tool commands register under the owning session's id, so an Esc
    // interrupt is scoped to the tab that spawned them.
    let session_id = app.session.id;
    for call in calls {
        if call.function.name != "delegate" {
            let sender = tools.clone();
            let web_key = web_key.clone();
            let workspace = workspace.clone();
            tokio::spawn(async move {
                let task_call = call.clone();
                let joined = tokio::task::spawn_blocking(move || {
                    // Registered under the spawning session, so an Esc
                    // interrupt from another tab leaves this command alone.
                    tools::scoped_owner(session_id, || {
                        tools::execute(&task_call, web_key.as_deref(), &workspace)
                    })
                })
                .await;
                let result = flatten_joined(joined, "tool");
                let _ = sender.send(ToolEvent { call, result });
            });
            continue;
        }
        // The subagent: its own conversation on the same provider, with the
        // same tools the main session has, working in the same workspace.
        let task = match tools::delegate_task_checked(&call) {
            Ok(task) => task,
            Err(message) => {
                // A rejected call still answers, so the turn continues.
                let _ = tools.send(ToolEvent {
                    call,
                    result: Err(message),
                });
                continue;
            }
        };
        let spec = match crate::delegation::DelegateSpec::from_call(&call, task.clone()) {
            Ok(spec) => spec,
            Err(message) => {
                // A rejected call still answers, so the turn continues.
                let _ = tools.send(ToolEvent {
                    call,
                    result: Err(message),
                });
                continue;
            }
        };
        let kind = spec.kind;
        let timeout = spec.timeout;
        let run = app.begin_subagent(&call.id, task.clone(), kind);
        // The delegate answers immediately: the model regains control and
        // keeps working while the run streams in the sidebar. The run's
        // final answer is delivered as a report that alerts the model when
        // it finishes.
        let _ = tools.send(ToolEvent {
            call,
            result: Ok(format!(
                "Subagent run {run} started (kind: {}). It works in the background with the \
                 same tools, streams in the sidebar (/agents), and a verified report is \
                 delivered to this conversation when it finishes — keep working, do not wait \
                 for it. The run id addresses corrections and stops. A \"done\" from the \
                 subagent is not completion: yopes checks the files it changed and the \
                 commands it ran before reporting it finished.",
                kind.label()
            )),
        });
        // The model names the task: the sidebar and the /agents rows show
        // that name instead of the raw task text. A failed naming request
        // stays silent and the row keeps the task excerpt until then.
        {
            let backend = backend.clone();
            let delegates = delegates.clone();
            let task = task.clone();
            let workspace = workspace.clone();
            tokio::spawn(async move {
                if let Ok(raw) = backend
                    .complete_once(crate::app::SUBAGENT_NAME_BRIEF, &task, workspace)
                    .await
                {
                    let _ = delegates.send(crate::app::DelegateUpdate::Named { run, name: raw });
                }
            });
        }
        let delegate_sender = delegates.clone();
        let backend = backend.clone();
        let web_key = web_key.clone();
        let approved = approved_tools.clone();
        let root = workspace.clone();
        // Sibling runs that will still be working after this one reports:
        // their presence stops a finished report from reading as "all done".
        let others_active: Vec<u64> = app.active_runs_in_owner();
        // The failover ladder: the same provider's other models first, then
        // other configured providers whose key the run can use without a
        // password. A subagent that keeps failing switches down this list
        // instead of dying with hours of work behind it.
        let mut failover: Vec<(String, String, Option<String>, AuthKind)> = Vec::new();
        for saved in backend.saved_models() {
            if saved.id.eq_ignore_ascii_case(backend.model())
                || failover
                    .iter()
                    .any(|(model, ..)| model.eq_ignore_ascii_case(&saved.id))
            {
                continue;
            }
            failover.push((
                saved.id.clone(),
                backend.base_url().to_owned(),
                backend.api_key().map(str::to_owned),
                backend.auth(),
            ));
        }
        for provider in app.custom_providers() {
            if provider.base_url.trim_end_matches('/') == backend.base_url().trim_end_matches('/') {
                continue;
            }
            let key = provider
                .token
                .clone()
                .filter(|token| !token.trim().is_empty())
                .or_else(|| {
                    provider
                        .secret
                        .plaintext()
                        .map(str::to_owned)
                        .filter(|key| !key.trim().is_empty())
                });
            let Some(key) = key else { continue };
            let Some(first) = provider.models.first() else {
                continue;
            };
            if failover
                .iter()
                .any(|(model, ..)| model.eq_ignore_ascii_case(&first.id))
            {
                continue;
            }
            failover.push((
                first.id.clone(),
                provider.base_url.clone(),
                Some(key),
                provider.auth,
            ));
        }
        tokio::spawn(async move {
            // Corrections from the main agent join this conversation; the
            // stop flag ends the run cooperatively.
            let (correction_sender, mut correction_receiver) = mpsc::unbounded_channel::<String>();
            // Two flags: a user cancellation, and the main agent's own stop.
            let (stop, cancelled) = crate::tools::register_subagent_run(run, correction_sender);
            // The deadline: a run that hangs holds the coordinator's work
            // hostage, so it ends with whatever it has and is judged on it.
            let started = std::time::Instant::now();
            let deadline = tokio::time::Instant::now() + timeout;
            // What the run actually did, observed here instead of taken from
            // its answer: the verdict is decided from this at the end.
            let mut evidence = crate::delegation::Evidence::default();
            let mut timed_out = false;
            let tools_enabled = mode != Mode::Chat && tools_mode.advertised();
            let task = spec.task.clone();
            let mut messages = vec![Message::new(Role::User, task.clone())];
            let mut answer = String::new();
            let mut reasoning = String::new();
            let mut failed = false;
            let mut failure = String::new();
            // Consecutive stream-level failures of one round: the run gives
            // up only after MAX_ROUND_ATTEMPTS, instead of dying on the
            // first provider hiccup.
            let mut round_attempts = 0u32;
            // The model this round streams from: swapped down the failover
            // ladder when the current model or endpoint keeps failing.
            let mut run_backend = backend.clone();
            let mut failover = failover.into_iter();
            // One nudge each: the write watchdog asks a run that has spent
            // many tool rounds without changing any file to land an artifact;
            // the empty nudge asks again when a round streams nothing, so a
            // run that worked never ends with no report.
            let mut write_nudged = false;
            let mut empty_nudges = 0u32;
            // Tool rounds since the last round that changed a file: the
            // watchdog fires once this passes ZERO_WRITE_NUDGE_ROUNDS.
            let mut quiet_rounds = 0usize;
            let mut workspace = root;
            loop {
                let (events, mut receiver) = mpsc::unbounded_channel::<StreamEvent>();
                let streamer = {
                    let events = events.clone();
                    let backend = run_backend.clone();
                    let messages = messages.clone();
                    let workspace = workspace.clone();
                    tokio::spawn(async move {
                        backend
                            .stream(
                                u64::MAX,
                                messages,
                                tools_enabled,
                                Some(crate::app::SUBAGENT_BRIEF.to_owned()),
                                workspace,
                                events,
                            )
                            .await;
                    })
                };
                let mut round_text = String::new();
                let mut requested: Vec<ToolCall> = Vec::new();
                loop {
                    match crate::delegation::next_before(&mut receiver, deadline).await {
                        crate::delegation::Next::Item(event) => match event {
                            StreamEvent::Delta { text, .. } => {
                                answer.push_str(&text);
                                round_text.push_str(&text);
                                let _ = delegate_sender
                                    .send(crate::app::DelegateUpdate::Delta { run, text });
                            }
                            StreamEvent::Reasoning { text, .. } => {
                                reasoning.push_str(&text);
                                let _ = delegate_sender
                                    .send(crate::app::DelegateUpdate::Reasoning { run, text });
                            }
                            StreamEvent::Usage {
                                prompt_tokens,
                                completion_tokens,
                                ..
                            } => {
                                let _ = delegate_sender.send(crate::app::DelegateUpdate::Usage {
                                    run,
                                    prompt_tokens,
                                    completion_tokens,
                                });
                            }
                            StreamEvent::ToolCalls { calls, .. } => requested = calls,
                            StreamEvent::Retrying {
                                attempt,
                                delay_secs,
                                reason,
                                ..
                            } => {
                                let _ = delegate_sender.send(crate::app::DelegateUpdate::Note {
                                run,
                                text: format!(
                                    "attempt {attempt} failed: {reason} — retrying in {delay_secs}s"
                                ),
                            });
                            }
                            StreamEvent::Stalled { message, .. } => {
                                failed = true;
                                failure = message;
                                break;
                            }
                            StreamEvent::Error { message, .. } => {
                                failed = true;
                                failure = message;
                                break;
                            }
                            StreamEvent::OutputLimit { .. } => {
                                failed = true;
                                failure = "the reply hit the output token limit".to_owned();
                                break;
                            }
                            StreamEvent::Done { .. } => break,
                            _ => {}
                        },
                        crate::delegation::Next::Closed => break,
                        crate::delegation::Next::TimedOut => {
                            timed_out = true;
                            break;
                        }
                    }
                }
                let _ = streamer.await;
                if timed_out {
                    // The deadline passed: the run ends here with what it has
                    // and is judged on its evidence, not on a claim.
                    break;
                }
                if failed {
                    // Auto-continue: a stream-level failure retries the round
                    // a few times before the run gives up, the same way the
                    // main session retries. The round's messages are
                    // unchanged - it failed before any tool call ran.
                    round_attempts += 1;
                    if round_attempts < MAX_ROUND_ATTEMPTS {
                        let _ = delegate_sender.send(crate::app::DelegateUpdate::Note {
                            run,
                            text: format!("attempt {round_attempts} failed: {failure} — retrying"),
                        });
                        tokio::time::sleep(Duration::from_millis(1000 * u64::from(round_attempts)))
                            .await;
                        failed = false;
                        failure = String::new();
                        continue;
                    }
                    // The model or endpoint keeps failing: switch to the next
                    // one on the failover ladder instead of giving up.
                    if let Some((model, base_url, key, auth)) = failover.next() {
                        run_backend = backend.failover(&model, &base_url, key.as_deref(), auth);
                        round_attempts = 0;
                        let _ = delegate_sender.send(crate::app::DelegateUpdate::Note {
                            run,
                            text: format!("repeated failures — switching to {model} and retrying"),
                        });
                        failed = false;
                        failure = String::new();
                        continue;
                    }
                    break;
                }
                round_attempts = 0;
                if requested.is_empty() {
                    // Done: a correction already queued re-opens the
                    // conversation; otherwise the run finishes and reports at
                    // once, with no grace window delaying the model.
                    if let Ok(correction) = correction_receiver.try_recv() {
                        messages.push(Message::new(Role::User, correction));
                        let _ = delegate_sender.send(crate::app::DelegateUpdate::Note {
                            run,
                            text: "the main agent sent a correction — continuing".to_owned(),
                        });
                        continue;
                    }
                    // The model streamed nothing this round: nudge it to
                    // answer with what it has instead of ending the run with
                    // no report.
                    if round_text.trim().is_empty() && answer.trim().is_empty() && empty_nudges < 2
                    {
                        empty_nudges += 1;
                        messages.push(Message::new(
                            Role::User,
                            "Your last reply was empty. Reply now with your final answer to "
                                .to_owned()
                                + "the task, based on the work done so far.",
                        ));
                        let _ = delegate_sender.send(crate::app::DelegateUpdate::Note {
                            run,
                            text: "the model streamed nothing — asking it to answer".to_owned(),
                        });
                        continue;
                    }
                    break;
                }
                if timed_out {
                    // A deadline that passed mid-round ends the run with what
                    // it has; the verdict names the timeout.
                    break;
                }
                // Corrections sent mid-round join the conversation before the
                // next request, so the model sees them with the tool results.
                while let Ok(correction) = correction_receiver.try_recv() {
                    messages.push(Message::new(Role::User, correction));
                }
                if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = delegate_sender.send(crate::app::DelegateUpdate::Note {
                        run,
                        text: "cancelled by the user — answering with what it has".to_owned(),
                    });
                    break;
                }
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = delegate_sender.send(crate::app::DelegateUpdate::Note {
                        run,
                        text: "stopped by the main agent — answering with what it has".to_owned(),
                    });
                    break;
                }
                // Did this round change a file? A round that runs none of
                // the writing tools counts toward the write watchdog.
                let mut mutated = false;
                // The assistant turn that asked for the calls, then one tool
                // result per call, keeps the next request valid for every
                // provider.
                evidence.rounds += 1;
                let mut asked = Message::new(Role::Assistant, std::mem::take(&mut round_text));
                asked.tool_calls = Some(requested.clone());
                messages.push(asked);
                for requested_call in requested {
                    // The same policy the main session applies, scoped to the
                    // tab's own workspace for the approval rules.
                    let needs = tools::scoped_workspace(&workspace, || {
                        tools_mode.needs_approval(&requested_call, &approved)
                    });
                    let blocked = if mode.denied(&requested_call.function.name) {
                        Some(format!(
                            "This tool call was blocked: {} mode only proposes work.",
                            mode.label()
                        ))
                    } else if requested_call.function.name == "delegate" {
                        // A subagent cannot spawn further subagents.
                        Some(
                            "A subagent cannot delegate; do the work yourself with the other \
                             tools."
                                .to_owned(),
                        )
                    } else if needs {
                        Some(
                            "This tool needs the user's approval and a subagent cannot ask for \
                             it. Report that it must run in the main session instead."
                                .to_owned(),
                        )
                    } else {
                        None
                    };
                    let result = match blocked {
                        Some(message) => {
                            // A refusal for lack of approval is what makes a
                            // run read as blocked instead of simply empty.
                            if needs {
                                evidence.blocked.push(requested_call.function.name.clone());
                            }
                            let _ = delegate_sender.send(crate::app::DelegateUpdate::Note {
                                run,
                                text: format!("⚠ {} — {}", requested_call.function.name, message),
                            });
                            Err(message)
                        }
                        None => {
                            let _ = delegate_sender.send(crate::app::DelegateUpdate::Note {
                                run,
                                text: format!(
                                    "→ {} {}",
                                    requested_call.function.name,
                                    tools::preview(&requested_call)
                                ),
                            });
                            let task_call = requested_call.clone();
                            let web_key = web_key.clone();
                            let root = workspace.clone();
                            // What the call is about to do, read before it
                            // runs: the targets of a file write, and the
                            // worktree state around a command.
                            let writes_files = crate::app::WRITING_TOOLS
                                .contains(&requested_call.function.name.as_str());
                            let is_command = matches!(
                                requested_call.function.name.as_str(),
                                "run_command" | "bash_background"
                            );
                            let snapshot = writes_files.then(|| {
                                crate::delegation::snapshot_targets(&requested_call, &root)
                            });
                            let before_files = is_command
                                .then(|| crate::delegation::fingerprint(&root))
                                .flatten();
                            let after_root = root.clone();
                            let joined = tokio::task::spawn_blocking(move || {
                                // A subagent's commands belong to the session
                                // that delegated the work: an interrupt from
                                // its own tab kills them; other tabs' survive.
                                tools::scoped_owner(session_id, || {
                                    tools::execute(&task_call, web_key.as_deref(), &root)
                                })
                            })
                            .await;
                            let result = match joined {
                                Ok(Ok(output)) => Ok(output),
                                Ok(Err(error)) => Err(format!("{error:#}")),
                                Err(join) => Err(format!("tool task failed: {join}")),
                            };
                            // A successful move_to re-roots this subagent's
                            // own work; the tab and other subagents keep
                            // theirs.
                            if result.is_ok() && requested_call.function.name == "move_to" {
                                if let Some(target) =
                                    tools::move_target(&requested_call, &workspace)
                                {
                                    workspace = target;
                                }
                            }
                            // The evidence, observed rather than claimed.
                            if let Some(snapshot) = snapshot {
                                let (changes, unchanged) = crate::delegation::compare(snapshot);
                                if changes.is_empty() && result.is_ok() {
                                    // A write call that left the bytes alone:
                                    // the coordinator's "it was updated" is
                                    // worth nothing, and the run must not be
                                    // credited for it.
                                    evidence.noop_writes += 1;
                                }
                                let _ = unchanged;
                                for change in changes {
                                    evidence.record_change(change);
                                }
                            }
                            if is_command {
                                let command =
                                    crate::tools::command_text(&requested_call).unwrap_or_default();
                                evidence.commands.push(
                                    crate::delegation::CommandRecord::from_result(
                                        &command, &result,
                                    ),
                                );
                                match (before_files, crate::delegation::fingerprint(&after_root)) {
                                    (Some(before), Some(after)) => {
                                        for change in before.changed_since(&after) {
                                            evidence.record_change(change);
                                        }
                                    }
                                    (None, _) => evidence.unobserved_commands += 1,
                                    _ => {}
                                }
                            }
                            if result.is_err() {
                                evidence.tool_errors += 1;
                            }
                            if result.is_ok()
                                && crate::app::WRITING_TOOLS
                                    .contains(&requested_call.function.name.as_str())
                            {
                                mutated = true;
                            }
                            result
                        }
                    };
                    let content = match &result {
                        Ok(output) => output.clone(),
                        Err(error) => format!("Error: {error}"),
                    };
                    let images = match &result {
                        Ok(output)
                            if matches!(
                                requested_call.function.name.as_str(),
                                "read_image" | "computer_use"
                            ) =>
                        {
                            crate::tools::scoped_workspace(&workspace, || {
                                crate::app::tool_result_attachment(&requested_call, output)
                            })
                            .into_iter()
                            .collect()
                        }
                        _ => Vec::new(),
                    };
                    let mut message = Message::tool_result(requested_call.id.clone(), content);
                    message.images = images;
                    messages.push(message);
                }
                // The write watchdog: a run that keeps reading without ever
                // changing a file is stuck in reconnaissance. Ask it once to
                // land the first artifact; the next round picks the message
                // up with its tool results. A task that is read-only by
                // nature may ignore this and continue.
                if mutated {
                    quiet_rounds = 0;
                } else {
                    quiet_rounds += 1;
                }
                if !write_nudged && quiet_rounds >= crate::app::ZERO_WRITE_NUDGE_ROUNDS {
                    write_nudged = true;
                    messages.push(Message::new(
                        Role::User,
                        format!(
                            "You have spent {quiet_rounds} tool rounds without writing or \
                             changing any file. If this task is meant to land work, stop \
                             exploring: produce the first artifact now — write the file, make \
                             the edit, run the command, commit. If this task is read-only by \
                             nature, ignore this and continue."
                        ),
                    ));
                    let _ = delegate_sender.send(crate::app::DelegateUpdate::Note {
                        run,
                        text: format!(
                            "{quiet_rounds} tool rounds with no file writes — nudging it to \
                             land an artifact"
                        ),
                    });
                }
            }
            crate::tools::unregister_subagent_run(run);
            // The report: the run's final answer, or the best account of what
            // it did. A model that worked but never wrote a final answer (tool
            // rounds only, or everything in reasoning) still reports something
            // useful instead of "no content".
            let fallback_text = messages
                .iter()
                .rev()
                .find(|message| {
                    message.role == Role::Assistant && !message.content.trim().is_empty()
                })
                .map(|message| message.content.trim().to_owned())
                .filter(|text| !text.is_empty())
                .or_else(|| {
                    let reasoning = reasoning.trim();
                    (!reasoning.is_empty()).then(|| {
                        let chars: Vec<char> = reasoning.chars().collect();
                        let start = chars.len().saturating_sub(1500);
                        let tail: String = chars[start..].iter().collect();
                        if start > 0 {
                            format!("…{tail}")
                        } else {
                            tail
                        }
                    })
                });
            // A failure reads as an error so the model can retry or report.
            // The run submitted: from here on it is checked, not trusted.
            let _ = delegate_sender.send(crate::app::DelegateUpdate::State {
                run,
                state: crate::delegation::RunState::Submitted,
            });
            // The verify command runs here, in the run's own workspace, so
            // the coordinator gets a result it did not have to ask for.
            let mut verify = None;
            let mut verify_skipped = None;
            match &spec.verify_command {
                Some(command) => {
                    let args = serde_json::json!({ "command": command }).to_string();
                    let call = ToolCall::new("verify", "run_command", args);
                    let root = workspace.clone();
                    let joined =
                        tokio::task::spawn_blocking(move || tools::execute(&call, None, &root))
                            .await;
                    let result = match joined {
                        Ok(Ok(output)) => Ok(output),
                        Ok(Err(error)) => Err(format!("{error:#}")),
                        Err(join) => Err(format!("tool task failed: {join}")),
                    };
                    verify = Some(crate::delegation::CommandRecord::from_result(
                        command, &result,
                    ));
                }
                None => {
                    verify_skipped = Some(
                        "no verify_command was given, so nothing beyond the evidence was run"
                            .to_owned(),
                    );
                }
            }
            let answered = if answer.trim().is_empty() {
                fallback_text.unwrap_or_default()
            } else {
                answer.clone()
            };
            let outcome = crate::delegation::RunOutcome {
                evidence: evidence.clone(),
                answer: answered,
                cancelled: cancelled.load(std::sync::atomic::Ordering::Relaxed),
                timed_out: timed_out.then_some(timeout),
                failure: if failed && !timed_out {
                    Some(if failure.is_empty() {
                        "the run failed without a message".to_owned()
                    } else {
                        failure
                    })
                } else {
                    None
                },
                verify,
                verify_skipped,
            };
            let verdict = crate::delegation::verdict(kind, &spec.expect_files, &outcome);
            let _ = delegate_sender.send(crate::app::DelegateUpdate::Verdict {
                run,
                state: verdict.state,
                reasons: verdict.reasons.clone(),
                files: outcome.evidence.changed_paths(),
                report: crate::delegation::report(
                    run,
                    &spec.task,
                    kind,
                    &verdict,
                    &outcome,
                    started.elapsed(),
                    &others_active,
                ),
                session_id,
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{PASTE_LINE_GAP, is_pasted_line_break};
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use std::time::{Duration, Instant};

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn an_enter_right_after_another_key_is_a_pasted_line_break() {
        let now = Instant::now();
        let enter = key(KeyCode::Enter, KeyModifiers::NONE);
        // A paste: the characters arrived microseconds ago.
        assert!(is_pasted_line_break(
            &enter,
            Some(now - Duration::from_micros(200)),
            now
        ));
        // Typed by a human: the gap is wide, so Enter submits.
        assert!(!is_pasted_line_break(
            &enter,
            Some(now - PASTE_LINE_GAP * 4),
            now
        ));
        // The first key of a session has nothing before it.
        assert!(!is_pasted_line_break(&enter, None, now));
        // Shift+Enter and friends keep their own meaning.
        assert!(!is_pasted_line_break(
            &key(KeyCode::Enter, KeyModifiers::SHIFT),
            Some(now),
            now
        ));
        // Only key presses count; a release is not a line break.
        let mut release = enter;
        release.kind = KeyEventKind::Release;
        assert!(!is_pasted_line_break(&release, Some(now), now));
    }
}
