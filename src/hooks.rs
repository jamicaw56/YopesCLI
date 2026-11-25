//! User hooks: shell commands run around tool calls and at the end of a turn,
//! configured in `~/.yopes/hooks.json`.
//!
//! ```json
//! {
//!   "hooks": {
//!     "PreToolUse":  [{ "match": "run_command", "command": "echo checking" }],
//!     "PostToolUse": [{ "match": "write_file", "command": "cargo fmt --quiet" }],
//!     "Stop":        [{ "command": "echo turn finished" }]
//!   }
//! }
//! ```
//!
//! `match` is one tool name, a `|`-separated list, or absent for every tool.
//! A `PreToolUse` command that exits non-zero blocks the call and hands its
//! output back as the tool's error. A `PostToolUse` command's output is
//! appended to the result. `Stop` runs when a turn chain finishes; its output
//! is not shown, which suits notifications and formatting.
//!
//! Hooks are read from the global file only: a repository must not be able to
//! run a command just by being cloned.

use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::session::ToolCall;

const FILE_NAME: &str = "hooks.json";

#[derive(Default, Deserialize)]
struct File {
    #[serde(default)]
    hooks: Events,
}

#[derive(Default, Deserialize)]
struct Events {
    #[serde(rename = "PreToolUse", default)]
    pre_tool_use: Vec<Hook>,
    #[serde(rename = "PostToolUse", default)]
    post_tool_use: Vec<Hook>,
    #[serde(rename = "Stop", default)]
    stop: Vec<Hook>,
}

#[derive(Deserialize)]
struct Hook {
    /// Tool names this hook fires for, `|`-separated; absent means all.
    #[serde(rename = "match", default)]
    tools: Option<String>,
    command: String,
}

impl Hook {
    fn applies_to(&self, tool: &str) -> bool {
        match self.tools.as_deref().map(str::trim) {
            None | Some("") => true,
            Some(tools) => tools
                .split('|')
                .any(|name| name.trim().eq_ignore_ascii_case(tool)),
        }
    }
}

/// The parsed hooks file. A missing file is no hooks; a malformed one is
/// reported once in the tool result that follows it, never as a silent skip.
fn load() -> (Events, Option<String>) {
    let path = crate::config::yopes_home().join(FILE_NAME);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (Events::default(), None);
        }
        Err(error) => {
            return (
                Events::default(),
                Some(format!("{}: {error}", path.display())),
            );
        }
    };
    match serde_json::from_str::<File>(&text) {
        Ok(file) => (file.hooks, None),
        Err(error) => (
            Events::default(),
            Some(format!("{}: {error}", path.display())),
        ),
    }
}

/// The `PreToolUse` output that blocks `call`, if any. The first failing hook
/// wins: its output is the reason the call did not run.
pub(crate) fn before_call(call: &ToolCall, workspace: &Path) -> Option<String> {
    let (events, problem) = load();
    if let Some(problem) = problem {
        // A broken file must not silently disable a hook the user relies on
        // to guard commands.
        return Some(format!("hooks: {problem}"));
    }
    run_before(&events, call, workspace)
}

/// `before_call` against an already-parsed file, so the matching rules can be
/// checked without writing into the user's home directory.
fn run_before(events: &Events, call: &ToolCall, workspace: &Path) -> Option<String> {
    for hook in events
        .pre_tool_use
        .iter()
        .filter(|hook| hook.applies_to(&call.function.name))
    {
        let command = subst(&hook.command, call);
        let (ok, output) = crate::tools::run_hook(&command, workspace);
        if !ok {
            return Some(if output.trim().is_empty() {
                format!("PreToolUse hook `{}` blocked this call.", hook.command)
            } else {
                output.trim().to_owned()
            });
        }
    }
    None
}

/// Append what the `PostToolUse` hooks printed to a successful result.
pub(crate) fn after_call(
    call: &ToolCall,
    result: anyhow::Result<String>,
    workspace: &Path,
) -> anyhow::Result<String> {
    let mut output = match result {
        Ok(output) => output,
        // A failed call reports its own error; a hook has nothing to add.
        Err(error) => return Err(error),
    };
    let (events, problem) = load();
    let hooks = events
        .post_tool_use
        .iter()
        .filter(|hook| hook.applies_to(&call.function.name));
    let mut notes = Vec::new();
    if let Some(problem) = problem {
        notes.push(format!("hooks: {problem}"));
    }
    for hook in hooks {
        let command = subst(&hook.command, call);
        let (ok, printed) = crate::tools::run_hook(&command, workspace);
        let printed = printed.trim();
        // Only a failure or a message is worth the model's attention: a hook
        // that succeeds quietly stays quiet.
        if !ok {
            notes.push(format!("PostToolUse `{}` failed: {printed}", hook.command));
        } else if !printed.is_empty() {
            notes.push(printed.to_owned());
        }
    }
    if !notes.is_empty() {
        output.push_str("\n\n");
        output.push_str(&notes.join("\n"));
    }
    Ok(output)
}

/// `{tool}` in a hook command is the tool's name, so one hook can serve many
/// tools without the shell seeing anything it has to parse.
fn subst(command: &str, call: &ToolCall) -> String {
    command.replace("{tool}", &call.function.name)
}

/// Run the `Stop` hooks for a finished turn, on their own thread: a hook that
/// waits on a build must not hold the interface. Output is discarded.
pub(crate) fn after_turn(workspace: &Path) {
    let (events, _) = load();
    if events.stop.is_empty() {
        return;
    }
    let workspace = workspace.to_path_buf();
    std::thread::spawn(move || {
        for hook in events.stop {
            let _ = crate::tools::run_hook(&hook.command, &workspace);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{Events, Hook, run_before, subst};
    use crate::session::ToolCall;

    fn events(json: &str) -> Events {
        serde_json::from_str(json).expect("hook config")
    }

    #[test]
    fn a_hook_matches_one_tool_a_list_or_every_tool() {
        let every = Hook {
            tools: None,
            command: "x".to_owned(),
        };
        assert!(every.applies_to("write_file"));
        let one = Hook {
            tools: Some("write_file".to_owned()),
            command: "x".to_owned(),
        };
        assert!(one.applies_to("write_file"));
        assert!(!one.applies_to("run_command"));
        let many = Hook {
            tools: Some("write_file | edit_file".to_owned()),
            command: "x".to_owned(),
        };
        assert!(many.applies_to("edit_file"));
        assert!(!many.applies_to("grep"));
        // An empty `match` is the same as none at all.
        let blank = Hook {
            tools: Some("  ".to_owned()),
            command: "x".to_owned(),
        };
        assert!(blank.applies_to("grep"));
    }

    #[test]
    fn the_tool_name_reaches_the_hook_command() {
        let call = ToolCall::new("c1", "write_file", "{}");
        assert_eq!(
            subst("echo {tool} {tool}x", &call),
            "echo write_file write_filex"
        );
    }

    #[test]
    fn a_failing_pre_tool_hook_blocks_the_call_and_a_passing_one_does_not() {
        let call = ToolCall::new("c1", "run_command", r#"{"command":"echo hi"}"#);
        let workspace = crate::temp::unique_dir("yopes-hooks-test");
        let blocking = events(r#"{"PreToolUse":[{"match":"run_command","command":"exit 3"}]}"#);
        assert!(run_before(&blocking, &call, &workspace).is_some());
        // The same hook does not fire for another tool.
        let other = ToolCall::new("c2", "grep", "{}");
        assert!(run_before(&blocking, &other, &workspace).is_none());
        let passing = events(r#"{"PreToolUse":[{"command":"exit 0"}]}"#);
        let blocked = run_before(&passing, &call, &workspace);
        assert!(blocked.is_none(), "exit 0 blocked: {blocked:?}");
        let _ = std::fs::remove_dir_all(workspace);
    }
}
