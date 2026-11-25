//! Done and needs-approval pings: the terminal bell, plus a Windows toast
//! when yopes runs on Windows.
//!
//! The point is to be reachable while you are doing something else: a turn
//! that ends, or a tool that stops to ask, should not be found by chance when
//! you look back at the window. Nothing here is on a timer and nothing blocks:
//! the toast runs on a thread of its own, and a machine without PowerShell
//! just gets the bell.

use std::io::Write;

use crate::app::App;

/// A bell is one byte on stdout; a terminal that ignores it ignores it.
fn bell() {
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x07");
    let _ = out.flush();
}

/// The Windows toast, fired and forgotten.
///
/// PowerShell carries a registered AppUserModelID, which is what Windows
/// requires before it will show a toast at all; the script never touches the
/// network and exits on its own.
#[cfg(windows)]
fn toast(title: &str, body: &str) {
    let script = format!(
        "$ErrorActionPreference='Stop';\
         [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null;\
         $xml = New-Object Windows.Data.Xml.Dom.XmlDocument;\
         $xml.LoadXml('<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>');\
         $toast = New-Object Windows.UI.Notifications.ToastNotification $xml;\
         [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier(\
         '{{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}}\\WindowsPowerShell\\v1.0\\powershell.exe').Show($toast);",
        xml_escape(title),
        xml_escape(body),
    );
    std::thread::spawn(move || {
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

/// Every other OS relies on the terminal's own notification plumbing.
#[cfg(not(windows))]
fn toast(_title: &str, _body: &str) {}

/// Toasts take text, not markup: the workspace name is user data.
fn xml_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// One ping: bell now, toast after.
fn ping(title: &str, body: &str) {
    #[cfg(test)]
    {
        // Tests never touch the terminal or spawn a shell: only the wording is
        // under test.
        let _ = (title, body);
    }
    #[cfg(not(test))]
    {
        bell();
        toast(title, body);
    }
}

/// The workspace name, for "what finished" in a notification that may be read
/// far from the window.
fn where_(app: &App) -> String {
    app.working_dir()
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| app.working_dir().display().to_string())
}

/// A turn finished: what it was for, and where.
pub fn done(app: &App) {
    let (title, body) = done_message(app);
    ping(&title, &body);
}

/// A tool stopped to ask: name the tool so the approval can be judged before
/// walking back to the terminal.
pub fn approval_needed(app: &App, tool: &str) {
    let (title, body) = approval_message(app, tool);
    ping(&title, &body);
}

/// The wording of the "turn finished" ping, pure so a test can check it.
pub fn done_message(app: &App) -> (String, String) {
    let folder = where_(app);
    let model = app.backend.model();
    let body = if model.trim().is_empty() {
        format!("Turn finished in {folder}")
    } else {
        format!("Turn finished in {folder} · {model}")
    };
    ("yopes · done".to_owned(), body)
}

/// The wording of the approval ping.
pub fn approval_message(app: &App, tool: &str) -> (String, String) {
    let folder = where_(app);
    (
        "yopes · approval needed".to_owned(),
        format!("{tool} is waiting for approval in {folder}"),
    )
}

#[cfg(test)]
mod tests {
    use super::{approval_message, done_message, xml_escape};

    /// The two pings name the workspace, the model and the tool, because a
    /// toast is read from outside the terminal.
    #[test]
    fn the_pings_say_what_happened_and_where() {
        let app = crate::app::test_app();
        let (title, body) = done_message(&app);
        assert_eq!(title, "yopes · done");
        assert!(body.starts_with("Turn finished in"), "{body}");
        assert!(body.contains("gpt-4o"), "{body}");

        let (title, body) = approval_message(&app, "run_command");
        assert_eq!(title, "yopes · approval needed");
        assert!(body.contains("run_command"), "{body}");
        assert!(body.contains("approval in"), "{body}");
    }

    #[test]
    fn toast_text_is_escaped() {
        assert_eq!(xml_escape("a & b <c>"), "a &amp; b &lt;c&gt;");
        assert_eq!(xml_escape("plain"), "plain");
    }
}
