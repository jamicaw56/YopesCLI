//! Delivering talk between sessions: the app loop owns every tab, so the
//! talk tools send their requests here.
//!
//! `check_session` is answered from a live snapshot of the tabs. `talk_to`
//! resolves the tab number, parks the message while that tab is busy, and
//! delivers it at once when the tab is idle — waking its model with a fresh
//! turn, the same machinery a finished subagent report uses. The parked
//! messages ride `flush_parked_messages` at the tab's next idle point.

use std::sync::mpsc;

use super::*;

impl App {
    /// Answer one talk request from a tool call. Returns the tab and the
    /// action to dispatch when a message woke an idle tab: the caller runs
    /// the action with that tab swapped in, so the wake uses the tab's own
    /// provider and working directory.
    pub(crate) fn on_talk_request(
        &mut self,
        request: tools::TalkRequest,
    ) -> Option<(usize, Action)> {
        match request {
            tools::TalkRequest::List { from, reply } => {
                let _ = reply.send(self.talk_directory(from));
                None
            }
            tools::TalkRequest::Send {
                from,
                tab,
                message,
                reply,
            } => self.talk_deliver(from, tab, message, reply),
        }
    }

    /// The directory `check_session` returns: every open tab with its
    /// number, title, and whether it is mid-turn. `from` marks the caller's
    /// own row.
    fn talk_directory(&self, from: u64) -> String {
        if self.tabs.len() == 1 {
            return "Only this session is open (tab 1). Open another tab (alt+w) \
                    to talk between sessions."
                .to_owned();
        }
        let mut lines =
            vec!["Open sessions; address one by its tab number with talk_to:".to_owned()];
        for index in 0..self.tabs.len() {
            let mine = self.tab_for_session(from) == Some(index);
            let title = self.session_title_of(index);
            let busy = self.tab_busy(index) || self.tab_streaming(index);
            let marker = if mine { " — this session" } else { "" };
            lines.push(format!(
                "{}: {} ({}{})",
                index + 1,
                title,
                if busy { "busy" } else { "idle" },
                marker
            ));
        }
        lines.join("\n")
    }

    /// The full title of tab `index`, trimmed like the transcript shows it.
    fn session_title_of(&self, index: usize) -> String {
        let title = if index == self.tab {
            self.session.title.trim()
        } else {
            self.tabs[index].session.title.trim()
        };
        title.to_owned()
    }

    /// Resolve a `talk_to` request: validate the tab, park the message while
    /// the target is busy or deliver it at once, and answer the tool call
    /// with what happened.
    fn talk_deliver(
        &mut self,
        from: u64,
        tab: usize,
        message: String,
        reply: mpsc::Sender<String>,
    ) -> Option<(usize, Action)> {
        let Some(index) = tab.checked_sub(1).filter(|index| *index < self.tabs.len()) else {
            let _ = reply.send(format!(
                "No tab {tab}. check_session lists the open tabs you can talk to."
            ));
            return None;
        };
        if self.tab_for_session(from) == Some(index) {
            let _ = reply.send(format!("Tab {tab} is this session; talk_to another tab."));
            return None;
        }
        // The delivery names the sender's tab, so the receiving model can
        // address its reply back. A session id that matches no open tab
        // (a tool that ran unscoped) still delivers, as "another session".
        let sender = match self.tab_for_session(from) {
            Some(sender_index) => {
                format!("tab {} — {}", sender_index + 1, self.session_title_of(sender_index))
            }
            None => "another session".to_owned(),
        };
        let title = self.session_title_of(index);
        let text = format!("[talk from {sender}] {message}");
        let (parked, wake) = self.with_tab(index, move |app| {
            if app.is_busy() {
                // Pushing into a streaming conversation would swallow the
                // deltas; the parked message lands at the next idle point
                // and wakes the model then.
                app.pending_talks.push_back(text);
                (true, None)
            } else {
                let action = app.deliver_talk(text);
                (false, Some(action))
            }
        });
        let _ = reply.send(if parked {
            format!(
                "Delivered to tab {tab} — {title}. The tab is busy: the message waits and \
                 lands when its current turn ends."
            )
        } else {
            format!(
                "Delivered to tab {tab} — {title}. The tab was idle: its model wakes now, \
                 reads the message, and can reply with talk_to."
            )
        });
        wake.map(|action| (index, action))
    }

    /// Push one talk message into this tab's conversation and start a turn,
    /// so the model reacts to it right away. The unmatched tool-call id makes
    /// the serializer send it as a `Tool result:` user message, the same way
    /// a background bash job reports.
    fn deliver_talk(&mut self, text: String) -> Action {
        let mut message = Message::new(Role::Tool, text);
        message.tool_call_id = Some(format!("talk-{}", self.session.messages.len()));
        self.session.messages.push(message);
        self.follow_tail = true;
        self.save_session();
        self.begin_turn()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn test_app() -> App {
        crate::app::turn::tests::test_app()
    }

    /// Ask `app` to deliver one talk message and return the tool result plus
    /// the wake action, when the message woke the receiving tab.
    fn deliver(app: &mut App, from: u64, tab: usize, message: &str) -> (String, Option<Action>) {
        let (reply, receiver) = mpsc::channel();
        let wake = app.on_talk_request(tools::TalkRequest::Send {
            from,
            tab,
            message: message.to_owned(),
            reply,
        });
        let answer = receiver.try_recv().expect("the request was answered");
        let wake = wake.map(|(owner, action)| {
            assert_eq!(owner, tab - 1, "the wake names the receiving tab");
            action
        });
        (answer, wake)
    }

    #[test]
    fn the_directory_lists_every_tab_with_its_number() {
        let mut app = test_app();
        let from = app.session.id;
        assert!(app.talk_directory(from).contains("Only this session is open"));
        app.open_tab();
        // open_tab leaves the new tab on screen; name the original one.
        app.with_tab(0, |tab| tab.session.title = "Plan the parser".to_owned());
        let directory = app.talk_directory(from);
        assert!(directory.contains("1: Plan the parser (idle) — this session"), "{directory}");
        assert!(directory.contains("2: New session (idle)"), "{directory}");

        // A busy background tab reads as busy.
        app.with_tab(1, |tab| {
            tab.running_tool = Some(RunningTool {
                call: ToolCall::new("call_1", "list_dir", "{}"),
                started: Instant::now(),
                before: None,
                existed: false,
                targets: Vec::new(),
            });
        });
        let directory = app.talk_directory(from);
        assert!(directory.contains("2: New session (busy)"), "{directory}");
    }

    #[test]
    fn a_message_wakes_an_idle_tab_at_once() {
        let mut app = test_app();
        // The sender id comes before open_tab, which leaves the new tab on
        // screen as the talk target.
        let from = app.session.id;
        app.open_tab();
        let (answer, action) = deliver(&mut app, from, 2, "hello from tab 1");
        let action = action.expect("an idle tab is woken");
        assert!(answer.contains("The tab was idle"), "{answer}");
        assert!(matches!(action, Action::Start { .. }));
        // The message landed in the receiving tab's conversation, named by
        // the sender's tab. The empty reply begin_request pushes sits after
        // it, so the check walks the messages instead of the tail.
        app.with_tab(1, |tab| {
            assert!(tab.session.messages.iter().any(|message| {
                message.content.contains("[talk from tab 1 — New session] hello from tab 1")
            }));
        });
    }

    #[test]
    fn a_message_parks_while_the_tab_is_busy_and_lands_at_the_next_idle_point() {
        let mut app = test_app();
        let from = app.session.id;
        app.open_tab();
        app.with_tab(1, |tab| {
            tab.running_tool = Some(RunningTool {
                call: ToolCall::new("call_1", "list_dir", "{}"),
                started: Instant::now(),
                before: None,
                existed: false,
                targets: Vec::new(),
            });
        });
        let (answer, action) = deliver(&mut app, from, 2, "ping");
        assert!(action.is_none(), "a busy tab is not woken");
        assert!(answer.contains("The tab is busy"), "{answer}");
        app.with_tab(1, |tab| assert_eq!(tab.pending_talks.len(), 1));

        // At the next idle point the parked message lands and the model is
        // alerted with a fresh turn.
        app.with_tab(1, |tab| {
            tab.running_tool = None;
            assert!(tab.flush_parked_messages());
            assert!(tab.pending_talks.is_empty());
            assert!(tab.session.messages.iter().any(|message| {
                message.content.contains("[talk from tab 1 — New session] ping")
            }));
        });
    }

    #[test]
    fn talking_to_a_missing_tab_or_this_session_is_rejected() {
        let mut app = test_app();
        let from = app.session.id;
        app.open_tab();
        let (answer, action) = deliver(&mut app, from, 3, "hi");
        assert!(answer.contains("No tab 3"), "{answer}");
        assert!(action.is_none());
        // Tab 1 is the sending session itself.
        let (answer, action) = deliver(&mut app, from, 1, "hi");
        assert!(answer.contains("is this session"), "{answer}");
        assert!(action.is_none());
    }
}
