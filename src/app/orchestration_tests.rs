//! Orchestration scenarios, run against the real app state machine: several
//! runs at once, a run that lies about being done, a run that times out, a
//! duplicate report, a cancel, tab switching mid-flight and a restart with
//! outstanding work.
//!
//! The verdicts and evidence feeding these tests come from `crate::delegation`,
//! so the scenarios exercise the same judgement the run loop uses.

#[cfg(test)]
mod tests {
    use crate::app::{Action, App, DelegateUpdate};
    use crate::delegation::{
        self, CommandRecord, Evidence, FileChange, RunOutcome, RunState, TaskKind, Verdict,
    };

    fn app() -> App {
        crate::app::turn::tests::test_app()
    }

    fn start(app: &mut App, call_id: &str, task: &str, kind: TaskKind) -> u64 {
        app.begin_subagent(call_id, task.to_owned(), kind)
    }

    /// A change that really happened, as the run loop records it.
    fn changed(path: &str) -> FileChange {
        FileChange {
            path: path.to_owned(),
            additions: 4,
            ..FileChange::default()
        }
    }

    /// The verdict for a run that changed `files` and said `answer`.
    fn judged(kind: TaskKind, files: &[&str], answer: &str) -> Verdict {
        let mut evidence = Evidence::default();
        for path in files {
            evidence.record_change(changed(path));
        }
        let outcome = RunOutcome {
            evidence,
            answer: answer.to_owned(),
            ..RunOutcome::default()
        };
        delegation::verdict(kind, &[], &outcome)
    }

    /// Deliver a verdict exactly as the event loop does, with the evidence's
    /// file list read from the ledger the loop writes it into.
    fn report(app: &mut App, run: u64, verdict: Verdict, answer: &str) -> Action {
        let files = app.run_files(run);
        deliver(app, run, verdict, files, answer)
    }

    fn deliver(
        app: &mut App,
        run: u64,
        verdict: Verdict,
        files: Vec<String>,
        answer: &str,
    ) -> Action {
        let outcome = RunOutcome {
            answer: answer.to_owned(),
            ..RunOutcome::default()
        };
        let text = delegation::report(
            run,
            "task",
            TaskKind::Change,
            &verdict,
            &outcome,
            std::time::Duration::from_secs(5),
            &[],
        );
        app.on_subagent_verdict(run, verdict.state, verdict.reasons, files, text)
    }

    #[test]
    fn several_runs_report_in_turn_and_only_the_last_one_wakes_the_model() {
        let mut app = app();
        let first = start(&mut app, "c1", "port the parser", TaskKind::Change);
        let second = start(&mut app, "c2", "write the tests", TaskKind::Change);
        assert_eq!(app.active_runs_in_owner(), vec![first, second]);

        // The first run finishes while its sibling still works: the report
        // lands, but the model is not woken to announce a partial batch.
        let action = report(&mut app, first, judged(TaskKind::Change, &["a.rs"], "landed"), "landed");
        assert!(matches!(action, Action::None), "a sibling run is still active");
        assert_eq!(app.session.delegations[0].state, RunState::Completed);
        assert_eq!(app.active_runs_in_owner(), vec![second]);
        assert!(app.session.messages.iter().any(|message| message
            .content
            .contains("verdict: COMPLETED")));

        // The last one wakes the coordinator.
        let action = report(&mut app, second, judged(TaskKind::Change, &["b.rs"], "done"), "done");
        assert!(matches!(action, Action::Start { .. }), "the batch is done");
        assert!(app.active_runs_in_owner().is_empty());
    }

    #[test]
    fn an_agent_that_claims_completion_without_changing_files_fails() {
        let mut app = app();
        let run = start(&mut app, "c1", "implement the parser", TaskKind::Change);
        let verdict = judged(TaskKind::Change, &[], "Implemented everything, all tests pass!");
        assert_eq!(verdict.state, RunState::Failed);
        let action = report(&mut app, run, verdict, "Implemented everything, all tests pass!");
        assert!(matches!(action, Action::Start { .. }));
        assert_eq!(app.session.delegations[0].state, RunState::Failed);
        assert_eq!(app.subagents[0].status_label(), "failed");
        // The coordinator's own record carries the reason, not the claim.
        let report = app
            .session
            .messages
            .iter()
            .rev()
            .find(|message| message.content.contains("[delegation report]"))
            .expect("a report landed");
        assert!(report.content.contains("changed no files"), "{}", report.content);
        assert!(report.content.contains("a claim, not evidence"));
    }

    #[test]
    fn an_agent_that_skips_an_expected_file_fails() {
        let mut app = app();
        let run = start(&mut app, "c1", "port", TaskKind::Change);
        let mut evidence = Evidence::default();
        evidence.record_change(changed("src/other.rs"));
        let outcome = RunOutcome {
            evidence,
            answer: "ported".to_owned(),
            ..RunOutcome::default()
        };
        let verdict = delegation::verdict(
            TaskKind::Change,
            &["src/parser.rs".to_owned()],
            &outcome,
        );
        assert_eq!(verdict.state, RunState::Failed);
        report(&mut app, run, verdict, "ported");
        assert_eq!(app.session.delegations[0].state, RunState::Failed);
        assert!(app.session.delegations[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("src/parser.rs")));
    }

    #[test]
    fn a_run_that_timed_out_reports_failed_and_keeps_its_partial_files() {
        let mut app = app();
        let run = start(&mut app, "c1", "long job", TaskKind::Change);
        let mut evidence = Evidence::default();
        evidence.record_change(changed("src/half.rs"));
        let outcome = RunOutcome {
            evidence,
            answer: "still working".to_owned(),
            timed_out: Some(std::time::Duration::from_secs(45 * 60)),
            ..RunOutcome::default()
        };
        let verdict = delegation::verdict(TaskKind::Change, &[], &outcome);
        assert_eq!(verdict.state, RunState::Failed);
        // The run loop sends the evidence's file list with the verdict.
        let files = outcome.evidence.changed_paths();
        deliver(&mut app, run, verdict, files, "still working");
        let record = &app.session.delegations[0];
        assert_eq!(record.state, RunState::Failed);
        assert_eq!(record.files, vec!["src/half.rs".to_owned()]);
        assert!(
            record.reasons.iter().any(|reason| reason.contains("timed out")),
            "{:?}",
            record.reasons
        );
    }

    #[test]
    fn one_agent_failing_midway_does_not_touch_its_siblings() {
        let mut app = app();
        let first = start(&mut app, "c1", "a", TaskKind::Change);
        let second = start(&mut app, "c2", "b", TaskKind::ReadOnly);
        let mut outcome = RunOutcome {
            answer: "half".to_owned(),
            failure: Some("stream error after every retry".to_owned()),
            ..RunOutcome::default()
        };
        outcome.evidence.record_change(changed("src/partial.rs"));
        let verdict = delegation::verdict(TaskKind::Change, &[], &outcome);
        report(&mut app, first, verdict, "half");
        assert_eq!(app.session.delegations[0].state, RunState::Failed);
        assert_eq!(app.session.delegations[1].state, RunState::Running);
        assert_eq!(app.subagents[1].status_label(), "running");
        assert_eq!(app.active_runs_in_owner(), vec![second]);
    }

    #[test]
    fn a_duplicate_report_is_ignored() {
        let mut app = app();
        let run = start(&mut app, "c1", "a", TaskKind::Change);
        report(&mut app, run, judged(TaskKind::Change, &["a.rs"], "done"), "done");
        let messages = app.session.messages.len();
        // The same verdict arrives again: no second message, no second turn.
        let action = report(&mut app, run, judged(TaskKind::Change, &["a.rs"], "done"), "done");
        assert!(matches!(action, Action::None));
        assert_eq!(app.session.messages.len(), messages);
    }

    #[test]
    fn only_a_verdict_finishes_a_run() {
        let mut app = app();
        let run = start(&mut app, "c1", "a", TaskKind::Change);
        // The loop's request ended, and it moved the run to Submitted: still
        // not finished, so the coordinator cannot read it as done.
        app.on_delegate_update(DelegateUpdate::State {
            run,
            state: RunState::Submitted,
        });
        assert_eq!(app.session.delegations[0].state, RunState::Submitted);
        assert!(app.subagents[0].state.is_active());
        // A submission is not a finish: the run stays active until a verdict.
        assert!(app.subagents[0].state.is_active());
        // A terminal state is not reachable in one jump.
        app.on_delegate_update(DelegateUpdate::State {
            run,
            state: RunState::Completed,
        });
        assert_ne!(app.session.delegations[0].state, RunState::Completed);
    }

    #[test]
    fn cancelling_the_owner_stops_its_runs_and_keeps_the_reports_silent() {
        let mut app = app();
        let first = start(&mut app, "c1", "a", TaskKind::Change);
        let second = start(&mut app, "c2", "b", TaskKind::Change);
        assert_eq!(app.active_runs_in_owner(), vec![first, second]);
        app.cancel_owned_runs();
        assert!(app.subagents.iter().all(|agent| agent.cancel_requested));
        assert_eq!(app.cancelling_runs, vec![first, second]);
        // The cancelled reports land in the transcript for the record, but
        // the interrupt already spoke: no turn claims success.
        let mut outcome = RunOutcome {
            answer: "stopped".to_owned(),
            cancelled: true,
            ..RunOutcome::default()
        };
        outcome.evidence.record_change(changed("src/partial.rs"));
        let verdict = delegation::verdict(TaskKind::Change, &[], &outcome);
        let action = report(&mut app, first, verdict, "stopped");
        assert!(matches!(action, Action::None));
        assert_eq!(app.session.delegations[0].state, RunState::Cancelled);
        assert!(!app.cancelling_runs.contains(&first));
    }

    #[test]
    fn switching_tabs_while_a_run_is_active_keeps_the_report_in_the_owner() {
        let mut app = app();
        let owner = app.session.id;
        let run = start(&mut app, "c1", "a", TaskKind::Change);
        // The user opens another tab and works there: the run belongs to the
        // session that delegated it, found through the ledger, not the screen.
        app.open_tab();
        let on_screen = app.session.id;
        assert_ne!(owner, on_screen);
        assert_eq!(app.session_for_run(run), Some(owner));
        let index = app.tab_for_session(owner).expect("the owner is open");
        app.with_tab(index, |tab| {
            let action = report(tab, run, judged(TaskKind::Change, &["a.rs"], "done"), "done");
            assert!(matches!(action, Action::Start { .. }));
        });
        // The tab on screen never received it: the report is in the owner's
        // history only.
        let index = app.tab_for_session(owner).expect("the owner is open");
        app.with_tab(index, |tab| {
            assert!(
                tab.session
                    .messages
                    .iter()
                    .any(|message| message.content.contains("[delegation report]"))
            );
        });
        let other = app.tab_for_session(on_screen).expect("the other tab is open");
        app.with_tab(other, |tab| {
            assert!(
                !tab.session
                    .messages
                    .iter()
                    .any(|message| message.content.contains("[delegation report]")),
                "the report leaked into the tab on screen"
            );
        });
    }

    #[test]
    fn a_restored_session_marks_outstanding_runs_interrupted() {
        let mut app = app();
        let live = start(&mut app, "c1", "a", TaskKind::Change);
        let done = start(&mut app, "c2", "b", TaskKind::Change);
        report(&mut app, done, judged(TaskKind::Change, &["b.rs"], "done"), "done");

        // The app stops and the session comes back from disk.
        let dir = std::env::temp_dir().join(format!("yopes-restore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");
        app.session.save(&path).unwrap();
        let restored = crate::session::Session::load(&path).unwrap();

        let record = |run| {
            restored
                .delegations
                .iter()
                .find(|record| record.run == run)
                .expect("the ledger kept the run")
        };
        assert_eq!(record(live).state, RunState::Failed);
        assert_eq!(record(live).reasons, vec![delegation::INTERRUPTED.to_owned()]);
        assert_eq!(record(done).state, RunState::Completed);
        // The ledger the next request carries forbids presenting it as done.
        let text = delegation::ledger(&restored.delegations, delegation::unix_now()).unwrap();
        assert!(text.contains("interrupted"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_ledger_travels_in_every_request_while_runs_are_active() {
        let mut app = app();
        start(&mut app, "c1", "port the parser", TaskKind::Change);
        let note = app.delegate_ledger_note().expect("a ledger note");
        assert!(note.contains("still active"), "{note}");
        assert!(note.contains("port the parser"), "{note}");
        assert!(note.contains("do not tell the user the work is finished"));
        // Every finished run leaves no ledger at all.
        app.session.delegations.clear();
        assert!(app.delegate_ledger_note().is_none());
    }

    #[test]
    fn a_run_that_never_checks_in_is_still_tracked_and_never_finished() {
        let mut app = app();
        let run = start(&mut app, "c1", "a", TaskKind::Change);
        // No delta, no state move: the run is silent but active.
        assert_eq!(app.subagents[0].state, RunState::Running);
        assert!(!app.subagents[0].heard_from);
        assert_eq!(app.active_runs_in_owner(), vec![run]);
        // Only evidence changes that.
        report(&mut app, run, judged(TaskKind::Change, &["a.rs"], "done"), "done");
        assert_eq!(app.session.delegations[0].state, RunState::Completed);
    }

    #[test]
    fn a_write_that_changed_nothing_is_not_evidence() {
        let mut claim = RunOutcome {
            answer: "Updated config.rs.".to_owned(),
            ..RunOutcome::default()
        };
        claim.evidence.noop_writes = 1;
        let verdict = delegation::verdict(TaskKind::Change, &[], &claim);
        assert_eq!(verdict.state, RunState::Failed);
        assert!(verdict.reasons[0].contains("left the file unchanged"));
    }

    #[test]
    fn a_red_test_suite_fails_the_run_even_with_files_changed() {
        let mut outcome = RunOutcome {
            answer: "done".to_owned(),
            ..RunOutcome::default()
        };
        outcome.evidence.record_change(changed("src/lib.rs"));
        outcome.evidence.commands.push(CommandRecord::from_result(
            "cargo test",
            &Ok("Exit code: 101.\n2 passed; 1 failed".to_owned()),
        ));
        let verdict = delegation::verdict(TaskKind::Change, &[], &outcome);
        assert_eq!(verdict.state, RunState::Failed);
        assert!(verdict.reasons.iter().any(|reason| reason.contains("cargo test")));
    }

    #[test]
    fn a_read_only_task_completes_on_its_answer_and_never_on_files() {
        let findings = RunOutcome {
            answer: "The parser is in src/parse.rs.".to_owned(),
            ..RunOutcome::default()
        };
        assert_eq!(
            delegation::verdict(TaskKind::ReadOnly, &[], &findings).state,
            RunState::Completed
        );
        let silent = RunOutcome::default();
        assert_eq!(
            delegation::verdict(TaskKind::ReadOnly, &[], &silent).state,
            RunState::Failed
        );
    }
}
