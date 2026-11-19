//! Delegation bookkeeping: the lifecycle of a delegated run, the evidence it
//! leaves behind, and the verdict that evidence earns.
//!
//! A subagent's final answer is a claim. Before this module a run counted as
//! done the moment its model stopped calling tools, and the coordinator saw
//! only that claim. Now the run loop records what actually happened (files
//! whose bytes changed, commands and their exit codes, calls that were
//! refused), and `verdict` decides the outcome from that record. `Completed`
//! is reachable only through `Verifying`, so nothing finishes on words alone.
//!
//! Everything here is plain data and pure functions, except the snapshot
//! helpers at the bottom, which read the files and the git worktree the run
//! touched. The app owns routing and persistence; `main` owns the run loop.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::diff::FileDiff;
use crate::session::ToolCall;

/// The lifecycle of one delegated run.
///
/// Pending: registered, waiting for a run slot. Assigned: holds a slot, its
/// conversation is built. Running: the model has started answering.
/// Submitted: the model stopped working and handed in its answer. Verifying:
/// yopes is checking the evidence (and running the verify command, when one
/// was given). The last four are terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Pending,
    Assigned,
    Running,
    Submitted,
    Verifying,
    Completed,
    Failed,
    Blocked,
    Cancelled,
}

impl RunState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Blocked | Self::Cancelled
        )
    }

    pub fn is_active(self) -> bool {
        !self.is_terminal()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Assigned => "assigned",
            Self::Running => "running",
            Self::Submitted => "submitted",
            Self::Verifying => "verifying",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
            Self::Cancelled => "cancelled",
        }
    }

    /// Whether `self -> next` is a legal move. A terminal state never moves
    /// again, which is what makes a duplicate report harmless, and the only
    /// road to `Completed` runs through `Verifying`.
    pub fn can_move_to(self, next: Self) -> bool {
        if self.is_terminal() {
            return false;
        }
        if self == next {
            // A re-stated state is a no-op, never a move; the run loop and
            // the verdict both say "submitted".
            return next == Self::Submitted;
        }
        matches!(
            (self, next),
            (_, Self::Failed | Self::Cancelled)
                | (Self::Pending, Self::Assigned)
                | (Self::Assigned, Self::Running | Self::Submitted)
                | (Self::Running, Self::Submitted)
                // The loop announces the submission itself; the verdict then
                // re-states it on the way to verification.
                | (Self::Submitted, Self::Submitted)
                | (Self::Submitted, Self::Verifying)
                | (Self::Verifying, Self::Completed | Self::Blocked)
        )
    }
}

/// What the coordinator expects a run to produce.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// The task lands work: it must change at least one file.
    #[default]
    Change,
    /// Research or review: it must answer, and it should change nothing.
    ReadOnly,
}

impl TaskKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Change => "change",
            Self::ReadOnly => "read_only",
        }
    }
}

/// The run time a delegate gets when the call names none.
pub const DEFAULT_RUN_MINUTES: u64 = 45;
/// The longest run time a delegate call may ask for.
pub const MAX_RUN_MINUTES: u64 = 240;
/// How many expected files one call may name.
const MAX_EXPECTED_FILES: usize = 40;

/// A checked `delegate` call: the task plus what will count as done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegateSpec {
    pub task: String,
    pub kind: TaskKind,
    pub expect_files: Vec<String>,
    pub verify_command: Option<String>,
    pub timeout: Duration,
}

impl DelegateSpec {
    /// Read the optional fields of a `delegate` call next to its task. The
    /// error text is the tool result the model sees.
    pub fn from_call(call: &ToolCall, task: String) -> Result<Self, String> {
        let arguments: Value =
            serde_json::from_str(&call.function.arguments).unwrap_or(Value::Null);
        let kind = match arguments.get("kind").and_then(Value::as_str) {
            Some(raw) => match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
                "change" | "code" | "write" => TaskKind::Change,
                "read_only" | "readonly" | "research" | "review" => TaskKind::ReadOnly,
                other => {
                    return Err(format!(
                        "delegate kind `{other}` is unknown; use `change` or `read_only`"
                    ));
                }
            },
            None => infer_kind(&task),
        };
        let mut expect_files = Vec::new();
        if let Some(list) = arguments.get("expect_files") {
            let Some(list) = list.as_array() else {
                return Err("expect_files must be an array of paths".to_owned());
            };
            for entry in list {
                let Some(path) = entry.as_str().map(str::trim).filter(|path| !path.is_empty())
                else {
                    return Err("expect_files must hold non-empty path strings".to_owned());
                };
                if !expect_files.iter().any(|known: &String| same_path(known, path)) {
                    expect_files.push(path.to_owned());
                }
            }
        }
        if expect_files.len() > MAX_EXPECTED_FILES {
            return Err(format!(
                "expect_files names {} paths; the limit is {MAX_EXPECTED_FILES}",
                expect_files.len()
            ));
        }
        if kind == TaskKind::ReadOnly && !expect_files.is_empty() {
            return Err("a read_only task cannot expect files to change".to_owned());
        }
        let verify_command = arguments
            .get("verify_command")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|command| !command.is_empty())
            .map(str::to_owned);
        let minutes = match arguments.get("timeout_minutes") {
            None | Some(Value::Null) => DEFAULT_RUN_MINUTES,
            Some(value) => match value.as_u64() {
                Some(minutes) if minutes >= 1 => minutes.min(MAX_RUN_MINUTES),
                _ => return Err("timeout_minutes must be a whole number of at least 1".to_owned()),
            },
        };
        Ok(Self {
            task,
            kind,
            expect_files,
            verify_command,
            timeout: Duration::from_secs(minutes * 60),
        })
    }
}

/// A task that says it changes nothing is read-only; everything else is
/// expected to land work. An explicit `kind` always wins over this guess.
fn infer_kind(task: &str) -> TaskKind {
    let lower = task.to_ascii_lowercase();
    const READ_ONLY: [&str; 9] = [
        "read-only",
        "read only",
        "research only",
        "change no files",
        "do not change any file",
        "don't change any file",
        "do not modify any file",
        "don't modify any file",
        "without changing any file",
    ];
    if READ_ONLY.iter().any(|phrase| lower.contains(phrase)) {
        TaskKind::ReadOnly
    } else {
        TaskKind::Change
    }
}

/// Whether two paths name the same file, as far as text can tell: slashes
/// and a leading `./` are normalized, case is ignored on Windows, and a
/// relative path matches an absolute one that ends with it.
pub fn same_path(a: &str, b: &str) -> bool {
    let normal = |path: &str| {
        let mut path = path.trim().replace('\\', "/");
        while let Some(rest) = path.strip_prefix("./") {
            path = rest.to_owned();
        }
        let path = path.trim_end_matches('/').to_owned();
        if cfg!(windows) {
            path.to_ascii_lowercase()
        } else {
            path
        }
    };
    let (a, b) = (normal(a), normal(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || a.ends_with(&format!("/{b}")) || b.ends_with(&format!("/{a}"))
}

/// One file whose bytes really changed during a run.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FileChange {
    pub path: String,
    pub created: bool,
    pub deleted: bool,
    pub additions: usize,
    pub deletions: usize,
    /// Changed by a shell command (seen in the git worktree) rather than by
    /// a file tool.
    pub by_command: bool,
}

/// One shell command a run executed, and how it ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandRecord {
    pub command: String,
    /// The exit code; `None` when the command timed out or never started.
    pub exit: Option<i32>,
    pub ok: bool,
    /// A test, build or lint command: its last result counts in the verdict.
    pub check: bool,
}

impl CommandRecord {
    /// Read a `run_command` result back into a record. A success prints no
    /// exit line; a failure starts with `Exit code: N.`; a timeout says so.
    pub fn from_result(command: &str, result: &Result<String, String>) -> Self {
        let check = is_check_command(command);
        let (exit, ok) = match result {
            Err(_) => (None, false),
            Ok(output) => {
                let first = output.lines().next().unwrap_or_default();
                if first.starts_with("Command timed out") {
                    (None, false)
                } else if let Some(rest) = first.strip_prefix("Exit code: ") {
                    let code = rest.trim_end_matches('.').trim().parse::<i32>().ok();
                    (code, false)
                } else {
                    (Some(0), true)
                }
            }
        };
        Self {
            command: command.to_owned(),
            exit,
            ok,
            check,
        }
    }

    pub fn describe(&self) -> String {
        let end = match (self.ok, self.exit) {
            (true, _) => "ok".to_owned(),
            (false, Some(code)) => format!("exit {code}"),
            (false, None) => "timed out or failed to start".to_owned(),
        };
        format!("`{}` → {end}", clip(&self.command, 120))
    }
}

/// Whether a command tests, builds or lints: a failing last run of one of
/// these fails a change task, because "done" with red tests is not done.
pub fn is_check_command(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    const PHRASES: [&str; 22] = [
        "cargo test",
        "cargo check",
        "cargo clippy",
        "cargo build",
        "cargo nextest",
        "npm test",
        "npm run test",
        "npm run build",
        "pnpm test",
        "yarn test",
        "python -m pytest",
        "go test",
        "go build",
        "go vet",
        "make test",
        "make check",
        "dotnet test",
        "dotnet build",
        "mvn test",
        "gradle test",
        "gradlew test",
        "bun test",
    ];
    const WORDS: [&str; 7] = ["pytest", "jest", "vitest", "tsc", "ctest", "rspec", "phpunit"];
    PHRASES.iter().any(|phrase| lower.contains(phrase))
        || lower
            .split(|character: char| character.is_whitespace() || "&|;()".contains(character))
            .any(|token| WORDS.contains(&token.trim_start_matches("npx").trim()))
}

/// Everything a run did that can be checked, collected by the run loop.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Evidence {
    pub changes: Vec<FileChange>,
    /// Successful write calls that left the file byte-for-byte the same.
    pub noop_writes: usize,
    pub commands: Vec<CommandRecord>,
    /// Calls refused because they needed the user's approval.
    pub blocked: Vec<String>,
    pub tool_errors: usize,
    pub rounds: usize,
    /// Commands that ran where changes could not be observed (no git).
    pub unobserved_commands: usize,
}

impl Evidence {
    /// Fold one change in: the same path twice sums its line counts, and a
    /// file created then edited stays "created".
    pub fn record_change(&mut self, change: FileChange) {
        match self
            .changes
            .iter_mut()
            .find(|known| same_path(&known.path, &change.path))
        {
            Some(known) => {
                known.additions += change.additions;
                known.deletions += change.deletions;
                known.deleted = change.deleted;
                known.by_command &= change.by_command;
            }
            None => self.changes.push(change),
        }
    }

    pub fn changed_paths(&self) -> Vec<String> {
        self.changes.iter().map(|change| change.path.clone()).collect()
    }

    /// The last run of every distinct check command, in order of first use.
    fn last_checks(&self) -> Vec<&CommandRecord> {
        let mut last: Vec<&CommandRecord> = Vec::new();
        for record in self.commands.iter().filter(|record| record.check) {
            match last.iter_mut().find(|known| known.command == record.command) {
                Some(known) => *known = record,
                None => last.push(record),
            }
        }
        last
    }
}

/// How a run ended, as the run loop saw it, before any judgement.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RunOutcome {
    pub evidence: Evidence,
    pub answer: String,
    pub cancelled: bool,
    pub timed_out: Option<Duration>,
    /// A failure the run could not recover from: stream errors after every
    /// retry and failover, or a crashed run task.
    pub failure: Option<String>,
    /// The verify command yopes ran itself after the run submitted.
    pub verify: Option<CommandRecord>,
    /// Why the verify command did not run, when it did not.
    pub verify_skipped: Option<String>,
}

/// The judgement of one run: its terminal state, the reasons that decided
/// it, and warnings that did not change it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Verdict {
    pub state: RunState,
    pub reasons: Vec<String>,
    pub warnings: Vec<String>,
}

/// Decide a run's terminal state from its evidence. The model's answer only
/// matters for read-only work, where the answer is the deliverable.
pub fn verdict(kind: TaskKind, expect_files: &[String], outcome: &RunOutcome) -> Verdict {
    let evidence = &outcome.evidence;
    let mut reasons = Vec::new();
    let mut warnings = Vec::new();
    if let Some(note) = &outcome.verify_skipped {
        warnings.push(note.clone());
    }
    if evidence.unobserved_commands > 0 {
        warnings.push(format!(
            "{} command(s) ran outside a git worktree, so files they changed are not counted",
            evidence.unobserved_commands
        ));
    }
    let partial = || {
        let paths = evidence.changed_paths();
        (!paths.is_empty()).then(|| format!("partial changes remain: {}", paths.join(", ")))
    };
    let failed = |reasons: Vec<String>, warnings: Vec<String>| Verdict {
        state: RunState::Failed,
        reasons,
        warnings,
    };

    if outcome.cancelled {
        warnings.extend(partial());
        return Verdict {
            state: RunState::Cancelled,
            reasons: vec!["stopped before it finished".to_owned()],
            warnings,
        };
    }
    if let Some(limit) = outcome.timed_out {
        warnings.extend(partial());
        return failed(
            vec![format!("timed out after {} minutes", limit.as_secs() / 60)],
            warnings,
        );
    }
    if let Some(failure) = &outcome.failure {
        warnings.extend(partial());
        return failed(vec![failure.clone()], warnings);
    }
    if let Some(verify) = outcome.verify.as_ref().filter(|verify| !verify.ok) {
        reasons.push(format!("the verify command failed: {}", verify.describe()));
    }

    match kind {
        TaskKind::Change => {
            if evidence.changes.is_empty() {
                if !evidence.blocked.is_empty() {
                    return Verdict {
                        state: RunState::Blocked,
                        reasons: vec![format!(
                            "changed no files; it needed the user's approval for: {}",
                            evidence.blocked.join(", ")
                        )],
                        warnings,
                    };
                }
                let mut reason = "reported completion but changed no files".to_owned();
                if evidence.noop_writes > 0 {
                    reason.push_str(&format!(
                        " ({} write call(s) left the file unchanged)",
                        evidence.noop_writes
                    ));
                }
                reasons.push(reason);
            }
            for expected in expect_files {
                if !evidence
                    .changes
                    .iter()
                    .any(|change| same_path(&change.path, expected))
                {
                    reasons.push(format!("expected file was not changed: {expected}"));
                }
            }
            for check in evidence.last_checks() {
                if !check.ok {
                    reasons.push(format!("the last check run failed: {}", check.describe()));
                }
            }
            if outcome.answer.trim().is_empty() {
                warnings.push("no final answer; judged on the evidence only".to_owned());
            }
            if !evidence.blocked.is_empty() && !evidence.changes.is_empty() {
                warnings.push(format!(
                    "calls refused for lack of approval: {}",
                    evidence.blocked.join(", ")
                ));
            }
        }
        TaskKind::ReadOnly => {
            if outcome.answer.trim().is_empty() {
                reasons.push("returned no answer".to_owned());
            }
            if !evidence.changes.is_empty() {
                warnings.push(format!(
                    "a read-only task changed files: {}",
                    evidence.changed_paths().join(", ")
                ));
            }
            if !evidence.blocked.is_empty() {
                warnings.push(format!(
                    "calls refused for lack of approval: {}",
                    evidence.blocked.join(", ")
                ));
            }
        }
    }
    if reasons.is_empty() {
        Verdict {
            state: RunState::Completed,
            reasons,
            warnings,
        }
    } else {
        failed(reasons, warnings)
    }
}

/// The longest stretch of the subagent's own answer a report carries; the
/// full text stays readable in `/agents`.
const REPORT_ANSWER_CHARS: usize = 4_000;

/// The structured report the coordinator receives for one finished run.
///
/// The verdict and the evidence come first; the subagent's own answer comes
/// last, labelled as a claim, so a confident summary cannot read as proof.
pub fn report(
    run: u64,
    label: &str,
    kind: TaskKind,
    verdict: &Verdict,
    outcome: &RunOutcome,
    elapsed: Duration,
    others_active: &[u64],
) -> String {
    let evidence = &outcome.evidence;
    let mut lines = vec![format!(
        "[delegation report] run {run} · {} · verdict: {}",
        clip(label, 80),
        verdict.state.label().to_ascii_uppercase()
    )];
    lines.push(format!(
        "kind: {} · {} tool round(s) · {}",
        kind.label(),
        evidence.rounds,
        format_elapsed(elapsed)
    ));
    if !verdict.reasons.is_empty() {
        lines.push("reasons:".to_owned());
        lines.extend(verdict.reasons.iter().map(|reason| format!("- {reason}")));
    }
    if !verdict.warnings.is_empty() {
        lines.push("warnings:".to_owned());
        lines.extend(verdict.warnings.iter().map(|warning| format!("- {warning}")));
    }
    lines.push("evidence (observed by yopes, not reported by the subagent):".to_owned());
    if evidence.changes.is_empty() {
        lines.push("- files changed: none".to_owned());
    } else {
        lines.push(format!("- files changed: {}", evidence.changes.len()));
        for change in evidence.changes.iter().take(30) {
            let what = if change.deleted {
                "deleted".to_owned()
            } else if change.created {
                format!("created +{}", change.additions)
            } else {
                format!("+{} -{}", change.additions, change.deletions)
            };
            let source = if change.by_command { " (by a command)" } else { "" };
            lines.push(format!("  - {} {what}{source}", change.path));
        }
        if evidence.changes.len() > 30 {
            lines.push(format!("  - … {} more", evidence.changes.len() - 30));
        }
    }
    if evidence.noop_writes > 0 {
        lines.push(format!(
            "- write calls with no effect: {}",
            evidence.noop_writes
        ));
    }
    if !evidence.commands.is_empty() {
        lines.push(format!("- commands run: {}", evidence.commands.len()));
        for record in evidence.commands.iter().rev().take(8).rev() {
            lines.push(format!("  - {}", record.describe()));
        }
    }
    if let Some(verify) = &outcome.verify {
        lines.push(format!("- verify command: {}", verify.describe()));
    }
    if evidence.tool_errors > 0 {
        lines.push(format!("- tool calls that errored: {}", evidence.tool_errors));
    }
    if !others_active.is_empty() {
        let ids: Vec<String> = others_active.iter().map(|id| format!("run {id}")).collect();
        lines.push(format!(
            "outstanding: {} other delegated run(s) still active ({}); the overall task is \
             not finished",
            others_active.len(),
            ids.join(", ")
        ));
    }
    let answer = outcome.answer.trim();
    lines.push("--- the subagent's own answer (a claim, not evidence) ---".to_owned());
    if answer.is_empty() {
        lines.push("(none)".to_owned());
    } else {
        lines.push(clip(answer, REPORT_ANSWER_CHARS));
    }
    lines.join("\n")
}

/// One delegated run as its owning session remembers it: persisted with the
/// session, so the ledger survives a restart and a run that was still active
/// then is not silently assumed to have finished.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DelegationRecord {
    pub run: u64,
    pub call_id: String,
    pub label: String,
    pub kind: TaskKind,
    pub state: RunState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    pub started_unix: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_unix: Option<u64>,
}

/// Seconds since the epoch, for persisted timestamps.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// The reason a restored run carries: it was active when the app stopped,
/// and no run outlives the process that drove it.
pub const INTERRUPTED: &str = "interrupted: yopes stopped before this run finished; its work \
                               was never verified";

/// After a session loads, every run it remembers as active is dead: mark it
/// failed with the reason, so the ledger tells the truth. Returns how many.
pub fn restore(records: &mut [DelegationRecord]) -> usize {
    let mut interrupted = 0;
    for record in records.iter_mut().filter(|record| record.state.is_active()) {
        record.state = RunState::Failed;
        record.reasons = vec![INTERRUPTED.to_owned()];
        record.finished_unix = Some(unix_now());
        interrupted += 1;
    }
    interrupted
}

/// How many finished runs the ledger lists besides the active ones.
const LEDGER_FINISHED: usize = 10;

/// The ledger appended to the coordinator's system prompt on every request.
///
/// The model forgets what it delegated as the context fills and compacts;
/// this note is rebuilt from the session's records each time, so the state of
/// every run is in front of it whenever it decides what to tell the user.
pub fn ledger(records: &[DelegationRecord], now_unix: u64) -> Option<String> {
    if records.is_empty() {
        return None;
    }
    let active: Vec<&DelegationRecord> =
        records.iter().filter(|record| record.state.is_active()).collect();
    let unresolved = records
        .iter()
        .filter(|record| matches!(record.state, RunState::Failed | RunState::Blocked))
        .count();
    let mut lines = vec![
        "DELEGATION LEDGER (kept by yopes from observed evidence; it outranks any subagent's \
         own words):"
            .to_owned(),
    ];
    let line = |record: &DelegationRecord| {
        let mut text = format!(
            "- run {} · {} · {}",
            record.run,
            clip(&record.label, 60),
            record.state.label()
        );
        if record.state.is_active() {
            let minutes = now_unix.saturating_sub(record.started_unix) / 60;
            text.push_str(&format!(" for {minutes}m"));
        }
        if let Some(reason) = record.reasons.first() {
            text.push_str(&format!(" · {}", clip(reason, 140)));
        }
        if !record.files.is_empty() && record.state == RunState::Completed {
            text.push_str(&format!(" · {} file(s) changed", record.files.len()));
        }
        text
    };
    lines.extend(active.iter().map(|record| line(record)));
    let finished: Vec<&DelegationRecord> =
        records.iter().filter(|record| record.state.is_terminal()).collect();
    let skip = finished.len().saturating_sub(LEDGER_FINISHED);
    lines.extend(finished.iter().skip(skip).map(|record| line(record)));
    if !active.is_empty() {
        lines.push(format!(
            "{} run(s) still active. Their reports arrive on their own: do not tell the user \
             the work is finished, and do not assume what they will do.",
            active.len()
        ));
    }
    if unresolved > 0 {
        lines.push(format!(
            "{unresolved} run(s) failed or are blocked. Say so plainly, then redo, re-delegate or \
             report that work as unfinished; never present it as done."
        ));
    }
    Some(lines.join("\n"))
}

/// A run's elapsed time, short: `42s`, `3m12s`, `1h05m`.
fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h{:02}m", seconds / 3_600, (seconds % 3_600) / 60)
    }
}

/// `text` cut to `chars` characters, with an ellipsis when cut.
fn clip(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_owned();
    }
    let mut clipped: String = text.chars().take(chars).collect();
    clipped.push('…');
    clipped
}

// Observation: reading what a tool call actually did to the files.

/// Files larger than this are compared by size and timestamp only.
const MAX_SNAPSHOT_BYTES: u64 = 8 * 1024 * 1024;
/// Text diffs larger than this are counted as changed without line counts.
const MAX_DIFF_BYTES: usize = 512 * 1024;

/// The bytes of every file a write call targets, read just before it runs.
pub struct Snapshot {
    entries: Vec<(String, PathBuf, Option<Vec<u8>>)>,
}

/// Snapshot the targets of a file-writing call, resolved against `root`.
pub fn snapshot_targets(call: &ToolCall, root: &Path) -> Snapshot {
    let targets = crate::tools::scoped_workspace(root, || crate::tools::edit_targets(call));
    let entries = targets
        .into_iter()
        .map(|(path, _)| {
            let absolute = if Path::new(&path).is_absolute() {
                PathBuf::from(&path)
            } else {
                root.join(&path)
            };
            let before = read_capped(&absolute);
            (path, absolute, before)
        })
        .collect();
    Snapshot { entries }
}

fn read_capped(path: &Path) -> Option<Vec<u8>> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    if metadata.len() > MAX_SNAPSHOT_BYTES {
        // A stand-in that still changes when the file does.
        let stamp = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos());
        return Some(format!("large:{}:{stamp}", metadata.len()).into_bytes());
    }
    fs::read(path).ok()
}

/// Compare a snapshot with the files as they are now. Returns the files whose
/// bytes really changed and how many targets stayed exactly the same.
pub fn compare(snapshot: Snapshot) -> (Vec<FileChange>, usize) {
    let mut changes = Vec::new();
    let mut unchanged = 0;
    for (path, absolute, before) in snapshot.entries {
        let after = read_capped(&absolute);
        if before == after {
            unchanged += 1;
            continue;
        }
        changes.push(file_change(path, before.as_deref(), after.as_deref(), false));
    }
    (changes, unchanged)
}

fn file_change(path: String, before: Option<&[u8]>, after: Option<&[u8]>, by_command: bool) -> FileChange {
    let (additions, deletions) = match (before, after) {
        (Some(before), Some(after)) if before.len() + after.len() <= MAX_DIFF_BYTES => {
            match (std::str::from_utf8(before), std::str::from_utf8(after)) {
                (Ok(before), Ok(after)) => {
                    let diff = FileDiff::new(path.clone(), before, after, false);
                    (diff.additions, diff.deletions)
                }
                _ => (0, 0),
            }
        }
        (None, Some(after)) if after.len() <= MAX_DIFF_BYTES => (
            std::str::from_utf8(after).map_or(0, |text| text.lines().count()),
            0,
        ),
        _ => (0, 0),
    };
    FileChange {
        path,
        created: before.is_none() && after.is_some(),
        deleted: before.is_some() && after.is_none(),
        additions,
        deletions,
        by_command,
    }
}

/// The dirty state of a git worktree: every path `git status` lists, with a
/// hash of its bytes. Two fingerprints taken around one command show the
/// files that command changed. `None` outside a git worktree.
pub struct Fingerprint {
    top: PathBuf,
    files: BTreeMap<String, u64>,
}

/// Take the fingerprint of the worktree holding `root`.
pub fn fingerprint(root: &Path) -> Option<Fingerprint> {
    let top = git(root, &["rev-parse", "--show-toplevel"])?;
    let top = PathBuf::from(String::from_utf8_lossy(&top).trim());
    let status = git(
        &top,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let mut files = BTreeMap::new();
    let mut entries = status.split(|byte| *byte == 0).filter(|entry| !entry.is_empty());
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let code = &entry[..2];
        let path = String::from_utf8_lossy(&entry[3..]).into_owned();
        files.insert(path.clone(), hash_file(&top.join(&path)));
        // A rename or copy carries its source path as the next entry.
        if code.contains(&b'R') || code.contains(&b'C') {
            if let Some(source) = entries.next() {
                let source = String::from_utf8_lossy(source).into_owned();
                files.insert(source.clone(), hash_file(&top.join(&source)));
            }
        }
    }
    Some(Fingerprint { top, files })
}

impl Fingerprint {
    /// The files whose state differs between `self` (before) and `after`.
    pub fn changed_since(&self, after: &Fingerprint) -> Vec<FileChange> {
        if self.top != after.top {
            return Vec::new();
        }
        let mut paths: Vec<&String> = self.files.keys().chain(after.files.keys()).collect();
        paths.sort();
        paths.dedup();
        paths
            .into_iter()
            .filter(|path| self.files.get(*path) != after.files.get(*path))
            .map(|path| {
                let now = read_capped(&self.top.join(path));
                FileChange {
                    path: path.clone(),
                    created: !self.files.contains_key(path) && now.is_some(),
                    deleted: now.is_none(),
                    additions: 0,
                    deletions: 0,
                    by_command: true,
                }
            })
            .collect()
    }
}

fn git(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let mut command = std::process::Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.quotepath=off"])
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashes up for each fingerprint.
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().ok()?;
    output.status.success().then_some(output.stdout)
}

fn hash_file(path: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    match read_capped(path) {
        None => u64::MAX,
        Some(bytes) => {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            bytes.hash(&mut hasher);
            hasher.finish()
        }
    }
}

/// The id a talk message carries in the conversation, derived from its text
/// so a re-sent message is recognized instead of delivered twice. The talk
/// tools already stamp a `[talk from …]` prefix, which the hash includes, so
/// two tabs saying the same words stay two messages.
pub fn talk_id(text: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.trim().hash(&mut hasher);
    format!("talk-{:016x}", hasher.finish())
}

/// What a deadline-bounded receive produced.
#[derive(Debug, Eq, PartialEq)]
pub enum Next<T> {
    Item(T),
    Closed,
    TimedOut,
}

/// Receive the next event, or report that the run's deadline passed first.
pub async fn next_before<T>(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<T>,
    deadline: tokio::time::Instant,
) -> Next<T> {
    match tokio::time::timeout_at(deadline, receiver.recv()).await {
        Ok(Some(item)) => Next::Item(item),
        Ok(None) => Next::Closed,
        Err(_) => Next::TimedOut,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(path: &str) -> FileChange {
        FileChange {
            path: path.to_owned(),
            additions: 3,
            ..FileChange::default()
        }
    }

    fn outcome(answer: &str) -> RunOutcome {
        RunOutcome {
            answer: answer.to_owned(),
            ..RunOutcome::default()
        }
    }

    fn verdict_state(expected: &[String], run: &RunOutcome) -> RunState {
        verdict(TaskKind::Change, expected, run).state
    }

    #[test]
    fn completed_is_reachable_only_through_verifying() {
        use RunState::*;
        assert!(Pending.can_move_to(Assigned));
        assert!(Assigned.can_move_to(Running));
        assert!(Running.can_move_to(Submitted));
        assert!(Submitted.can_move_to(Verifying));
        assert!(Verifying.can_move_to(Completed));
        for state in [Pending, Assigned, Running, Submitted] {
            assert!(!state.can_move_to(Completed), "{state:?} skipped verification");
        }
        // Terminal states never move again: a duplicate report is a no-op.
        for state in [Completed, Failed, Blocked, Cancelled] {
            for next in [Running, Completed, Failed, Cancelled] {
                assert!(!state.can_move_to(next), "{state:?} -> {next:?}");
            }
        }
        for state in [Pending, Assigned, Running, Submitted, Verifying] {
            assert!(state.can_move_to(Failed));
            assert!(state.can_move_to(Cancelled));
        }
    }

    #[test]
    fn a_claimed_completion_with_no_changes_fails() {
        let claim = outcome("Done! I implemented the parser and all tests pass.");
        let verdict = verdict(TaskKind::Change, &[], &claim);
        assert_eq!(verdict.state, RunState::Failed);
        assert!(verdict.reasons[0].contains("changed no files"), "{verdict:?}");
    }

    #[test]
    fn writes_that_changed_nothing_are_named_in_the_failure() {
        let mut claim = outcome("Updated config.rs");
        claim.evidence.noop_writes = 2;
        let verdict = verdict(TaskKind::Change, &[], &claim);
        assert_eq!(verdict.state, RunState::Failed);
        assert!(verdict.reasons[0].contains("2 write call(s) left the file unchanged"));
    }

    #[test]
    fn a_missing_expected_file_fails_even_with_other_changes() {
        let mut run = outcome("done");
        run.evidence.record_change(change("src/other.rs"));
        let expected = vec!["src/parser.rs".to_owned()];
        let verdict = verdict(TaskKind::Change, &expected, &run);
        assert_eq!(verdict.state, RunState::Failed);
        assert_eq!(
            verdict.reasons,
            vec!["expected file was not changed: src/parser.rs".to_owned()]
        );
        // The same file under another spelling counts.
        run.evidence.record_change(change("./src\\parser.rs"));
        assert_eq!(verdict_state(&expected, &run), RunState::Completed);
    }

    #[test]
    fn red_tests_fail_a_change_task_but_a_later_green_run_clears_it() {
        let mut run = outcome("done");
        run.evidence.record_change(change("src/lib.rs"));
        let failing = CommandRecord::from_result(
            "cargo test",
            &Ok("Exit code: 101.\ntest result: FAILED".to_owned()),
        );
        assert_eq!(failing.exit, Some(101));
        assert!(!failing.ok && failing.check);
        run.evidence.commands.push(failing);
        assert_eq!(verdict_state(&[], &run), RunState::Failed);
        let passing = CommandRecord::from_result("cargo test", &Ok("test result: ok".to_owned()));
        assert!(passing.ok);
        run.evidence.commands.push(passing);
        assert_eq!(verdict_state(&[], &run), RunState::Completed);
    }

    #[test]
    fn a_failing_verify_command_fails_the_run() {
        let mut run = outcome("done");
        run.evidence.record_change(change("src/lib.rs"));
        run.verify = Some(CommandRecord::from_result(
            "make check",
            &Ok("Exit code: 2.\n".to_owned()),
        ));
        let verdict = verdict(TaskKind::Change, &[], &run);
        assert_eq!(verdict.state, RunState::Failed);
        assert!(verdict.reasons[0].contains("verify command failed"));
    }

    #[test]
    fn blocked_calls_without_changes_read_as_blocked() {
        let mut run = outcome("I could not run it");
        run.evidence.blocked.push("run_command".to_owned());
        assert_eq!(
            verdict(TaskKind::Change, &[], &run).state,
            RunState::Blocked
        );
    }

    #[test]
    fn cancel_timeout_and_failure_never_complete_and_keep_partial_work_visible() {
        let mut run = outcome("done");
        run.evidence.record_change(change("src/half.rs"));
        run.cancelled = true;
        let cancelled = verdict(TaskKind::Change, &[], &run);
        assert_eq!(cancelled.state, RunState::Cancelled);
        assert!(
            cancelled
                .warnings
                .iter()
                .any(|warning| warning.contains("src/half.rs"))
        );

        run.cancelled = false;
        run.timed_out = Some(Duration::from_secs(45 * 60));
        let timed_out = verdict(TaskKind::Change, &[], &run);
        assert_eq!(timed_out.state, RunState::Failed);
        assert_eq!(
            timed_out.reasons,
            vec!["timed out after 45 minutes".to_owned()]
        );

        run.timed_out = None;
        run.failure = Some("stream error after every retry".to_owned());
        assert_eq!(verdict(TaskKind::Change, &[], &run).state, RunState::Failed);
    }

    #[test]
    fn read_only_work_needs_an_answer_and_only_warns_on_changes() {
        assert_eq!(
            verdict(TaskKind::ReadOnly, &[], &outcome("")).state,
            RunState::Failed
        );
        let mut run = outcome("The parser lives in src/parse.rs.");
        assert_eq!(
            verdict(TaskKind::ReadOnly, &[], &run).state,
            RunState::Completed
        );
        run.evidence.record_change(change("src/parse.rs"));
        let verdict = verdict(TaskKind::ReadOnly, &[], &run);
        assert_eq!(verdict.state, RunState::Completed);
        assert!(verdict.warnings[0].contains("read-only task changed files"));
    }

    #[test]
    fn the_spec_reads_kind_files_verify_and_timeout() {
        let call = ToolCall::new(
            "c1",
            "delegate",
            r#"{"task":"t","kind":"change","expect_files":["a.rs","./a.rs","b.rs"],
                "verify_command":" cargo test ","timeout_minutes":10}"#,
        );
        let spec = DelegateSpec::from_call(&call, "t".to_owned()).unwrap();
        assert_eq!(spec.kind, TaskKind::Change);
        assert_eq!(spec.expect_files, vec!["a.rs", "b.rs"]);
        assert_eq!(spec.verify_command.as_deref(), Some("cargo test"));
        assert_eq!(spec.timeout, Duration::from_secs(600));

        let bare = ToolCall::new(
            "c2",
            "delegate",
            r#"{"task":"Research only, change no files"}"#,
        );
        let spec =
            DelegateSpec::from_call(&bare, "Research only, change no files".to_owned()).unwrap();
        assert_eq!(spec.kind, TaskKind::ReadOnly);
        assert_eq!(spec.timeout, Duration::from_secs(DEFAULT_RUN_MINUTES * 60));

        let capped = ToolCall::new("c3", "delegate", r#"{"task":"t","timeout_minutes":99999}"#);
        let spec = DelegateSpec::from_call(&capped, "t".to_owned()).unwrap();
        assert_eq!(spec.timeout, Duration::from_secs(MAX_RUN_MINUTES * 60));

        let bad = ToolCall::new(
            "c4",
            "delegate",
            r#"{"task":"t","kind":"read_only","expect_files":["a"]}"#,
        );
        assert!(DelegateSpec::from_call(&bad, "t".to_owned()).is_err());
        let bad = ToolCall::new("c5", "delegate", r#"{"task":"t","timeout_minutes":0}"#);
        assert!(DelegateSpec::from_call(&bad, "t".to_owned()).is_err());
    }

    #[test]
    fn the_report_puts_the_verdict_and_evidence_before_the_claim() {
        let mut run = outcome("All done, everything works.");
        run.evidence.rounds = 4;
        let verdict = verdict(TaskKind::Change, &[], &run);
        let text = report(
            7,
            "Parser",
            TaskKind::Change,
            &verdict,
            &run,
            Duration::from_secs(75),
            &[9],
        );
        let verdict_at = text.find("verdict: FAILED").expect("verdict");
        let evidence_at = text.find("files changed: none").expect("evidence");
        let claim_at = text.find("a claim, not evidence").expect("claim label");
        assert!(verdict_at < evidence_at && evidence_at < claim_at, "{text}");
        assert!(text.contains("outstanding: 1 other delegated run(s) still active (run 9)"));
        assert!(text.contains("1m15s"));
    }

    #[test]
    fn restoring_marks_active_runs_interrupted_and_leaves_finished_ones() {
        let record = |run, state| DelegationRecord {
            run,
            call_id: format!("c{run}"),
            label: "x".to_owned(),
            kind: TaskKind::Change,
            state,
            reasons: Vec::new(),
            files: Vec::new(),
            started_unix: 0,
            finished_unix: None,
        };
        let mut records = vec![
            record(1, RunState::Running),
            record(2, RunState::Completed),
            record(3, RunState::Verifying),
        ];
        assert_eq!(restore(&mut records), 2);
        assert_eq!(records[0].state, RunState::Failed);
        assert_eq!(records[0].reasons, vec![INTERRUPTED.to_owned()]);
        assert_eq!(records[1].state, RunState::Completed);
        assert_eq!(records[2].state, RunState::Failed);
    }

    #[test]
    fn the_ledger_names_active_and_failed_runs_and_forbids_declaring_done() {
        let mut records = vec![DelegationRecord {
            run: 4,
            call_id: "c4".to_owned(),
            label: "Wire the parser".to_owned(),
            kind: TaskKind::Change,
            state: RunState::Running,
            reasons: Vec::new(),
            files: Vec::new(),
            started_unix: 100,
            finished_unix: None,
        }];
        let text = ledger(&records, 280).unwrap();
        assert!(
            text.contains("run 4 · Wire the parser · running for 3m"),
            "{text}"
        );
        assert!(text.contains("do not tell the user the work is finished"));
        records[0].state = RunState::Failed;
        records[0].reasons = vec!["changed no files".to_owned()];
        let text = ledger(&records, 400).unwrap();
        assert!(text.contains("failed · changed no files"), "{text}");
        assert!(text.contains("never present it as done"));
        assert!(!text.contains("still active"));
        assert!(ledger(&[], 0).is_none());
    }

    #[test]
    fn check_commands_are_recognized() {
        for command in [
            "cargo test --all",
            "npx jest",
            "pytest -q",
            "cd app && npm test",
            "go vet ./...",
        ] {
            assert!(is_check_command(command), "{command}");
        }
        for command in ["git status", "ls -la", "echo testing"] {
            assert!(!is_check_command(command), "{command}");
        }
    }

    #[test]
    fn snapshots_count_only_real_byte_changes() {
        let dir = std::env::temp_dir().join(format!("yopes-snap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("same.txt"), "one\ntwo\n").unwrap();
        fs::write(dir.join("edit.txt"), "one\ntwo\n").unwrap();

        let same = ToolCall::new(
            "a",
            "write_file",
            r#"{"path":"same.txt","content":"one\ntwo\n"}"#,
        );
        let snapshot = snapshot_targets(&same, &dir);
        fs::write(dir.join("same.txt"), "one\ntwo\n").unwrap();
        let (changes, unchanged) = compare(snapshot);
        assert!(changes.is_empty());
        assert_eq!(unchanged, 1);

        let edit = ToolCall::new(
            "b",
            "edit_file",
            r#"{"path":"edit.txt","old_text":"two","new_text":"2"}"#,
        );
        let snapshot = snapshot_targets(&edit, &dir);
        fs::write(dir.join("edit.txt"), "one\n2\nthree\n").unwrap();
        let (changes, _) = compare(snapshot);
        assert_eq!(changes.len(), 1);
        assert_eq!((changes[0].additions, changes[0].deletions), (2, 1));

        let create = ToolCall::new("c", "write_file", r#"{"path":"new.txt","content":"x"}"#);
        let snapshot = snapshot_targets(&create, &dir);
        fs::write(dir.join("new.txt"), "x\n").unwrap();
        let (changes, _) = compare(snapshot);
        assert!(changes[0].created);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_git_fingerprint_sees_files_a_command_changed() {
        let dir = std::env::temp_dir().join(format!("yopes-git-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let init = std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .arg("init")
            .output();
        if !init.is_ok_and(|output| output.status.success()) {
            // No git here: the fingerprint is unavailable and the verdict
            // warns about unobserved commands instead.
            let _ = fs::remove_dir_all(&dir);
            return;
        }
        fs::write(dir.join("a.txt"), "1").unwrap();
        let before = fingerprint(&dir).expect("inside a worktree");
        fs::write(dir.join("a.txt"), "2").unwrap();
        fs::write(dir.join("b.txt"), "new").unwrap();
        let after = fingerprint(&dir).unwrap();
        let mut paths: Vec<String> = before
            .changed_since(&after)
            .into_iter()
            .map(|change| change.path)
            .collect();
        paths.sort();
        assert_eq!(paths, vec!["a.txt", "b.txt"]);
        assert!(fingerprint(&dir.join("no-such-dir")).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_deadline_receive_times_out_instead_of_hanging() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<u8>();
        let soon = tokio::time::Instant::now() + Duration::from_millis(30);
        assert_eq!(next_before(&mut receiver, soon).await, Next::TimedOut);
        sender.send(5).unwrap();
        let later = tokio::time::Instant::now() + Duration::from_secs(5);
        assert_eq!(next_before(&mut receiver, later).await, Next::Item(5));
        drop(sender);
        assert_eq!(next_before(&mut receiver, later).await, Next::Closed);
    }
}
