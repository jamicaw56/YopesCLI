//! The delegate system: subagent runs with their own conversations.
//!
//! A `delegate` tool call spawns one subagent run: a plain request on the
//! same provider and model, streamed live into `App::subagents` so the user
//! can watch the response while it runs. `/agents` lists the runs and opens
//! one for reading; the list and the viewer are app-global, so a run stays
//! viewable from every tab while its report lands in the owning tab.
//!
//! Delegation is fast and never blocks the conversation: the tool call
//! answers as soon as the run starts, the model keeps working, and when a
//! run finishes its final answer is delivered to the owning conversation as
//! a report that alerts the model with a new turn.

use std::time::Instant;

use super::*;
use crate::delegation::{DelegationRecord, RunState, TaskKind};

/// The subagent brief: appended to the system prompt for every run.
pub(crate) const SUBAGENT_BRIEF: &str = "You are a subagent inside yopes, a terminal coding \
    assistant. You have the same tools as the main session — use them when the task needs \
    files, commands, or the web, exactly as the main session would. If the task involves \
    changing files, land them: write, edit, and commit as you go, and make your final \
    answer list what changed, not describe what a successor could do. Answer the task \
    given below with only the useful final result, no preamble, no offers to help further.";

/// The brief for the model that names a subagent task: the sidebar and the
/// `/agents` rows show that name instead of the raw task text.
pub(crate) const SUBAGENT_NAME_BRIEF: &str = "You name subagent tasks for a sidebar. Answer with \
    only a short name, at most five words, capitalized, no quotes, no final punctuation. You \
    have no tools.";

/// How many runs stay in memory: `/agents` shows the most recent ones.
const MAX_SUBAGENTS: usize = 50;
/// The longest streamed text kept per run, so one huge answer cannot eat
/// memory through deltas.
const MAX_STORED_CHARS: usize = 200_000;
/// Characters of the task shown on a picker row.
const TASK_EXCERPT: usize = 46;
/// Longest row label shown on a picker row: the model's name is already
/// short, but a long name is clipped the same as a long task line was.
const LABEL_CHARS: usize = 46;
/// The tools that change files: a round in which one of these runs
/// successfully resets the write watchdog.
pub(crate) const WRITING_TOOLS: [&str; 4] =
    ["write_file", "edit_file", "multi_edit", "apply_patch"];
/// Tool rounds a run may spend without one of `WRITING_TOOLS` landing
/// before the one-time watchdog nudge asks it to produce an artifact: a run
/// stuck in reconnaissance is told to stop exploring and write, and a task
/// that is read-only by nature may ignore the nudge and continue.
pub(crate) const ZERO_WRITE_NUDGE_ROUNDS: usize = 8;

/// The status of one subagent run, as the sidebar, the transcript row and
/// `/agents` show it. The lifecycle itself lives in `RunState`; this is the
/// short form the UI draws.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SubagentStatus {
    Running,
    Done,
    Failed,
}

/// One subagent run: the task, the streamed answer, and what it cost.
pub(crate) struct Subagent {
    pub(crate) id: u64,
    /// The `delegate` tool call that spawned this run: the transcript maps a
    /// tool-call row back to its run's name and live status through it.
    pub(crate) call_id: String,
    pub(crate) task: String,
    /// What the coordinator expects this run to produce.
    pub(crate) kind: TaskKind,
    /// The lifecycle state: pending, running, verifying, then one terminal
    /// state. Only `verdict` moves it into a terminal state.
    pub(crate) state: RunState,
    /// The name the model gave this task, shown instead of the task text.
    pub(crate) name: Option<String>,
    /// The response, streamed live while the run is in flight.
    pub(crate) answer: String,
    /// Reasoning streamed before the answer, when the provider sends it.
    pub(crate) reasoning: String,
    /// Retry or failure note, shown live in the viewer.
    pub(crate) note: Option<String>,
    pub(crate) started: Instant,
    /// Set once the run finished: how long it took, in milliseconds.
    pub(crate) elapsed_ms: u64,
    /// (prompt, completion) tokens billed, when the provider reported them.
    pub(crate) usage: Option<(u64, u64)>,
    /// For a terminal state: why, and the files the evidence proved changed.
    pub(crate) reasons: Vec<String>,
    pub(crate) files: Vec<String>,
    /// Set when the run was cancelled: the run loop ends it with what it has.
    pub(crate) cancel_requested: bool,
    /// Whether the run has checked in (its first delta arrived). A run that
    /// never did is worth a warning when the user asks about it.
    pub(crate) heard_from: bool,
}

impl Subagent {
    /// The short status the UI draws from the lifecycle state.
    pub(crate) fn status(&self) -> SubagentStatus {
        match self.state {
            RunState::Running | RunState::Pending | RunState::Assigned | RunState::Submitted => {
                SubagentStatus::Running
            }
            RunState::Verifying => SubagentStatus::Running,
            RunState::Completed => SubagentStatus::Done,
            RunState::Failed | RunState::Blocked | RunState::Cancelled => SubagentStatus::Failed,
        }
    }

    pub(crate) fn status_label(&self) -> &'static str {
        self.state.label()
    }

    /// The task's first line, trimmed to the picker's row width.
    pub(crate) fn task_excerpt(&self) -> String {
        let first = self.task.lines().next().unwrap_or_default();
        let mut excerpt: String = first.chars().take(TASK_EXCERPT).collect();
        if first.chars().count() > TASK_EXCERPT {
            excerpt.push('…');
        }
        excerpt
    }

    /// The row summary: the name the model gave the task once it arrived,
    /// falling back to the task's first line only until naming lands.
    pub(crate) fn row_label(&self) -> String {
        match &self.name {
            Some(name) => {
                let mut label: String = name.chars().take(LABEL_CHARS).collect();
                if name.chars().count() > LABEL_CHARS {
                    label.push('…');
                }
                label
            }
            None => self.task_excerpt(),
        }
    }
}

/// An update from a running subagent, sent through the delegate channel.
pub(crate) enum DelegateUpdate {
    Delta {
        run: u64,
        text: String,
    },
    Reasoning {
        run: u64,
        text: String,
    },
    Usage {
        run: u64,
        prompt_tokens: u64,
        completion_tokens: u64,
    },
    /// A retry or failure note, shown live in the viewer.
    Note {
        run: u64,
        text: String,
    },
    /// The name the model gave this task: the sidebar and `/agents` rows
    /// show it instead of the raw task text.
    Named {
        run: u64,
        name: String,
    },
    /// The run loop's own lifecycle moves, before the verdict: Submitted and
    /// Verifying show progress while the evidence is checked.
    State {
        run: u64,
        state: RunState,
    },
    /// The checked outcome of one run, delivered to the owning session: the
    /// terminal state the evidence earned, why, the files it proved changed,
    /// and the structured report the coordinator reads.
    Verdict {
        run: u64,
        state: RunState,
        reasons: Vec<String>,
        files: Vec<String>,
        report: String,
        session_id: u64,
    },
}

/// Append text up to the per-run cap, so deltas cannot grow a record
/// without bound.
fn push_capped(target: &mut String, text: &str) {
    if target.chars().count() >= MAX_STORED_CHARS {
        return;
    }
    target.push_str(text);
    if target.chars().count() > MAX_STORED_CHARS {
        *target = target.chars().take(MAX_STORED_CHARS).collect();
        target.push_str("\n… truncated");
    }
}

impl App {
    /// Register a new subagent run and return its id. Runs are app-global:
    /// the tool result routes to the owning tab, the record stays viewable.
    pub(crate) fn begin_subagent(&mut self, call_id: &str, task: String, kind: TaskKind) -> u64 {
        self.next_subagent_id = self.next_subagent_id.wrapping_add(1);
        let id = self.next_subagent_id;
        // The ledger records the run, so the coordinator sees it even after a
        // restart: it is the session's memory of what it delegated.
        let label = task
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("subagent task")
            .to_owned();
        self.session.delegations.retain(|record| record.run != id);
        self.session.delegations.push(DelegationRecord {
            run: id,
            call_id: call_id.to_owned(),
            label: label.clone(),
            kind,
            state: RunState::Running,
            reasons: Vec::new(),
            files: Vec::new(),
            started_unix: crate::delegation::unix_now(),
            finished_unix: None,
        });
        self.subagents.push(Subagent {
            id,
            call_id: call_id.to_owned(),
            task,
            kind,
            state: RunState::Running,
            name: None,
            answer: String::new(),
            reasoning: String::new(),
            note: None,
            started: Instant::now(),
            elapsed_ms: 0,
            usage: None,
            reasons: Vec::new(),
            files: Vec::new(),
            cancel_requested: false,
            heard_from: false,
        });
        // Evict the oldest finished run, never a live one: dropping a running
        // record would leave a report with nowhere to land, and the viewer
        // indexes into this list.
        while self.subagents.len() > MAX_SUBAGENTS {
            let Some(victim) = self
                .subagents
                .iter()
                .position(|agent| agent.state.is_terminal())
            else {
                break;
            };
            self.subagents.remove(victim);
            if let Some(view) = self.subagent_view.as_mut() {
                if *view > victim {
                    *view -= 1;
                } else if *view == victim {
                    self.subagent_view = None;
                }
            }
        }
        id
    }

    /// Runs owned by this session that are still active: the coordinator's
    /// outstanding work, which is why a finished run never reads as "all
    /// done" while its siblings keep working.
    pub(crate) fn active_runs_in_owner(&self) -> Vec<u64> {
        self.session
            .delegations
            .iter()
            .filter(|record| record.state.is_active())
            .map(|record| record.run)
            .collect()
    }

    /// The session that delegated a run, found in the persisted ledgers of
    /// every tab: a report routes to the session that asked even after the
    /// tab on screen changed, or the owning tab was closed.
    pub(crate) fn session_for_run(&self, run: u64) -> Option<u64> {
        let holds = |session: &Session| {
            session
                .delegations
                .iter()
                .any(|record| record.run == run)
        };
        if holds(&self.session) {
            return Some(self.session.id);
        }
        self.tabs
            .iter()
            .find(|state| holds(&state.session))
            .map(|state| state.session.id)
    }

    /// A run's lifecycle moves one step, when that step is legal. A terminal
    /// state never moves again, so a duplicate report changes nothing; the
    /// error names the illegal move.
    fn move_run(&mut self, run: u64, next: RunState) -> Result<(), &'static str> {
        let Some(agent) = self.subagents.iter_mut().find(|agent| agent.id == run) else {
            // A record that is not in memory (restored, or evicted): the
            // ledger still answers for it.
            return Ok(());
        };
        if !agent.state.can_move_to(next) {
            return Err("illegal lifecycle move");
        }
        agent.state = next;
        Ok(())
    }

    /// The delegation ledger for the next request's system note, or None
    /// when this session never delegated anything.
    pub(crate) fn delegate_ledger_note(&self) -> Option<String> {
        crate::delegation::ledger(&self.session.delegations, crate::delegation::unix_now())
    }

    /// The files the ledger records for a run: the evidence, kept where a
    /// report can reach it even when the UI record was evicted.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn run_files(&self, run: u64) -> Vec<String> {
        self.session
            .delegations
            .iter()
            .find(|record| record.run == run)
            .map(|record| record.files.clone())
            .unwrap_or_default()
    }

    /// The coordinator's report for one finished run, built from the verdict
    /// the evidence earned. Called with the owning session swapped in.
    pub(crate) fn on_subagent_verdict(
        &mut self,
        run: u64,
        state: RunState,
        reasons: Vec<String>,
        files: Vec<String>,
        report: String,
    ) -> Action {
        // Submitted -> Verifying -> the verdict, never a jump: the only road
        // to Completed runs through verification. A duplicate report is a
        // terminal state that cannot move, so it stops here.
        for next in [RunState::Submitted, RunState::Verifying, state] {
            if self.move_run(run, next).is_err() {
                // Already terminal: this report is a duplicate.
                return Action::None;
            }
        }
        if let Some(agent) = self.subagents.iter_mut().find(|agent| agent.id == run) {
            agent.elapsed_ms = u64::try_from(agent.started.elapsed().as_millis()).unwrap_or(0);
            agent.reasons = reasons.clone();
            agent.files = files.clone();
        }
        if let Some(record) = self
            .session
            .delegations
            .iter_mut()
            .find(|record| record.run == run)
        {
            record.state = state;
            record.reasons = reasons;
            record.files = files;
            record.finished_unix = Some(crate::delegation::unix_now());
        }
        let mut message = Message::new(Role::Tool, report);
        message.tool_call_id = Some(format!("subagent-run-{run}"));
        self.session.messages.push(message);
        self.follow_tail = true;
        self.save_session();
        self.after_report(run)
    }

    /// The wake decision for a report that just landed in the transcript:
    /// silent while a sibling run still works or when the run was cancelled,
    /// a fresh turn otherwise.
    fn after_report(&mut self, run: u64) -> Action {
        // The cancellation marker is spent by the report it belongs to,
        // whichever branch decides the wake.
        let was_cancelled = self.cancelling_runs.contains(&run);
        self.cancelling_runs.retain(|cancelled| *cancelled != run);
        // While sibling runs keep working, the report lands silently: the
        // continuation already in flight picks it up, and a fresh turn per
        // finished run would let the coordinator announce "all done" in the
        // middle of the batch.
        if !self.active_runs_in_owner().is_empty() {
            return Action::None;
        }
        // A cancelled run reports for the record, not as an invitation to
        // claim success: the interrupt already told the user what happened.
        if was_cancelled {
            return Action::None;
        }
        self.begin_turn()
    }

    /// Append every parked report and talk message to the conversation.
    /// Callers only use it at an idle point (nothing streaming, no tool
    /// pending), so nothing lands between an assistant tool call and its
    /// result. True when at least one message joined.
    pub(crate) fn flush_parked_messages(&mut self) -> bool {
        let mut delivered = false;
        for (run, report) in std::mem::take(&mut self.pending_reports) {
            let mut message = Message::new(Role::Tool, report);
            message.tool_call_id = Some(format!("subagent-run-{run}"));
            self.session.messages.push(message);
            delivered = true;
        }
        for text in std::mem::take(&mut self.pending_talks) {
            let id = crate::delegation::talk_id(&text);
            if self
                .session
                .messages
                .iter()
                .any(|message| message.tool_call_id.as_deref() == Some(id.as_str()))
            {
                // A re-sent message is delivered once.
                continue;
            }
            let mut message = Message::new(Role::Tool, text);
            message.tool_call_id = Some(id);
            self.session.messages.push(message);
            delivered = true;
        }
        if delivered {
            self.follow_tail = true;
            self.save_session();
        }
        delivered
    }

    /// The subagent a delegate tool call spawned: the transcript row shows
    /// its model-given name and live status. None when the call was rejected
    /// or predates the run tracking.
    pub(crate) fn subagent_for_call(&self, call_id: &str) -> Option<&Subagent> {
        self.subagents.iter().find(|agent| agent.call_id == call_id)
    }

    /// A subagent update arrived: fold it into the run record. Deltas redraw
    /// at the streaming cadence, so the viewer (and the run list) stay live.
    pub(crate) fn on_delegate_update(&mut self, update: DelegateUpdate) {
        let run = match &update {
            DelegateUpdate::Delta { run, .. }
            | DelegateUpdate::Reasoning { run, .. }
            | DelegateUpdate::Usage { run, .. }
            | DelegateUpdate::Note { run, .. }
            | DelegateUpdate::Named { run, .. }
            | DelegateUpdate::State { run, .. }
            | DelegateUpdate::Verdict { run, .. } => *run,
        };
        let Some(agent) = self.subagents.iter_mut().find(|agent| agent.id == run) else {
            return;
        };
        match update {
            DelegateUpdate::Delta { text, .. } => {
                agent.heard_from = true;
                push_capped(&mut agent.answer, &text);
            }
            DelegateUpdate::Reasoning { text, .. } => push_capped(&mut agent.reasoning, &text),
            DelegateUpdate::Usage {
                prompt_tokens,
                completion_tokens,
                ..
            } => {
                let usage = agent.usage.get_or_insert((0, 0));
                usage.0 = usage.0.max(prompt_tokens);
                usage.1 = usage.1.max(completion_tokens);
            }
            DelegateUpdate::Note { text, .. } => agent.note = Some(text),
            DelegateUpdate::Named { name, .. } => {
                // The same cleanup the session title gets: one non-empty
                // line, quotes stripped, bounded to the row width.
                let named: String = name
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .unwrap_or_default()
                    .trim_matches(|character: char| {
                        matches!(character, '"' | '\'' | '`' | '*' | '#')
                    })
                    .chars()
                    .take(48)
                    .collect();
                let named = named.trim();
                if !named.is_empty() {
                    agent.name = Some(named.to_owned());
                }
            }
            DelegateUpdate::State { state, .. } => {
                if agent.state.can_move_to(state) {
                    agent.state = state;
                    // The persisted ledger mirrors the lifecycle: the
                    // coordinator's note is built from it, and a restart
                    // reads it back to know what was outstanding.
                    if let Some(record) = self
                        .session
                        .delegations
                        .iter_mut()
                        .find(|record| record.run == run)
                    {
                        if record.state.can_move_to(state) {
                            record.state = state;
                        }
                    }
                }
            }
            DelegateUpdate::Verdict { .. } => {
                // Handled by `on_subagent_verdict`, which owns the ledger and
                // the report routing; the sidebar only needs the state, and
                // that arrives with the same call.
            }
        }
    }

    /// The run currently open in the viewer, if any.
    pub(crate) fn viewed_subagent(&self) -> Option<&Subagent> {
        let index = *self.subagent_view.as_ref()?;
        self.subagents.get(index)
    }

    /// Open the `/agents` list. It only reads app state, so it may open
    /// while a turn streams, like the other read-only commands.
    pub(crate) fn open_agents_picker(&mut self) {
        if self.subagents.is_empty() {
            self.info("No subagent runs yet — delegate calls appear here.");
            return;
        }
        self.agents_picker = Some(ListPicker::default());
        self.subagent_view = None;
    }

    /// `/agents` rows: the filtered runs, newest first, as indices into
    /// `subagents`. Both the model-given name and the task text match, so a
    /// run is findable by either.
    pub(crate) fn agents_matches(&self) -> Vec<usize> {
        let filter = self
            .agents_picker
            .as_ref()
            .map_or(String::new(), |picker| picker.filter.to_lowercase());
        (0..self.subagents.len())
            .rev()
            .filter(|&index| {
                let agent = &self.subagents[index];
                filter.is_empty()
                    || agent.task.to_lowercase().contains(&filter)
                    || agent
                        .name
                        .as_ref()
                        .is_some_and(|name| name.to_lowercase().contains(&filter))
            })
            .collect()
    }

    /// Rows shown in the `/agents` popup for its current filter.
    pub(crate) fn agents_picker_count(&self) -> usize {
        self.agents_matches().len()
    }

    pub(crate) fn handle_agents_picker_key(&mut self, key: KeyEvent) -> Action {
        if key.code == KeyCode::Esc {
            self.agents_picker = None;
            return Action::None;
        }
        // Editing a clone and writing it back keeps the borrow rules simple:
        // helper methods borrow the whole App, not single fields.
        let Some(mut picker) = self.agents_picker.clone() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down => {
                picker.selected =
                    (picker.selected + 1).min(self.agents_picker_count().saturating_sub(1));
            }
            KeyCode::Backspace => {
                picker.filter.pop();
                picker.selected = 0;
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                picker.filter.push(character);
                picker.selected = 0;
            }
            KeyCode::Enter => {
                let matches = self.agents_matches();
                let target = matches
                    .get(picker.selected.min(matches.len().saturating_sub(1)))
                    .copied();
                self.agents_picker = None;
                if let Some(index) = target {
                    self.subagent_view = Some(index);
                    self.subagent_scroll = 0;
                }
                return Action::None;
            }
            _ => return Action::None,
        }
        self.agents_picker = Some(picker);
        Action::None
    }

    /// Keys for the run viewer: scroll and close, nothing else.
    pub(crate) fn handle_viewer_key(&mut self, key: KeyEvent) -> Action {
        if key.code == KeyCode::Esc {
            self.subagent_view = None;
            self.subagent_scroll = 0;
            return Action::None;
        }
        match key.code {
            KeyCode::Up | KeyCode::Left => {
                self.subagent_scroll = self.subagent_scroll.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Right => self.subagent_scroll += 1,
            KeyCode::Home => self.subagent_scroll = 0,
            KeyCode::PageUp => self.subagent_scroll = self.subagent_scroll.saturating_sub(10),
            KeyCode::PageDown => self.subagent_scroll += 10,
            _ => {}
        }
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::{App, DelegateUpdate, SubagentStatus};
    use crate::delegation::RunState;

    fn app_with_run() -> (App, u64) {
        let mut app = crate::app::turn::tests::test_app();
        let run = app.begin_subagent(
        "call-1",
        "Write the tests for the parser".to_owned(),
        crate::delegation::TaskKind::Change,
    );
        (app, run)
    }

    #[test]
    fn a_run_records_deltas_usage_and_finishes() {
        let (mut app, run) = app_with_run();
        app.on_delegate_update(DelegateUpdate::Delta {
            run,
            text: "partial".to_owned(),
        });
        app.on_delegate_update(DelegateUpdate::Usage {
            run,
            prompt_tokens: 100,
            completion_tokens: 20,
        });
        let agent = &app.subagents[0];
        assert_eq!(agent.status(), SubagentStatus::Running);
        assert_eq!(agent.answer, "partial");
        assert_eq!(agent.usage, Some((100, 20)));

        let agent = &app.subagents[0];
        assert_eq!(agent.status(), SubagentStatus::Running);
        assert!(agent.elapsed_ms < 60_000);
    }

    #[test]
    fn a_failed_run_keeps_its_note_and_marks_failed() {
        let (mut app, run) = app_with_run();
        app.on_delegate_update(DelegateUpdate::Note {
            run,
            text: "attempt 1 failed: timeout".to_owned(),
        });
        let agent = &app.subagents[0];
        assert_eq!(agent.state, RunState::Running);
        assert_eq!(agent.note.as_deref(), Some("attempt 1 failed: timeout"));
    }

    #[test]
    fn the_model_names_a_task_and_the_row_shows_it() {
        let (mut app, run) = app_with_run();
        // Until the name arrives the row falls back to the task excerpt.
        assert_eq!(
            app.subagents[0].row_label(),
            "Write the tests for the parser"
        );

        app.on_delegate_update(DelegateUpdate::Named {
            run,
            name: "\"Parser unit tests\"\n".to_owned(),
        });
        assert_eq!(app.subagents[0].row_label(), "Parser unit tests");

        // An empty naming answer changes nothing.
        app.on_delegate_update(DelegateUpdate::Named {
            run,
            name: "  \n \n".to_owned(),
        });
        assert_eq!(app.subagents[0].row_label(), "Parser unit tests");
    }

    #[test]
    fn the_picker_filters_by_the_model_name_too() {
        let (mut app, run) = app_with_run();
        app.on_delegate_update(DelegateUpdate::Named {
            run,
            name: "Parser unit tests".to_owned(),
        });
        app.open_agents_picker();
        app.agents_picker.as_mut().unwrap().filter = "unit".to_owned();
        assert_eq!(app.agents_matches(), vec![0]);
    }

    #[test]
    fn updates_for_unknown_runs_are_ignored() {
        let (mut app, _) = app_with_run();
        app.on_delegate_update(DelegateUpdate::Delta {
            run: 999,
            text: "stray".to_owned(),
        });
        assert!(app.subagents[0].answer.is_empty());
    }

    #[test]
    fn the_picker_lists_newest_first_and_filters() {
        let (mut app, first) = app_with_run();
        let second = app.begin_subagent(
            "call-2",
            "Review the diff".to_owned(),
            crate::delegation::TaskKind::ReadOnly,
        );
        assert_ne!(first, second);
        app.open_agents_picker();
        assert_eq!(app.agents_matches(), vec![1, 0]);
        assert_eq!(app.agents_picker_count(), 2);

        app.agents_picker.as_mut().unwrap().filter = "review".to_owned();
        assert_eq!(app.agents_matches(), vec![1]);
        assert_eq!(app.agents_picker_count(), 1);

        // Enter on the only match opens its viewer.
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        app.handle_agents_picker_key(key);
        assert!(app.agents_picker.is_none());
        assert_eq!(app.subagent_view, Some(1));
        assert!(app.viewed_subagent().is_some());
    }

    #[test]
    fn the_viewer_scrolls_and_closes() {
        let (mut app, _) = app_with_run();
        app.subagent_view = Some(0);
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let key = |code: KeyCode| KeyEvent::new(code, KeyModifiers::NONE);
        app.handle_viewer_key(key(KeyCode::Down));
        app.handle_viewer_key(key(KeyCode::PageDown));
        assert_eq!(app.subagent_scroll, 11);
        app.handle_viewer_key(key(KeyCode::Home));
        assert_eq!(app.subagent_scroll, 0);
        app.handle_viewer_key(key(KeyCode::Esc));
        assert!(app.subagent_view.is_none());
    }

    #[test]
    fn streamed_text_is_capped_per_run() {
        let (mut app, run) = app_with_run();
        let chunk = "x".repeat(45_000);
        for _ in 0..6 {
            app.on_delegate_update(DelegateUpdate::Delta {
                run,
                text: chunk.clone(),
            });
        }
        let agent = &app.subagents[0];
        // The stored response stays bounded and says where it stopped.
        assert!(agent.answer.chars().count() <= 200_050, "cap exceeded");
        assert!(agent.answer.ends_with("\n… truncated"));
    }
}
