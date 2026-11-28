//! Tool policy: which calls need the user's approval, and the system prompt
//! that tells the model how to use the tools.

use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use serde_json::Value;

use crate::{session::ToolCall, temp};

use super::workspace;

/// Whether a tool call needs the user's approval.
///
/// Workspace writes and edits within the project or temp directory run
/// automatically. Commands flagged as risky or any file modification
/// escaping the workspace requires user approval.
pub fn requires_approval(call: &ToolCall) -> bool {
    if call.function.name == "delegate" {
        // A subagent request spends extra tokens on the user's account.
        return true;
    }
    if crate::mcp::is_mcp(&call.function.name) {
        // A configured MCP server runs its own code, so each call asks.
        return true;
    }
    if call.function.name == "talk_to" {
        // A message to another tab can wake that tab's model into a whole
        // new turn, so it spends tokens on the user's account like a
        // delegate call does.
        return true;
    }
    if call.function.name == "computer_use" {
        // Mouse and keyboard input reaches everything the user could touch,
        // so this one asks by default. Only an explicit "always allow" (the
        // `a` key), yolo, or dangerous mode gets past it.
        return true;
    }
    if matches!(
        call.function.name.as_str(),
        "run_command" | "bash_background"
    ) && command_risk(&call.function.arguments)
    {
        return true;
    }
    is_external_modification(call)
}

fn is_external_modification(call: &ToolCall) -> bool {
    let args: Value = match serde_json::from_str(&call.function.arguments) {
        Ok(val) => val,
        Err(_) => return false,
    };
    match call.function.name.as_str() {
        "write_file" | "edit_file" => args
            .get("path")
            .and_then(Value::as_str)
            .is_some_and(|p| !is_within_allowed_roots(Path::new(p))),
        "multi_edit" => args
            .get("edits")
            .and_then(Value::as_array)
            .is_some_and(|edits| {
                edits.iter().any(|edit| {
                    edit.get("path")
                        .and_then(Value::as_str)
                        .is_some_and(|p| !is_within_allowed_roots(Path::new(p)))
                })
            }),
        "apply_patch" => super::patch::patch_paths(&call.function.arguments)
            .iter()
            .any(|p| !is_within_allowed_roots(Path::new(p))),
        // Moving the working directory outside the workspace or the temp root
        // re-roots every relative path the tools see, so it asks first.
        "move_to" => args
            .get("path")
            .and_then(Value::as_str)
            .is_some_and(|p| !is_within_allowed_roots(Path::new(p))),
        _ => false,
    }
}

fn is_within_allowed_roots(path: &Path) -> bool {
    // The scoped workspace: the tab's own directory, never the process one.
    let ws = normalize_path(&workspace());
    let temp = normalize_path(&temp::platform_temp_root());
    let path = if path.is_absolute() {
        normalize_path(path)
    } else {
        normalize_path(&ws.join(path))
    };
    is_subpath(&path, &ws) || is_subpath(&path, &temp)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    normalized
}

fn is_subpath(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        let p = path.to_string_lossy().to_ascii_lowercase();
        let r = root.to_string_lossy().to_ascii_lowercase();
        p == r || p.starts_with(&format!("{}\\", r)) || p.starts_with(&format!("{}/", r))
    }
    #[cfg(not(windows))]
    {
        path.starts_with(root)
    }
}

/// The `risk` flag the model set on a `run_command` or `bash_background` call.
pub fn command_risk(arguments: &str) -> bool {
    serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|arguments| arguments.get("risk").and_then(Value::as_bool))
        .unwrap_or(false)
}

/// Which system prompt a request gets.
///
/// `Chat` is plain conversation: no tool text and no project files, so the
/// model answers like a chat partner instead of an agent whose tools went
/// missing. `NoTools` is an agent turn with the tools withheld (tools mode
/// off, or a provider that refused them): the project instructions still ride
/// along, because that session keeps its files and its tool history.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Prompt {
    Chat,
    #[default]
    Tools,
    NoTools,
}

impl Prompt {
    /// Whether tool calls are offered to the model with this prompt.
    pub const fn tools(self) -> bool {
        matches!(self, Self::Tools)
    }

    pub const fn chat(self) -> bool {
        matches!(self, Self::Chat)
    }
}

pub fn system_prompt(prompt: Prompt, workspace: &Path) -> String {
    let cwd = workspace.display().to_string();
    let os = std::env::consts::OS;
    let temp = temp::platform_temp_root().display().to_string();
    let base = if prompt.chat() {
        "You are yopes, a chat companion in the user's terminal. This is plain \
         conversation: no tools are wired up for this session, so nothing can be read, \
         written, or run — answer from what you know and from the conversation, in prose. \
         Format replies with Markdown by default: ## headings for sections, bullets or \
         numbered lists for multiple items, **bold** for key terms, and fenced code \
         blocks with a language tag for code. Keep paragraphs readable; do not flatten \
         lists or code into plain prose. Keep responses short and concrete."
            .to_owned()
    } else if prompt.tools() {
        format!(
            "You are yopes, a coding assistant running in the user's terminal.\n\
             Working directory: {cwd}\n\
             Operating system: {os}\n\
             Scratch files (scripts, downloads, logs) go in {temp}: that is where the user looks for them.\n\
             Use the provided tools to inspect and change files and to run commands. \
             Prefer tools over guessing, read a file before editing it, and never claim \
             that a file changed unless a tool result confirms it. Set run_command risk to \
             true for anything destructive or hard to undo so the user is asked first. \
             run_command and bash_background take a `shell` argument — bash, powershell, \
             cmd or sh — when a command needs a specific interpreter; without it commands \
             run through git-bash on Windows (cmd when Git is missing) and sh elsewhere. \
             Use glob to find files by pattern and grep to search their contents, so you \
             do not read whole directories blindly. A `.yopesignore` at the workspace root \
             filters what those walks and list_dir see, so a path missing from results may \
             be excluded there; read it when something you expect is absent. \
             For several replacements in one or more files, batch them with multi_edit: \
             it applies every edit in order, and one failure refuses the whole batch. \
             For larger changes (new files, deletions, renames, many hunks) use apply_patch \
             with a `*** Begin Patch` envelope; it also applies all or nothing. \
             Change the working directory with move_to when a task lives in another folder: \
             relative paths resolve against it, and moving outside the workspace asks first. \
             To read several files in one call, batch them with multi_read: \
             with `all: true` it reads every file directly inside each folder in `paths`. \
             Use read_all to read every text file under a directory recursively. \
             Use web_search for anything newer than your knowledge or that needs current \
             facts, and cite the URL you used. \
             Use web_fetch to read the text of a specific page, such as one surfaced by \
             web_search. \
             Start long-running or detached work with bash_background; the app notifies \
             this conversation automatically when the job finishes, so never poll \
             bash_output in a loop — stop the job with bash_kill when it has served its purpose. \
             Maintain the session task checklist dynamically with todo_write: call it at the \
             start of any multi-step task to establish the plan, update it as items are \
             completed (marking them done), and send an empty list to close the checklist once \
             everything is done. Do NOT maintain plans solely in conversational text; \
             always keep the checklist synchronized via todo_write. When asked to run commands, \
             tests, builds, git commits, or shell tasks, always execute them directly using \
             run_command or bash_background. Never merely explain the command in text when \
             execution is requested. \
             For an independent subtask use delegate: hand the subagent one self-contained \
             task with the context it needs — it runs with the same tools you have in the \
             same workspace. A delegate call returns at once and the subagent works in the \
             background: keep working on other things and never wait for it; its final \
             answer is delivered to this conversation automatically when it finishes. \
             Several delegate calls in one turn run in parallel and stream in the sidebar; \
             the user reads every response with /agents. \
             Sessions can talk between themselves: `check_session` lists the open tabs with \
             their tab numbers, and `talk_to` delivers a message into another tab's \
             conversation — an idle tab wakes its model at once, a busy tab receives the \
             message when its turn ends. When a message like `[talk from tab N — Title] <text>` \
             arrives, it is another open session in the terminal talking to you. Read what \
             they ask and reply directly using `talk_to(tab: N, message: \"...\")`. Never ignore \
             it, and do not repeat in conversational text to the user instead of replying to tab N. \
             On Windows, computer_use drives the real desktop: it takes a screenshot, runs pointer \
             or keyboard actions, then attaches a second screenshot of the result, so aim by the \
             pixels you saw and describe what the new screenshot shows. The agent has its own \
             pointer: it glides from point to point with an eased animation, hovering by posting a \
             move to the window under the point; clicks, drags and the wheel inject real input and \
             then restore the user's pointer position, so the person at the keyboard keeps their \
             mouse. The agent cursor is a \
             visible click-through desktop arrow and is drawn into the capture, so do not \
             describe the system pointer as yours. Pass `window` (part of a title) to drive one \
             window in the background instead: input is posted to that window's handle and the \
             capture is of the window itself, so the system pointer does not move and nothing is \
             raised, which suits working in a window the user is watching. In that mode \
             coordinates are window client pixels and posted input may be ignored by apps that \
             draw themselves. Every call asks the user \
             first; one call should do one visible thing. The result reports the frame origin and \
             size in screen pixels, the desktop bounds and where the agent's pointer ended up, so add the \
             frame origin to a pixel you pick in the image to get the coordinate to click. Crop \
             with region when you already know where to look, add a wait step when an app needs \
             a moment, and lead with focus_window before typing into a window that is not in \
             front. \
             Format user-facing replies with Markdown by default: use ## headings for sections, \
             bullets or numbered lists for multiple items, **bold** for key terms, and fenced \
             code blocks with a language tag for code or commands. Use inline code for paths, \
             commands, identifiers, and short snippets. Keep paragraphs readable; do not flatten \
             lists or code into plain prose. Keep responses short and concrete. Paths may be relative to the working directory."
        )
    } else {
        format!(
            "You are yopes, a coding assistant running in the user's terminal.\n\
             Working directory: {cwd}\n\
             Operating system: {os}\n\
             Format replies with Markdown by default: use ## headings for sections, bullets or \
             numbered lists for multiple items, **bold** for key terms, and fenced code blocks \
             with language tags for code. Use inline code for paths, commands, and identifiers. \
             Keep paragraphs readable; do not flatten lists or code into plain prose. \
             Keep responses short and concrete."
        )
    };
    // How the assistant writes, whichever branch built the base prompt.
    let base = format!(
        "{base}\n\nWriting style: answer first, then the detail. Short sentences with \
         concrete nouns: real paths, symbols and error text, not 'it' or 'the issue'. No \
         filler, no self-congratulation, no recap of what was already said, no closing \
         'let me know'. A heading or a list earns its place only when it helps the reader; \
         a one-line answer gets one line. Never claim a result you did not verify."
    );
    let project_instructions = if prompt.chat() {
        // A plain chat should not read like a briefing on someone's repo.
        String::new()
    } else {
        project_instructions(workspace)
    };
    if project_instructions.is_empty() {
        base
    } else {
        format!(
            "{base}\n\nProject Markdown context (AGENTS.md files contain instructions):\n{project_instructions}"
        )
    }
}

/// AGENTS.md context is bounded and cached: a big workspace must not stall
/// the key handler while the system prompt is built for every message.
fn project_instructions(workspace: &Path) -> String {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (Instant, String)>>> = OnceLock::new();
    const TTL: Duration = Duration::from_secs(5);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(cache) = cache.lock()
        && let Some((loaded, text)) = cache.get(workspace)
        && loaded.elapsed() < TTL
    {
        return text.clone();
    }
    let text = load_agents_instructions(workspace);
    if let Ok(mut cache) = cache.lock() {
        cache.insert(workspace.to_path_buf(), (Instant::now(), text.clone()));
    }
    text
}

fn load_agents_instructions(root: &Path) -> String {
    const MAX_FILES: usize = 32;
    const MAX_TOTAL_CHARS: usize = 24_000;
    const MAX_FILE_CHARS: usize = 8_000;
    let mut files = Vec::new();
    collect_agents_files(root, &mut files);
    files.sort_by(|a, b| {
        let depth_a = a.parent().map_or(0, |p| {
            p.strip_prefix(root).unwrap_or(p).components().count()
        });
        let depth_b = b.parent().map_or(0, |p| {
            p.strip_prefix(root).unwrap_or(p).components().count()
        });
        depth_a.cmp(&depth_b).then_with(|| a.cmp(b))
    });
    let mut output = String::new();
    for path in files.into_iter().take(MAX_FILES) {
        if output.len() >= MAX_TOTAL_CHARS {
            break;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let content: String = content.chars().take(MAX_FILE_CHARS).collect();
        let relative = path.strip_prefix(root).unwrap_or(&path).display();
        let remaining = MAX_TOTAL_CHARS.saturating_sub(output.len());
        output.push_str(&format!("\n--- {} ---\n", relative));
        output.push_str(&content.chars().take(remaining).collect::<String>());
        output.push('\n');
    }
    output
}

/// Breadth-first with hard budgets. The old recursive walk read every
/// directory under a workspace — on a large project that blocked the UI on
/// every message. Shallow AGENTS.md files come first; a symlinked directory
/// is never followed (`file_type` does not resolve links).
fn collect_agents_files(root: &Path, files: &mut Vec<PathBuf>) {
    const MAX_DIRS: usize = 96;
    const MAX_ENTRIES: usize = 256;
    let mut queue = VecDeque::from([root.to_path_buf()]);
    let mut visited = 0;
    while let Some(directory) = queue.pop_front() {
        if visited >= MAX_DIRS || files.len() >= 64 {
            break;
        }
        visited += 1;
        // A root instruction file must never be lost to a huge directory
        // listing, even when the entry cap cuts that listing short.
        let root_instructions = if directory == root {
            let path = root.join("AGENTS.md");
            path.is_file().then_some(path)
        } else {
            None
        };
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        let mut children = Vec::new();
        for (index, entry) in entries.flatten().enumerate() {
            if index >= MAX_ENTRIES {
                break;
            }
            let path = entry.path();
            let name = entry.file_name();
            if name == ".git" || name == "target" || name == "node_modules" || name == ".venv" {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                children.push(path);
            } else if kind.is_file()
                && name == "AGENTS.md"
                && root_instructions.as_ref() != Some(&path)
            {
                // Only the instruction file counts. Collecting every Markdown
                // file would put the README into the system prompt.
                files.push(path);
            }
        }
        if let Some(path) = root_instructions {
            files.push(path);
        }
        children.sort();
        queue.extend(children);
    }
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod extracted_tests;
