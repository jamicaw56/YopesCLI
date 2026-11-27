#![cfg(test)]
//! Mockup renders: the real `ui::draw` on a `TestBackend`, written to
//! `C:\Users\rosMa\Temp\yopes-mockups` so the new UI can be looked at without
//! running the app. Run with:
//!
//! ```text
//! YOPES_MOCKUPS=1 cargo test mockups -- --nocapture
//! ```
//!
//! Without the variable each test returns at once: these scenes are for a
//! human to read, not a CI assertion.

use crate::app::{App, TextSelection};
use crate::session::{Message, Role, ToolCall};
use ratatui::{Terminal, backend::TestBackend};

/// Where the captures land, created on demand.
fn out_dir() -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(r"C:\Users\rosMa\Temp\yopes-mockups");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// The frame as text: one line per screen row, trailing blanks trimmed.
fn capture(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|row| {
            (0..width)
                .map(|column| buffer[(column, row)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<String>>()
        .join("\n")
}

fn enabled() -> bool {
    std::env::var("YOPES_MOCKUPS").is_ok()
}

fn write(name: &str, body: &str) {
    let path = out_dir().join(format!("{name}.txt"));
    let _ = std::fs::write(&path, format!("{body}\n"));
    println!("\n===== {name} =====\n{body}\n");
    println!("written: {}", path.display());
}

fn tool_message(id: &str, name: &str, content: &str) -> (Message, Message) {
    let mut assistant = Message::new(Role::Assistant, "");
    assistant.tool_calls = Some(vec![ToolCall::new(id, name, "{}")]);
    let result = Message {
        role: Role::Tool,
        content: content.to_owned(),
        reasoning: String::new(),
        tool_calls: None,
        tool_call_id: Some(id.to_owned()),
        usage: None,
        images: Vec::new(),
        error: None,
    };
    (assistant, result)
}

/// A working session: a reply with thinking, a folded tool result, a diff, a
/// running tool and the live footer.
#[test]
fn mockup_a_working_session() {
    if !enabled() {
        return;
    }
    let mut app = crate::app::test_app();
    app.session
        .messages
        .push(Message::new(Role::User, "make the transcript selectable"));
    let mut thinking = Message::new(Role::Assistant, "");
    thinking.reasoning = "The terminal swallows the mouse, so the app has to paint\nits own selection and copy the cells it highlighted.".to_owned();
    app.session.messages.push(thinking);
    let (assistant, result) = tool_message(
        "call_read",
        "read_file",
        "src/ui/transcript.rs (1305 lines)\nlines 1-400 of 1305",
    );
    app.session.messages.push(assistant);
    app.session.messages.push(result);
    app.session.messages.push(Message::new(
        Role::Assistant,
        "The transcript now paints its own selection. Drag across a reply and \
         the highlighted cells go to the clipboard on release; a plain click \
         still opens a tool result.",
    ));
    app.session
        .messages
        .push(Message::new(Role::User, "now fix the retry countdown"));
    let (assistant, result) = tool_message(
        "call_shell",
        "run_command",
        "error: could not compile `yopes`\n  --> src/ui/connect.rs:257:13\n   |\n257|   .unwrap_or(\"tab next\")\n   |    ^^^^^^^^^ expected `&str`, found `String`",
    );
    app.session.messages.push(assistant);
    app.session.messages.push(result);
    app.session.messages.push(Message::new(
        Role::Assistant,
        "The footer counted the wait from its own copy of the delay, so the \
         number never moved. It now counts down from when the retry was \
         raised.",
    ));
    app.running_tool = Some(crate::app::RunningTool {
        call: ToolCall::new("call_1", "grep", "{}"),
        started: std::time::Instant::now(),
        before: None,
        existed: false,
        targets: Vec::new(),
    });
    app.input
        .set("and make the tool output a rail instead of a box".to_owned());
    app.spinner = 5;
    let body = capture(&mut app, 100, 34);
    write("01-working-session", &body);
}

/// The same conversation with a drag: the highlighted run and the copy.
#[test]
fn mockup_b_selection() {
    if !enabled() {
        return;
    }
    let mut app = crate::app::test_app();
    app.session
        .messages
        .push(Message::new(Role::User, "what changed in the transcript?"));
    app.session.messages.push(Message::new(
        Role::Assistant,
        "The transcript now paints its own selection.\nDrag across a reply and it copies on release.\nA plain click still opens a tool result.",
    ));
    // Draw once so the transcript publishes its area, then select two rows.
    let mut terminal = Terminal::new(TestBackend::new(88, 20)).unwrap();
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();
    let area = app.transcript_area.expect("area");
    app.text_selection = Some(TextSelection {
        anchor: (app.scroll + 4, 0),
        head: (app.scroll + 5, area.width.saturating_sub(1)),
    });
    let body = capture(&mut app, 88, 20);
    write("02-selection", &body);
    println!("copied selection: {:?}", app.selection_text);
}

/// The connect dialog as the docked box, on the provider stage.
#[test]
fn mockup_c_connect_box() {
    if !enabled() {
        return;
    }
    let mut app = crate::app::test_app();
    app.session
        .messages
        .push(Message::new(Role::User, "connect my own endpoint"));
    app.session.messages.push(Message::new(
        Role::Assistant,
        "Run /provider connect: the box opens above the composer and the \
         conversation stays visible while you pick a provider.",
    ));
    app.begin_connect();
    let body = capture(&mut app, 96, 28);
    write("03-connect-providers", &body);
}

/// The connect box on the three-field add-provider form.
#[test]
fn mockup_d_connect_add_provider() {
    if !enabled() {
        return;
    }
    let mut app = crate::app::test_app();
    app.begin_connect();
    if let Some(connect) = app.connect_state_mut() {
        connect.stage = crate::app::ConnectStage::AddProvider;
        connect.custom_name = "local-vllm".to_owned();
        connect.custom_endpoint = "http://127.0.0.1:8000/v1".to_owned();
        connect.custom_token = "sk-local".to_owned();
        connect.add_provider_field = crate::app::AddProviderField::CompletionUrl;
    }
    let body = capture(&mut app, 96, 28);
    write("04-connect-add-provider", &body);
}

/// A failed request counting its retry down.
#[test]
fn mockup_e_retry_footer() {
    if !enabled() {
        return;
    }
    let mut app = crate::app::test_app();
    app.session
        .messages
        .push(Message::new(Role::User, "summarize the diff"));
    app.retry = Some(crate::app::RetryNotice {
        attempt: 2,
        delay_secs: 9,
        reason: "provider returned 502: upstream temporarily unavailable".to_owned(),
        since: std::time::Instant::now() - std::time::Duration::from_secs(6),
    });
    let body = capture(&mut app, 96, 14);
    write("05-retry-footer", &body);
}
