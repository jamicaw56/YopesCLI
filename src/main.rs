mod app;
mod backend;
mod catalog;
mod clipboard;
mod config;
mod credentials;
mod diff;
mod logos;
mod session;
mod skills;
mod temp;
mod terminal;
mod theme;
mod tools;
mod ui;
mod update;

use std::{collections::HashMap, future::Future, pin::Pin, time::Instant};

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{Event, EventStream, MouseEventKind};
use futures_util::StreamExt;
#[cfg(unix)]
use tokio::signal::unix::{SignalKind, signal};
use tokio::{sync::mpsc, task::JoinHandle, time::Duration};

use clipboard::read_clipboard;
use terminal::{TerminalGuard, ctrl_c_interrupt};

use crate::{
    app::{Action, App, ToolEvent},
    backend::{Backend, StreamEvent},
    catalog::FetchResult,
    config::{Cli, Command, Config, load_session},
    session::{Message, Role, ToolCall},
};

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("yopes: {error:#}");
        std::process::exit(1);
    }
}

/// Every background task talks to the loop through one bundle of senders.
struct Channels {
    events: mpsc::UnboundedSender<StreamEvent>,
    tools: mpsc::UnboundedSender<ToolEvent>,
    delegates: mpsc::UnboundedSender<crate::app::DelegateUpdate>,
    user_commands: mpsc::UnboundedSender<(u64, Result<String, String>)>,
    catalog: mpsc::UnboundedSender<FetchResult>,
    models: mpsc::UnboundedSender<Result<Vec<(String, String)>, String>>,
    logos: mpsc::UnboundedSender<(String, Result<crate::logos::Logo, String>)>,
    paste: mpsc::UnboundedSender<Result<(String, String), String>>,
    titles: mpsc::UnboundedSender<(u64, String)>,
}

async fn run() -> Result<()> {
    terminal::install_panic_hook();
    let cli = Cli::parse();
    if matches!(cli.command, Some(Command::Update)) {
        return update::run().await;
    }
    let config = Config::from_cli(cli)?;
    // The theme is global state read by every draw call, so set it first.
    crate::theme::set(&config.theme);
    let session_path = config.session_path.clone();
    let sessions_dir = config.sessions_dir.clone();
    let config_path = config.config_path.clone();
    let session = load_session(session_path.as_deref())?;
    let backend = Backend::new(&config)?;
    let custom_providers = config.custom_providers.clone();
    let tavily_api_key = config.tavily_api_key.clone();
    let auto_compact = config.auto_compact;
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
    let mut terminal = TerminalGuard::new()?;
    let (sender, mut receiver) = mpsc::unbounded_channel::<StreamEvent>();
    let (tool_sender, mut tool_receiver) = mpsc::unbounded_channel::<ToolEvent>();
    let (delegate_sender, mut delegate_receiver) =
        mpsc::unbounded_channel::<crate::app::DelegateUpdate>();
    let (user_cmd_sender, mut user_cmd_receiver) =
        mpsc::unbounded_channel::<(u64, Result<String, String>)>();
    let (catalog_sender, mut catalog_receiver) = mpsc::unbounded_channel::<FetchResult>();
    let (models_sender, mut models_receiver) =
        mpsc::unbounded_channel::<Result<Vec<(String, String)>, String>>();
    let (logo_sender, mut logo_receiver) =
        mpsc::unbounded_channel::<(String, Result<crate::logos::Logo, String>)>();
    let (paste_sender, mut paste_receiver) =
        mpsc::unbounded_channel::<Result<(String, String), String>>();
    let (title_sender, mut title_receiver) = mpsc::unbounded_channel::<(u64, String)>();
    let channels = Channels {
        events: sender,
        tools: tool_sender,
        delegates: delegate_sender,
        user_commands: user_cmd_sender,
        catalog: catalog_sender,
        models: models_sender,
        logos: logo_sender,
        paste: paste_sender,
        titles: title_sender,
    };
    let mut terminal_events = EventStream::new();
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

    // Minimum gap between frames generated by streaming text.
    const STREAM_REDRAW_MILLIS: u128 = 50;
    let mut redraw_now = true;
    let mut redraw_streamed = false;
    let mut last_draw = Instant::now();

    loop {
        if app.should_quit {
            break;
        }
        // Redrawing re-lays-out the whole transcript, so stream text redraws
        // at a fixed cadence while user input still gets an immediate frame.
        if redraw_now
            || (redraw_streamed && last_draw.elapsed().as_millis() >= STREAM_REDRAW_MILLIS)
        {
            terminal
                .terminal
                .draw(|frame| ui::draw(frame, &mut app))
                .context("drawing terminal UI")?;
            redraw_now = false;
            redraw_streamed = false;
            last_draw = Instant::now();
        }

        tokio::select! {
            event = terminal_events.next() => {
                match event {
                    Some(Ok(Event::Key(key))) => {
                        let action = app.handle_key(key);
                        dispatch(
                            action,
                            &mut app,
                            &channels,
                            &mut active_tasks,
                        );
                        redraw_now = true;
                    }
                    Some(Ok(Event::Paste(value))) => {
                        // An empty paste usually means the clipboard holds an
                        // image: the terminal's own paste sends nothing for a
                        // screenshot, so the empty event is the only signal.
                        if value.trim().is_empty()
                            && app.approval.is_none()
                            && !app.connect_open()
                            && !app.show_help
                        {
                            dispatch(
                                Action::PasteImage,
                                &mut app,
                                &channels,
                                &mut active_tasks,
                            );
                        } else {
                            app.handle_paste(&value);
                        }
                        redraw_now = true;
                    }
                    Some(Ok(Event::Mouse(mouse))) => {
                        // A mouse move changes nothing on screen, so it must
                        // not cost a full transcript rebuild.
                        if mouse.kind != MouseEventKind::Moved {
                            let action = app.handle_mouse(mouse);
                            dispatch(
                                action,
                                &mut app,
                                &channels,
                                &mut active_tasks,
                            );
                            redraw_now = true;
                        }
                    }
                    Some(Ok(Event::FocusGained)) => {
                        ticker.reset();
                        app.set_focused(true);
                        redraw_now = true;
                    }
                    Some(Ok(Event::FocusLost)) => {
                        // While the terminal is in the background nothing is
                        // visible, so the ticker stops and no frame is drawn.
                        app.set_focused(false);
                    }
                    Some(Ok(Event::Resize(_, _))) => redraw_now = true,
                    Some(Err(error)) => return Err(error).context("reading terminal event"),
                    None => return Err(anyhow::anyhow!("terminal event stream closed")),
                }
            }
            event = receiver.recv() => {
                let Some(event) = event else { break; };
                let request_id = match &event {
                    StreamEvent::Delta { request_id, .. }
                    | StreamEvent::Reasoning { request_id, .. }
                    | StreamEvent::ToolProgress { request_id, .. }
                    | StreamEvent::Usage { request_id, .. }
                    | StreamEvent::ToolCalls { request_id, .. }
                    | StreamEvent::Retrying { request_id, .. }
                    | StreamEvent::Done { request_id }
                    | StreamEvent::Error { request_id, .. } => *request_id,
                };
                // Text deltas arrive in bursts and can be drawn at the fixed
                // cadence; lifecycle changes (usage, tool calls, completion)
                // need an immediate frame.
                let text_event = matches!(
                    &event,
                    StreamEvent::Delta { .. }
                        | StreamEvent::Reasoning { .. }
                        | StreamEvent::ToolProgress { .. }
                );
                let finished = matches!(event, StreamEvent::Done { .. } | StreamEvent::Error { .. });
                // Route to the tab that owns the request: a background tab
                // streams on while another is on screen, and its events must
                // land in its own session.
                let owner = app.tab_for_request(request_id);
                if finished {
                    active_tasks.remove(&request_id);
                }
                match owner {
                    Some(owner) => app.with_tab(owner, |app| {
                        let action = app.on_stream_event(event);
                        dispatch(
                            action,
                            app,
                            &channels,
                            &mut active_tasks,
                        );
                    }),
                    None => {
                        let action = app.on_stream_event(event);
                        dispatch(
                            action,
                            &mut app,
                            &channels,
                            &mut active_tasks,
                        );
                    }
                }
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
                match owner {
                    Some(owner) => app.with_tab(owner, |app| {
                        let action = app.on_tool_result(result);
                        dispatch(
                            action,
                            app,
                            &channels,
                            &mut active_tasks,
                        );
                    }),
                    None => {
                        let action = app.on_tool_result(result);
                        dispatch(
                            action,
                            &mut app,
                            &channels,
                            &mut active_tasks,
                        );
                    }
                }
                redraw_now = true;
            }
            result = catalog_receiver.recv() => {
                if let Some(result) = result {
                    app.on_catalog(result);
                }
                for provider_id in app.take_pending_logos() {
                    dispatch(
                        Action::FetchLogo(provider_id),
                        &mut app,
                        &channels,
                        &mut active_tasks,
                    );
                }
                redraw_now = true;
            }
            result = models_receiver.recv() => {
                if let Some(result) = result {
                    app.on_custom_models(result);
                }
                redraw_now = true;
            }
            result = logo_receiver.recv() => {
                let Some((provider_id, result)) = result else { break; };
                app.on_logo(provider_id, result);
                redraw_now = true;
            }
            result = paste_receiver.recv() => {
                let Some(result) = result else { break; };
                app.on_paste_image(result);
                redraw_now = true;
            }
            update = delegate_receiver.recv() => {
                let Some(update) = update else { break; };
                // Subagent deltas stream at the fixed cadence; a finished or
                // failed run needs an immediate frame.
                let finished = matches!(update, crate::app::DelegateUpdate::Finished { .. });
                app.on_delegate_update(update);
                if finished {
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
                match owner {
                    Some(owner) => app.with_tab(owner, |app| {
                        app.on_user_command_result(result);
                    }),
                    None => app.on_user_command_result(result),
                }
                redraw_now = true;
            }
            title = title_receiver.recv() => {
                let Some((session_id, title)) = title else { break; };
                // A background tab may have asked for the title: land it in
                // the session that asked, whichever tab is on screen.
                let owner = app.tab_for_session(session_id);
                match owner {
                    Some(owner) => app.with_tab(owner, |app| {
                        app.on_session_title(session_id, title);
                    }),
                    None => app.on_session_title(session_id, title),
                }
                redraw_now = true;
            }
            _ = &mut interrupt => break,
            _ = term_signal.as_mut() => break,
            _ = ticker.tick(),
            if app.focused
                && (app.any_streaming()
                    || app.connect_open()
                    || app.any_tools_busy()
                    || app.any_subagent_running()
                    || app.session.messages.is_empty()) =>
            {
                app.tick();
                redraw_now = true;
            }
        }
    }

    for (_, task) in active_tasks.drain() {
        task.abort();
    }
    drop(terminal);
    app.persist_all()
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
            let task = tokio::spawn(async move {
                // A panic in the request task must not leave the UI streaming a
                // response that will never arrive.
                let stream = std::panic::AssertUnwindSafe(backend.stream(
                    request_id,
                    messages,
                    tools_enabled,
                    mode_note,
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
            tokio::spawn(async move {
                // A long build must not hold the event loop: the command runs
                // on the blocking pool and the result lands in its session.
                let joined =
                    tokio::task::spawn_blocking(move || tools::run_user_command(&command)).await;
                let result = match joined {
                    Ok(Ok(output)) => Ok(output),
                    Ok(Err(error)) => Err(format!("{error:#}")),
                    Err(error) => Err(format!("command task failed: {error}")),
                };
                let _ = sender.send((session_id, result));
            });
        }
        Action::LoadCatalog => {
            let sender = channels.catalog.clone();
            let config_path = app.config_path().to_path_buf();
            tokio::spawn(async move {
                // The catalog is loaded once and kept next to the config; a
                // later /connect reuses it instead of downloading it again.
                if let Some(cached) = catalog::load_cached(&config_path) {
                    let _ = sender.send(Ok(cached));
                    return;
                }
                let mut result = catalog::fetch().await.map_err(|error| format!("{error:#}"));
                if let Ok(catalog) = &mut result {
                    // A fresh fetch must not stomp the context presets the
                    // user applied to models.
                    catalog::merge_saved_context_windows(&config_path, catalog);
                    let _ = catalog::save_cached(&config_path, catalog);
                }
                let _ = sender.send(result);
            });
        }
        Action::RefreshCatalog => {
            let sender = channels.catalog.clone();
            let config_path = app.config_path().to_path_buf();
            tokio::spawn(async move {
                let mut result = catalog::fetch().await.map_err(|error| format!("{error:#}"));
                if let Ok(catalog) = &mut result {
                    // Same as above: presets survive a refresh.
                    catalog::merge_saved_context_windows(&config_path, catalog);
                    let _ = catalog::save_cached(&config_path, catalog);
                }
                let _ = sender.send(result);
            });
        }
        Action::FetchCustomModels { url, token, auth } => {
            let sender = channels.models.clone();
            tokio::spawn(async move {
                let result = catalog::fetch_models(&url, auth, token.as_deref())
                    .await
                    .map_err(|error| format!("{error:#}"));
                let _ = sender.send(result);
            });
        }
        Action::FetchLogo(provider_id) => {
            let sender = channels.logos.clone();
            tokio::spawn(async move {
                let result = crate::logos::fetch(&provider_id)
                    .await
                    .map_err(|error| format!("{error:#}"));
                let _ = sender.send((provider_id, result));
            });
        }
        Action::NameSession { task } => {
            let backend = app.backend.clone();
            let sender = channels.titles.clone();
            let session_id = app.session.id;
            tokio::spawn(async move {
                let brief = "You name coding sessions. Answer with only a short title, \
                             at most five words, capitalized, no quotes, no final \
                             punctuation. You have no tools.";
                // Errors stay silent: the session simply keeps a plain title.
                if let Ok(raw) = backend.complete_once(brief, &task).await {
                    let _ = sender.send((session_id, raw));
                }
            });
        }
    }
}

fn cancel_task(active_tasks: &mut HashMap<u64, JoinHandle<()>>, request_id: u64) {
    if let Some(task) = active_tasks.remove(&request_id) {
        task.abort();
    }
}

/// Spawn every tool call of one action. A delegate becomes a subagent run on
/// the same provider: the response streams live through the delegate channel
/// and the final answer lands as the tool result. Anything else runs on the
/// blocking pool as before.
fn spawn_tool_calls(calls: Vec<ToolCall>, app: &mut App, channels: &Channels) {
    let backend = app.backend.clone();
    let delegates = channels.delegates.clone();
    let tools = channels.tools.clone();
    let web_key = app.tavily_key().map(str::to_owned);
    for call in calls {
        if call.function.name != "delegate" {
            let sender = tools.clone();
            let web_key = web_key.clone();
            tokio::spawn(async move {
                let task_call = call.clone();
                let joined = tokio::task::spawn_blocking(move || {
                    tools::execute(&task_call, web_key.as_deref())
                })
                .await;
                let result = match joined {
                    Ok(Ok(output)) => Ok(output),
                    Ok(Err(error)) => Err(format!("{error:#}")),
                    Err(error) => Err(format!("tool task failed: {error}")),
                };
                let _ = sender.send(ToolEvent { call, result });
            });
            continue;
        }
        // The subagent: one request, no tools, the task as the user message.
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
        let run = app.begin_subagent(task.clone());
        let sender = tools.clone();
        let delegate_sender = delegates.clone();
        let backend = backend.clone();
        tokio::spawn(async move {
            let (events, mut receiver) = mpsc::unbounded_channel::<StreamEvent>();
            let streamer = {
                let events = events.clone();
                let backend = backend.clone();
                tokio::spawn(async move {
                    backend
                        .stream(
                            u64::MAX,
                            vec![Message::new(Role::User, task)],
                            false,
                            Some(crate::app::SUBAGENT_BRIEF),
                            events,
                        )
                        .await;
                })
            };
            let mut answer = String::new();
            let mut failed = false;
            let mut failure = String::new();
            while let Some(event) = receiver.recv().await {
                match event {
                    StreamEvent::Delta { text, .. } => {
                        answer.push_str(&text);
                        let _ =
                            delegate_sender.send(crate::app::DelegateUpdate::Delta { run, text });
                    }
                    StreamEvent::Reasoning { text, .. } => {
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
                    StreamEvent::Error { message, .. } => {
                        failed = true;
                        failure = message;
                        break;
                    }
                    StreamEvent::Done { .. } => break,
                    _ => {}
                }
            }
            let _ = streamer.await;
            let _ = delegate_sender.send(crate::app::DelegateUpdate::Finished { run, failed });
            let result = if failed {
                Err(if failure.is_empty() {
                    "the subagent returned no content".to_owned()
                } else {
                    failure
                })
            } else if answer.trim().is_empty() {
                Err("the subagent returned no content".to_owned())
            } else {
                Ok(answer.trim().to_owned())
            };
            let _ = sender.send(ToolEvent { call, result });
        });
    }
}
