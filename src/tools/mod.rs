//! The tool surface offered to the model: definitions, approval rules, change
//! previews, and the dispatch that runs a call.
//!
//! `mod.rs` holds the contract shared by every tool: the limits, `execute`,
//! and the argument helpers. `definitions` carries the JSON schemas, `policy`
//! the approval rules and system prompt, and `preview` the change previews.
//! The tools themselves live in one module per family: `files` (read, write,
//! edit), `search` (glob, grep), `shell` (commands and background jobs), `web`
//! (search, fetch) and `todo`. `ignore` holds the `.yopesignore` filter the
//! filesystem walkers apply.

mod definitions;
mod files;
mod ignore;
mod policy;
mod preview;
mod search;
mod shell;
mod talk;
mod todo;
mod web;

use std::{
    cell::RefCell,
    env, fs,
    io::Read,
    path::{Path, PathBuf},
};

pub use definitions::definitions;
pub(crate) use files::{change_preview, edit_targets, move_target};
pub(crate) use ignore::Ignore;
pub use policy::{requires_approval, system_prompt};
pub use preview::{applied_change_all, command_text, preview, target_existed};
pub(crate) use shell::BashNotice;
pub(crate) use shell::kill_commands_for;
pub(crate) use shell::kill_spawned_commands;
pub(crate) use shell::poll_bash_jobs;

mod subagents;
pub(crate) use subagents::{register_subagent_run, unregister_subagent_run};

pub(crate) use talk::{TalkRequest, install as install_talk};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;

// The `json!` macro reaches the tool submodules through `use super::*`.
pub use serde_json::json;

use crate::{diff::FileDiff, session::ToolCall};

const MAX_LIST_ENTRIES: usize = 300;
const MAX_READ_BYTES: u64 = 4 * 1024 * 1024;
const MAX_READ_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_READ_LINES: usize = 400;
const MAX_READ_LINE_LIMIT: usize = 2_000;
const MAX_WRITE_BYTES: usize = 2 * 1024 * 1024;
const MAX_PREVIEW_BYTES: u64 = 1024 * 1024;
const MAX_OUTPUT_CHARS: usize = 16_000;
const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
const MAX_TIMEOUT_SECONDS: u64 = 600;

#[cfg(test)]
pub const TOOL_NAMES: [&str; 22] = [
    "list_dir",
    "read_file",
    "multi_read",
    "read_image",
    "glob",
    "grep",
    "write_file",
    "edit_file",
    "multi_edit",
    "move_to",
    "run_command",
    "bash_background",
    "bash_output",
    "message_subagent",
    "stop_subagent",
    "bash_kill",
    "web_search",
    "web_fetch",
    "todo_write",
    "delegate",
    "check_session",
    "talk_to",
];

#[derive(Debug)]
pub struct ChangePreview {
    pub diffs: Vec<FileDiff>,
    pub before: Option<String>,
    pub note: Option<String>,
}

thread_local! {
    /// The workspace root the current tool task resolves relative paths
    /// against. Every tab carries its own directory, so `execute` scopes it
    /// per task; without a scope the process working directory applies.
    static WORKSPACE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    /// The session id that owns the current tool task: `run_command` registers
    /// its process under it, so one tab's Esc interrupt kills only its own
    /// commands instead of every tab's.
    static OWNER: RefCell<Option<u64>> = const { RefCell::new(None) };
}

/// Run `body` with relative paths resolving against `root`. The previous
/// root is restored afterwards, so nested scopes and tests stay honest.
pub fn scoped_workspace<T>(root: &Path, body: impl FnOnce() -> T) -> T {
    WORKSPACE.with(|cell| {
        let previous = cell.borrow_mut().replace(root.to_path_buf());
        let result = body();
        *cell.borrow_mut() = previous;
        result
    })
}

/// Run `body` with spawned commands registered under `session_id`, so an
/// interrupt from that session can kill them without touching other tabs'.
pub fn scoped_owner<T>(session_id: u64, body: impl FnOnce() -> T) -> T {
    OWNER.with(|cell| {
        let previous = cell.borrow_mut().replace(session_id);
        let result = body();
        *cell.borrow_mut() = previous;
        result
    })
}

/// The session id the current tool task belongs to: `0` when no scope is
/// set, a bucket only process shutdown clears.
pub fn current_owner() -> u64 {
    OWNER.with(|cell| cell.borrow().unwrap_or(0))
}

/// Where relative tool paths resolve against right now: the scoped root when
/// one is set, the process working directory otherwise.
pub fn workspace() -> PathBuf {
    WORKSPACE
        .with(|cell| cell.borrow().clone())
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

pub fn execute(call: &ToolCall, web_key: Option<&str>, workspace: &Path) -> Result<String> {
    scoped_workspace(workspace, || match call.function.name.as_str() {
        "list_dir" => files::list_dir(&call.function.arguments),
        "read_file" => files::read_file(&call.function.arguments),
        "multi_read" => files::multi_read(&call.function.arguments),
        "read_image" => files::read_image(&call.function.arguments),
        "glob" => search::glob(&call.function.arguments),
        "grep" => search::grep(&call.function.arguments),
        "write_file" => files::write_file(&call.function.arguments),
        "edit_file" => files::edit_file(&call.function.arguments),
        "multi_edit" => files::multi_edit(&call.function.arguments),
        "move_to" => files::move_to(&call.function.arguments),
        "run_command" => shell::run_command(&call.function.arguments),
        "bash_background" => shell::bash_background(&call.function.arguments),
        "bash_output" => shell::bash_output(&call.function.arguments),
        "bash_kill" => shell::bash_kill(&call.function.arguments),
        "web_search" => web::web_search(&call.function.arguments, web_key),
        "web_fetch" => web::web_fetch(&call.function.arguments),
        "todo_write" => todo::todo_write(&call.function.arguments),
        // Control tools for running subagents: corrections steer a run
        // mid-flight, stop flags end it cooperatively.
        "message_subagent" => subagents::subagent_message(&call.function.arguments),
        "stop_subagent" => subagents::subagent_stop(&call.function.arguments),
        // Talking between sessions: the app loop owns every tab, so both
        // calls send a request there and answer with what happened.
        "check_session" => talk::check_session(),
        "talk_to" => talk::talk_to(&call.function.arguments),
        // Handled by the session loop: the subagent needs the backend.
        "delegate" => bail!("delegate cannot run outside the session loop"),
        other => bail!("unknown tool `{other}`"),
    })
}

/// The `task` argument of a `delegate` call, when it is a non-empty string.
pub fn delegate_task(call: &ToolCall) -> Option<String> {
    serde_json::from_str::<Value>(&call.function.arguments)
        .ok()
        .and_then(|arguments| {
            arguments
                .get("task")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .filter(|task| !task.trim().is_empty())
        })
}

/// The longest `task` a `delegate` call may carry: about 15k tokens, so one
/// runaway call cannot silently burn the user's whole context.
pub const MAX_DELEGATE_TASK_CHARS: usize = 60_000;

/// The checked `task` of a `delegate` call: present, non-empty, and within
/// the size cap. The error text is the tool result the model sees.
pub fn delegate_task_checked(call: &ToolCall) -> Result<String, String> {
    let Some(task) = delegate_task(call) else {
        return Err("delegate needs a task argument".to_owned());
    };
    let chars = task.chars().count();
    if chars > MAX_DELEGATE_TASK_CHARS {
        return Err(format!(
            "delegate task is too large ({chars} characters; the limit is \
             {MAX_DELEGATE_TASK_CHARS}). Trim it or split the work into several delegate calls."
        ));
    }
    Ok(task)
}

/// Execute a command directly for a user, in the tab's workspace.
pub fn run_user_command(command: &str, workspace: &Path) -> Result<String> {
    let args = serde_json::json!({
        "command": command,
    })
    .to_string();
    scoped_workspace(workspace, || shell::run_command(&args))
}

fn resolve(path: &str) -> PathBuf {
    let path = path.trim();
    let path = if path.is_empty() { "." } else { path };
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace().join(path)
    }
}

fn parse<T>(arguments: &str) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    if arguments.trim().is_empty() {
        serde_json::from_str("{}").context("tool arguments must be a JSON object")
    } else {
        serde_json::from_str(arguments)
            .with_context(|| format!("invalid tool arguments: {}", clip(arguments, 200)))
    }
}

fn clip(text: &str, width: usize) -> String {
    // Newlines collapse to spaces: a preview lands in the transcript as one
    // Line, and an embedded newline makes the row estimator count rows
    // ratatui never renders.
    let flat = text.replace("\r\n", " ").replace(['\n', '\r'], " ");
    let mut characters = flat.chars();
    let mut result = characters.by_ref().take(width).collect::<String>();
    if characters.next().is_some() {
        result.pop();
        result.push('…');
    }
    result
}

fn truncate_output(mut text: String) -> String {
    if text.chars().count() <= MAX_OUTPUT_CHARS {
        return text;
    }
    text = text.chars().take(MAX_OUTPUT_CHARS).collect();
    text.push_str("\n… output truncated");
    text
}

fn temporary_log_path() -> PathBuf {
    crate::temp::unique_path("yopes-tool", "log")
}

fn read_log(path: &Path) -> String {
    let Ok(file) = fs::File::open(path) else {
        return String::new();
    };
    let mut buffer = Vec::new();
    let _ = file
        .take((MAX_OUTPUT_CHARS * 4) as u64)
        .read_to_end(&mut buffer);
    String::from_utf8_lossy(&buffer).replace("\r\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::preview::{applied_change, preview};
    use super::{TOOL_NAMES, definitions, requires_approval};
    use crate::session::ToolCall;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn definitions_cover_every_tool() {
        let definitions = definitions();
        let names = definitions
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|definition| definition.pointer("/function/name"))
            .filter_map(|name| name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, TOOL_NAMES);
    }

    #[test]
    fn writes_never_ask_and_only_risky_commands_do() {
        let call = |name: &str, arguments: &str| ToolCall::new("call_1", name, arguments);
        assert!(!requires_approval(&call(
            "write_file",
            r#"{"path":"a.txt","content":"x"}"#
        )));
        assert!(!requires_approval(&call(
            "edit_file",
            r#"{"path":"a.txt","old_text":"x","new_text":"y"}"#
        )));
        assert!(!requires_approval(&call(
            "read_file",
            r#"{"path":"a.txt"}"#
        )));
        assert!(!requires_approval(&call(
            "run_command",
            r#"{"command":"cargo fmt --all"}"#
        )));
        assert!(!requires_approval(&call(
            "run_command",
            r#"{"command":"ls","risk":false}"#
        )));
        assert!(requires_approval(&call(
            "run_command",
            r#"{"command":"rm -rf build","risk":true}"#
        )));
        assert!(requires_approval(&call(
            "write_file",
            r#"{"path":"../../../../etc/passwd","content":"root"}"#
        )));
        assert!(requires_approval(&call(
            "edit_file",
            r#"{"path":"../../../../etc/shadow","old_text":"a","new_text":"b"}"#
        )));
    }

    #[test]
    fn write_diffs_are_new_only_when_the_file_did_not_exist_before() {
        let call = ToolCall::new(
            "call_1",
            "write_file",
            r#"{"path":"new.txt","content":"hello"}"#,
        );
        assert!(
            applied_change(&call, Some(String::new()), false)
                .unwrap()
                .created
        );
        assert!(
            !applied_change(&call, Some("old".to_owned()), true)
                .unwrap()
                .created
        );
    }

    #[test]
    fn multi_edit_applies_or_refuses_whole_batches() {
        use super::execute;
        use std::fs;
        let dir = std::env::temp_dir().join(format!("yopes-multi-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.txt"), "alpha\nbeta\nalpha\n").unwrap();
        fs::write(dir.join("b.txt"), "one\n").unwrap();
        let a = dir.join("a.txt").display().to_string();
        let b = dir.join("b.txt").display().to_string();
        let batch = json!({
            "edits": [
                { "path": a, "old_text": "alpha", "new_text": "A", "replace_all": true },
                { "path": a, "old_text": "beta", "new_text": "B", "replace_all": true },
                { "path": b, "old_text": "one", "new_text": "uno" },
            ]
        });
        let output = execute(
            &ToolCall::new("call_20", "multi_edit", batch.to_string()),
            None,
            Path::new("."),
        )
        .unwrap();
        assert!(
            output.contains("Applied 3 edits across 2 files"),
            "{output}"
        );
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "A\nB\nA\n");
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "uno\n");

        // A bad edit refuses the whole batch and leaves every file untouched.
        let broken = json!({
            "edits": [
                { "path": a, "old_text": "A", "new_text": "X", "replace_all": true },
                { "path": b, "old_text": "absent", "new_text": "?" },
            ]
        });
        let error = execute(
            &ToolCall::new("call_21", "multi_edit", broken.to_string()),
            None,
            Path::new("."),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("was not found"), "{error}");
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "A\nB\nA\n");
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "uno\n");
        assert!(!requires_approval(&ToolCall::new(
            "call_22",
            "multi_edit",
            r#"{"edits":[{"path":"a.txt","old_text":"x","new_text":"y"}]}"#
        )));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_file_matches_lf_text_against_crlf_files() {
        use super::execute;
        use std::fs;
        let dir = std::env::temp_dir().join(format!("yopes-crlf-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.txt");
        fs::write(&path, "alpha = 1\r\nbeta = 2\r\nalpha = 1\r\n").unwrap();
        let target = path.display().to_string();

        // An LF-terminated needle matches CRLF text; the inserted lines adopt
        // the file's endings and untouched lines keep their exact bytes.
        let single = json!({
            "path": target,
            "old_text": "beta = 2\n",
            "new_text": "beta = 20\n",
        });
        let output = execute(
            &ToolCall::new("call_23", "edit_file", single.to_string()),
            None,
            Path::new("."),
        )
        .unwrap();
        assert!(output.contains("Replaced 1 occurrence"), "{output}");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "alpha = 1\r\nbeta = 20\r\nalpha = 1\r\n"
        );

        // replace_all converts every inserted line the same way.
        let all = json!({
            "path": target,
            "old_text": "alpha = 1\n",
            "new_text": "gamma = 3\n",
            "replace_all": true,
        });
        execute(
            &ToolCall::new("call_24", "edit_file", all.to_string()),
            None,
            Path::new("."),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "gamma = 3\r\nbeta = 20\r\ngamma = 3\r\n"
        );

        let leading_lf = json!({
            "path": target,
            "old_text": "\nbeta = 20\n",
            "new_text": "\nbeta = 200\n",
        });
        execute(
            &ToolCall::new("call_25", "edit_file", leading_lf.to_string()),
            None,
            Path::new("."),
        )
        .unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "gamma = 3\r\nbeta = 200\r\ngamma = 3\r\n");
        assert!(!content.contains("\r\r\n"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn multi_edit_preview_reports_every_file_nothing_written() {
        use super::{change_preview, execute};
        use std::fs;
        let dir = std::env::temp_dir().join(format!("yopes-multi-preview-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("n.txt"), "aa\nbb\n").unwrap();
        fs::write(dir.join("m.txt"), "cc\n").unwrap();
        let args = json!({
            "edits": [
                { "path": dir.join("n.txt").display().to_string(), "old_text": "aa", "new_text": "xx" },
                { "path": dir.join("m.txt").display().to_string(), "old_text": "cc", "new_text": "dd" },
            ]
        });
        let call = ToolCall::new("call_23", "multi_edit", args.to_string());
        let change = change_preview(&call);
        assert_eq!(change.diffs.len(), 2);
        assert!(change.before.is_none());
        // Only the preview ran; the files are still as they were.
        assert_eq!(fs::read_to_string(dir.join("n.txt")).unwrap(), "aa\nbb\n");
        assert_eq!(fs::read_to_string(dir.join("m.txt")).unwrap(), "cc\n");
        assert!(execute(&call, None, Path::new(".")).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bash_background_output_and_kill_handle_the_lifecycle() {
        use super::execute;
        use super::shell::bash_path;
        use std::{thread, time::Duration};
        if bash_path().is_none() {
            // No bash on this machine yet; the tools refuse gracefully.
            let error = execute(
                &ToolCall::new("call_40", "bash_background", r#"{"command":"echo hi"}"#),
                None,
                Path::new("."),
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains("bash is not available"), "{error}");
            return;
        }
        let job_id = |started: &str| {
            started
                .split("bash job ")
                .nth(1)
                .and_then(|part| part.split_whitespace().next())
                .and_then(|part| part.parse::<u32>().ok())
                .unwrap()
        };
        let started = execute(
            &ToolCall::new(
                "call_41",
                "bash_background",
                r#"{"command":"printf step-one","timeout_seconds":30}"#,
            ),
            None,
            Path::new("."),
        )
        .unwrap();
        let id = job_id(&started);
        let arguments = format!(r#"{{"job_id":{id}}}"#);
        let mut output = String::new();
        // A slow or loaded machine spawns `bash -lc` late: the poll window
        // is generous (30s) so a scanning antivirus or parallel test load
        // cannot flake a job that only prints one line.
        for _ in 0..300 {
            output = execute(
                &ToolCall::new("call_42", "bash_output", &arguments),
                None,
                Path::new("."),
            )
            .unwrap();
            if output.contains("Exit code") {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        assert!(output.contains("step-one"), "{output}");
        assert!(output.contains("Exit code"), "{output}");

        // An unknown id refuses with the same phrasing everywhere.
        let error = execute(
            &ToolCall::new("call_43", "bash_output", r#"{"job_id":999999}"#),
            None,
            Path::new("."),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("no bash job"), "{error}");

        // Killing a sleeper ends it immediately with no exit code.
        let sleeper = execute(
            &ToolCall::new(
                "call_44",
                "bash_background",
                r#"{"command":"sleep 60","timeout_seconds":60}"#,
            ),
            None,
            Path::new("."),
        )
        .unwrap();
        let id = job_id(&sleeper);
        let arguments = format!(r#"{{"job_id":{id}}}"#);
        let killed = execute(
            &ToolCall::new("call_45", "bash_kill", &arguments),
            None,
            Path::new("."),
        )
        .unwrap();
        assert!(killed.contains("Killed"), "{killed}");
        thread::sleep(Duration::from_millis(50));
        let after = execute(
            &ToolCall::new("call_46", "bash_output", &arguments),
            None,
            Path::new("."),
        )
        .unwrap();
        assert!(after.contains("Exit code"), "{after}");
    }

    #[test]
    fn web_search_needs_a_key_and_previews_the_query() {
        use super::execute;
        let search = ToolCall::new("call_3", "web_search", r#"{"query":"rust 2026 release"}"#);
        assert_eq!(preview(&search), "rust 2026 release");

        let error = execute(&search, None, Path::new("."))
            .unwrap_err()
            .to_string();
        assert!(error.contains("TAVILY_API_KEY"));
        let empty = ToolCall::new("call_4", "web_search", r#"{"query":"  "}"#);
        let error = execute(&empty, Some("tvly-test"), Path::new("."))
            .unwrap_err()
            .to_string();
        assert!(error.contains("query"));
    }

    #[test]
    fn web_fetch_guards_urls_and_strips_the_page_noise() {
        use super::execute;
        use super::web::strip_html;
        let empty = ToolCall::new("call_30", "web_fetch", r#"{"url":"  "}"#);
        let error = execute(&empty, None, Path::new("."))
            .unwrap_err()
            .to_string();
        assert!(error.contains("url"), "{error}");
        let scheme = ToolCall::new("call_31", "web_fetch", r#"{"url":"ftp://example.test"}"#);
        let error = execute(&scheme, None, Path::new("."))
            .unwrap_err()
            .to_string();
        assert!(error.contains("http"), "{error}");
        let local = ToolCall::new("call_32", "web_fetch", r#"{"url":"http://127.0.0.1:8080"}"#);
        let error = execute(&local, None, Path::new("."))
            .unwrap_err()
            .to_string();
        assert!(error.contains("blocked"), "{error}");
        let meta = ToolCall::new(
            "call_33",
            "web_fetch",
            r#"{"url":"http://169.254.169.254"}"#,
        );
        let error = execute(&meta, None, Path::new("."))
            .unwrap_err()
            .to_string();
        assert!(error.contains("blocked"), "{error}");

        assert_eq!(
            strip_html(
                "<html><head><script>noise()</script><style>p { color: red; }</style>\
                 </head><body>Hi &amp; <b>bye</b></body></html>"
            ),
            "Hi & bye"
        );
        // An unterminated block swallows what follows, and look-alike tags
        // such as <scripts> are kept as plain text.
        assert_eq!(strip_html("<p>keep</p><SCRIPT>drop"), "keep");
        assert_eq!(strip_html("<scripts>x</scripts>"), "x");
    }

    #[test]
    fn glob_segments_match_one_directory_level_only() {
        use super::search::{glob_matches, glob_segment};
        let segment = |pattern: &str, text: &str| {
            glob_segment(
                &pattern.chars().collect::<Vec<_>>(),
                &text.chars().collect::<Vec<_>>(),
            )
        };
        assert!(segment("*", "anything"));
        assert!(segment("*.rs", "main.rs"));
        assert!(!segment("*.rs", "sub/main.rs"));
        assert!(segment("?at", "cat"));
        assert!(!segment("?at", "chat"));
        assert!(segment("a*b", "ab"));
        assert!(segment("a*b", "acb"));
        assert!(!segment("a*b", "aXc"));
        assert!(segment("*.RS", "main.rs"), "matching is case-insensitive");

        let path = |pattern: &[&str], text: &[&str]| {
            glob_matches(
                &pattern
                    .iter()
                    .map(|segment| (*segment).to_owned())
                    .collect::<Vec<_>>(),
                &text
                    .iter()
                    .map(|segment| (*segment).to_owned())
                    .collect::<Vec<_>>(),
            )
        };
        assert!(path(&["**"], &[]));
        assert!(path(&["**"], &["a", "b"]));
        assert!(path(&["a", "**", "c"], &["a", "b", "c"]));
        assert!(path(&["src", "*.rs"], &["src", "main.rs"]));
        assert!(!path(&["src", "*.rs"], &["src", "nested", "main.rs"]));
        assert!(path(&["src", "**", "*.rs"], &["src", "nested", "main.rs"]));
    }

    #[test]
    fn globs_skip_dependency_directories() {
        use super::execute;
        let dir = std::env::temp_dir().join(format!("yopes-glob-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub").join("deep")).unwrap();
        std::fs::write(dir.join("README.md"), "# x").unwrap();
        std::fs::write(dir.join("sub").join("note.md"), "x").unwrap();
        std::fs::write(dir.join("sub").join("deep").join("last.md"), "x").unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("target").join("stale.md"), "x").unwrap();

        let args = |pattern: &str| {
            format!(
                r#"{{"pattern":{:?},"path":{:?}}}"#,
                pattern,
                dir.display().to_string()
            )
        };
        let all = ToolCall::new("call_10", "glob", args("**/*.md"));
        let output = execute(&all, None, Path::new(".")).unwrap();
        assert!(output.contains("README.md"), "{output}");
        assert!(output.contains("sub/note.md"), "{output}");
        assert!(output.contains("sub/deep/last.md"), "{output}");
        assert!(!output.contains("target"), "{output}");

        let shallow = ToolCall::new("call_11", "glob", args("*.md"));
        let output = execute(&shallow, None, Path::new(".")).unwrap();
        assert!(output.contains("README.md"));
        assert!(!output.contains("note.md"), "{output}");

        let missing = ToolCall::new("call_12", "glob", args("*.toml"));
        let output = execute(&missing, None, Path::new(".")).unwrap();
        assert!(output.contains("(no matches)"), "{output}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn yopesignore_filters_globs_and_listings() {
        use super::execute;
        use std::fs;
        let dir = std::env::temp_dir().join(format!("yopes-ignore-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("dist").join("nested")).unwrap();
        fs::write(dir.join("src.rs"), "x").unwrap();
        fs::write(dir.join("debug.log"), "x").unwrap();
        fs::write(dir.join("keep.log"), "x").unwrap();
        fs::write(dir.join("dist").join("bundle.js"), "x").unwrap();
        fs::write(dir.join("dist").join("nested").join("chunk.js"), "x").unwrap();
        fs::write(dir.join(".yopesignore"), "dist/\n*.log\n!keep.log\n").unwrap();

        let glob = |pattern: &str| {
            let arguments = format!(r#"{{"pattern":{pattern:?},"path":"."}}"#);
            execute(&ToolCall::new("call_20", "glob", arguments), None, &dir).unwrap()
        };
        let every = glob("**/*");
        assert!(every.contains("src.rs"), "{every}");
        assert!(every.contains("keep.log"), "{every}");
        assert!(!every.contains("debug.log"), "{every}");
        assert!(!every.contains("dist"), "{every}");

        let listing = execute(
            &ToolCall::new("call_21", "list_dir", r#"{"path":"."}"#),
            None,
            &dir,
        )
        .unwrap();
        assert!(listing.contains("src.rs"), "{listing}");
        assert!(!listing.contains("debug.log"), "{listing}");
        assert!(!listing.contains("dist/"), "{listing}");

        let search = execute(
            &ToolCall::new(
                "call_22",
                "grep",
                r#"{"pattern":"x","path":".","include":"*.log"}"#,
            ),
            None,
            &dir,
        )
        .unwrap();
        assert!(search.contains("keep.log:1:"), "{search}");
        assert!(!search.contains("debug.log"), "{search}");

        // Without the file nothing is filtered again.
        fs::remove_file(dir.join(".yopesignore")).unwrap();
        let every = glob("**/*");
        assert!(every.contains("debug.log"), "{every}");
        assert!(every.contains("dist"), "{every}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn grep_matches_lines_and_respects_includes() {
        use super::execute;
        use std::fs;
        let dir = std::env::temp_dir().join(format!("yopes-grep-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("top.txt"), "needle here\nother\nneedles again").unwrap();
        fs::write(dir.join("sub").join("code.rs"), "let needle = 1;").unwrap();
        // A binary file that mentions the pattern must stay silent.
        fs::write(dir.join("skip.bin"), [0u8, b'n', b'e', b'e', b'd']).unwrap();

        let args = |more: &str| {
            format!(
                r#"{{"pattern":"needle","path":{:?}{more}}}"#,
                dir.display().to_string()
            )
        };
        let every = execute(
            &ToolCall::new("call_13", "grep", args("")),
            None,
            Path::new("."),
        )
        .unwrap();
        assert!(every.contains("top.txt:1:"), "{every}");
        assert!(every.contains("top.txt:3:"), "{every}");
        assert!(every.contains("code.rs:1:"), "{every}");
        assert!(!every.contains("skip.bin"), "{every}");

        let only_rust = execute(
            &ToolCall::new("call_14", "grep", args(r#","include":"*.rs""#)),
            None,
            Path::new("."),
        )
        .unwrap();
        assert!(only_rust.contains("code.rs:1:"));
        assert!(!only_rust.contains("top.txt"), "{only_rust}");

        let insensitive = execute(
            &ToolCall::new("call_15", "grep", args(r#","ignore_case":true"#)),
            None,
            Path::new("."),
        )
        .unwrap();
        assert!(insensitive.contains("top.txt:1:"), "{insensitive}");

        let absent_args = format!(
            r#"{{"pattern":"absent-nowhere","path":{:?}}}"#,
            dir.display().to_string()
        );
        let absent = execute(
            &ToolCall::new("call_16", "grep", absent_args),
            None,
            Path::new("."),
        )
        .unwrap();
        assert!(absent.contains("(no matches)"), "{absent}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_file_lists_a_directory_instead_of_failing() {
        use super::execute;
        use std::fs;
        let dir = std::env::temp_dir().join(format!("yopes-read-dir-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("nested")).unwrap();
        fs::write(dir.join("note.txt"), "hi").unwrap();
        let call = ToolCall::new(
            "call_7",
            "read_file",
            format!(r#"{{"path":{:?}}}"#, dir.display().to_string()),
        );
        let output = execute(&call, None, Path::new(".")).unwrap();
        assert!(output.contains("note.txt"), "{output}");
        assert!(output.contains("nested/"), "{output}");
        // Dot also works, and line ranges stay file-only.
        let listing = ToolCall::new("call_8", "read_file", r#"{"path":"."}"#);
        assert!(execute(&listing, None, Path::new(".")).is_ok());
        let ranged = ToolCall::new("call_9", "read_file", r#"{"path":".","start_line":2}"#);
        let error = execute(&ranged, None, Path::new("."))
            .unwrap_err()
            .to_string();
        assert!(error.contains("directory"), "{error}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn multi_read_reads_files_and_expands_folders() {
        use super::execute;
        use std::fs;
        let dir = std::env::temp_dir().join(format!("yopes-multiread-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.txt"), "alpha").unwrap();
        fs::write(dir.join("b.txt"), "beta").unwrap();
        // A file that is not valid UTF-8 is reported per file, not fatal.
        fs::write(dir.join("skip.bin"), [0u8, 159, 146, 150]).unwrap();

        let both = ToolCall::new(
            "call_21",
            "multi_read",
            format!(
                r#"{{"paths":[{:?},{:?}]}}"#,
                dir.join("a.txt").display().to_string(),
                dir.join("b.txt").display().to_string()
            ),
        );
        let output = execute(&both, None, Path::new(".")).unwrap();
        assert!(output.contains("alpha"), "{output}");
        assert!(output.contains("beta"), "{output}");

        let folder = ToolCall::new(
            "call_22",
            "multi_read",
            format!(
                r#"{{"paths":[{:?}],"all":true}}"#,
                dir.display().to_string()
            ),
        );
        let all = execute(&folder, None, Path::new(".")).unwrap();
        assert!(all.contains("alpha"), "{all}");
        assert!(all.contains("beta"), "{all}");
        assert!(all.contains("skip.bin"), "{all}");

        let empty = ToolCall::new("call_23", "multi_read", r#"{"paths":[]}"#);
        assert!(execute(&empty, None, Path::new(".")).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn multi_read_caps_output_while_accumulating() {
        use super::execute;
        use std::fs;
        let dir = std::env::temp_dir().join(format!("yopes-multiread-cap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // The first file alone exhausts the budget; the second must never be
        // read into memory.
        fs::write(dir.join("big.txt"), "x".repeat(20_000)).unwrap();
        fs::write(dir.join("small.txt"), "small-marker").unwrap();

        let folder = ToolCall::new(
            "call_24",
            "multi_read",
            format!(
                r#"{{"paths":[{:?}],"all":true}}"#,
                dir.display().to_string()
            ),
        );
        let all = execute(&folder, None, Path::new(".")).unwrap();
        assert!(all.contains("output truncated"), "{all}");
        assert!(!all.contains("small-marker"), "{all}");
        assert!(all.chars().count() <= 16_000 + 20, "{all}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn todo_write_allows_an_empty_list_to_clear_and_previews_the_count() {
        use super::execute;
        let empty = ToolCall::new("call_5", "todo_write", r#"{"items":[]}"#);
        assert_eq!(execute(&empty, None, Path::new(".")).unwrap(), "[]");

        let call = ToolCall::new(
            "call_6",
            "todo_write",
            r#"{"items":[{"text":"a"},{"text":"b","done":true}]}"#,
        );
        assert_eq!(preview(&call), "2 items");
        assert!(!requires_approval(&call));
    }

    #[test]
    fn previews_name_the_target_without_dumping_content() {
        let write = ToolCall::new(
            "call_1",
            "write_file",
            r#"{"path":"test.html","content":"<h1>hi</h1>"}"#,
        );
        assert_eq!(preview(&write), "test.html · 11 bytes");

        let command = ToolCall::new("call_2", "run_command", r#"{"command":"cargo fmt --all"}"#);
        assert_eq!(preview(&command), "cargo fmt --all");
    }

    #[test]
    fn move_to_reports_a_normalized_target_without_touching_the_process_cwd() {
        use super::execute;
        use super::move_target;
        use std::fs;
        let before = std::env::current_dir().unwrap();
        let dir = std::env::temp_dir().join(format!("yopes-moveto-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("inner/deeper")).unwrap();
        fs::write(dir.join("f.txt"), "x").unwrap();
        let workspace = dir.display().to_string();

        // A relative target resolves against the passed workspace, and `.`
        // and `..` collapse instead of leaving literal components behind.
        let normalized = ToolCall::new(
            "call_30",
            "move_to",
            json!({ "path": "inner/../inner/deeper" }).to_string(),
        );
        let output = execute(&normalized, None, Path::new(&workspace)).unwrap();
        assert!(output.contains("Working directory"), "{output}");
        assert_eq!(
            move_target(&normalized, Path::new(&workspace)),
            Some(dir.join("inner/deeper"))
        );

        // A file target fails.
        let file = dir.join("f.txt").display().to_string();
        let error = execute(
            &ToolCall::new("call_31", "move_to", json!({ "path": file }).to_string()),
            None,
            Path::new(&workspace),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("is not a directory"), "{error}");

        // A missing target fails the same way.
        let error = execute(
            &ToolCall::new(
                "call_32",
                "move_to",
                json!({ "path": "absent" }).to_string(),
            ),
            None,
            Path::new(&workspace),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("is not a directory"), "{error}");

        // The process working directory never moved: tabs stay isolated.
        assert_eq!(std::env::current_dir().unwrap(), before);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn move_to_within_the_workspace_runs_and_outside_asks() {
        let call =
            |path: &str| ToolCall::new("call_31", "move_to", json!({ "path": path }).to_string());
        // A subdirectory of the workspace runs automatically.
        assert!(!requires_approval(&call("src")));
        // The temp root runs automatically; anywhere else asks.
        let temp = crate::temp::platform_temp_root().display().to_string();
        assert!(!requires_approval(&call(&temp)));
        // Leaving the workspace entirely — `..` collapses past the root.
        assert!(requires_approval(&call("../../outside")));
        // A Windows absolute path is outside on Windows and a relative name
        // on everything else, so only Windows can assert it as a drive path.
        #[cfg(windows)]
        assert!(requires_approval(&call("C:\\Windows\\System32")));
    }
}
