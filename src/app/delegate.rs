//! The delegate system: subagent runs with their own conversations.
//!
//! A `delegate` tool call spawns one subagent run: a plain request on the
//! same provider and model, streamed live into `App::subagents` so the user
//! can watch the response while it runs. `/agents` lists the runs and opens
//! one for reading; the list and the viewer are app-global, so a run stays
//! viewable from every tab while its tool result lands in the owning tab.

use std::time::Instant;

use super::*;

/// The subagent brief: appended to the system prompt for every run.
pub(crate) const SUBAGENT_BRIEF: &str = "You are a subagent inside yopes, a terminal coding \
    assistant. Answer the task given below with only the useful result, no preamble, no offers \
    to help further. You have no tools; rely only on the context in the task.";

/// How many runs stay in memory: `/agents` shows the most recent ones.
const MAX_SUBAGENTS: usize = 50;
/// The longest streamed text kept per run, so one huge answer cannot eat
/// memory through deltas.
const MAX_STORED_CHARS: usize = 200_000;
/// Characters of the task shown on a picker row.
const TASK_EXCERPT: usize = 46;

/// The status of one subagent run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SubagentStatus {
    Running,
    Done,
    Failed,
}

/// One subagent run: the task, the streamed answer, and what it cost.
pub(crate) struct Subagent {
    pub(crate) id: u64,
    pub(crate) task: String,
    /// The response, streamed live while the run is in flight.
    pub(crate) answer: String,
    /// Reasoning streamed before the answer, when the provider sends it.
    pub(crate) reasoning: String,
    /// Retry or failure note, shown live in the viewer.
    pub(crate) note: Option<String>,
    pub(crate) started: Instant,
    /// Set once the run finished: how long it took, in milliseconds.
    pub(crate) elapsed_ms: u64,
    pub(crate) status: SubagentStatus,
    /// (prompt, completion) tokens billed, when the provider reported them.
    pub(crate) usage: Option<(u64, u64)>,
}

impl Subagent {
    pub(crate) fn status_label(&self) -> &'static str {
        match self.status {
            SubagentStatus::Running => "running",
            SubagentStatus::Done => "done",
            SubagentStatus::Failed => "failed",
        }
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
    /// The request ended: `failed` picks the final status. The answer text
    /// itself already streamed in through the deltas.
    Finished {
        run: u64,
        failed: bool,
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
    pub(crate) fn begin_subagent(&mut self, task: String) -> u64 {
        self.next_subagent_id = self.next_subagent_id.wrapping_add(1);
        let id = self.next_subagent_id;
        self.subagents.push(Subagent {
            id,
            task,
            answer: String::new(),
            reasoning: String::new(),
            note: None,
            started: Instant::now(),
            elapsed_ms: 0,
            status: SubagentStatus::Running,
            usage: None,
        });
        while self.subagents.len() > MAX_SUBAGENTS {
            self.subagents.remove(0);
        }
        id
    }

    /// A subagent update arrived: fold it into the run record. Deltas redraw
    /// at the streaming cadence, so the viewer (and the run list) stay live.
    pub(crate) fn on_delegate_update(&mut self, update: DelegateUpdate) {
        let run = match &update {
            DelegateUpdate::Delta { run, .. }
            | DelegateUpdate::Reasoning { run, .. }
            | DelegateUpdate::Usage { run, .. }
            | DelegateUpdate::Note { run, .. }
            | DelegateUpdate::Finished { run, .. } => *run,
        };
        let Some(agent) = self.subagents.iter_mut().find(|agent| agent.id == run) else {
            return;
        };
        match update {
            DelegateUpdate::Delta { text, .. } => push_capped(&mut agent.answer, &text),
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
            DelegateUpdate::Finished { failed, .. } => {
                agent.status = if failed {
                    SubagentStatus::Failed
                } else {
                    SubagentStatus::Done
                };
                agent.elapsed_ms = u64::try_from(agent.started.elapsed().as_millis()).unwrap_or(0);
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
    /// `subagents`.
    pub(crate) fn agents_matches(&self) -> Vec<usize> {
        let filter = self
            .agents_picker
            .as_ref()
            .map_or(String::new(), |picker| picker.filter.to_lowercase());
        (0..self.subagents.len())
            .rev()
            .filter(|&index| {
                filter.is_empty() || self.subagents[index].task.to_lowercase().contains(&filter)
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

    fn app_with_run() -> (App, u64) {
        let mut app = crate::app::turn::tests::test_app();
        let run = app.begin_subagent("Write the tests for the parser".to_owned());
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
        assert_eq!(agent.status, SubagentStatus::Running);
        assert_eq!(agent.answer, "partial");
        assert_eq!(agent.usage, Some((100, 20)));

        app.on_delegate_update(DelegateUpdate::Finished { run, failed: false });
        let agent = &app.subagents[0];
        assert_eq!(agent.status, SubagentStatus::Done);
        assert!(agent.elapsed_ms < 60_000);
    }

    #[test]
    fn a_failed_run_keeps_its_note_and_marks_failed() {
        let (mut app, run) = app_with_run();
        app.on_delegate_update(DelegateUpdate::Note {
            run,
            text: "attempt 1 failed: timeout".to_owned(),
        });
        app.on_delegate_update(DelegateUpdate::Finished { run, failed: true });
        let agent = &app.subagents[0];
        assert_eq!(agent.status, SubagentStatus::Failed);
        assert_eq!(agent.note.as_deref(), Some("attempt 1 failed: timeout"));
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
        let second = app.begin_subagent("Review the diff".to_owned());
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
