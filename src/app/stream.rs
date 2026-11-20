use super::*;

/// Sent after a reply that hit the output token cap: the model carries on from
/// the partial answer instead of starting over.
const CONTINUATION_PROMPT: &str = "Continue exactly where you stopped. Do not repeat text you \
already wrote; pick up mid-sentence if that is where the reply was cut.";

impl App {
    pub(crate) fn on_stream_event(&mut self, event: StreamEvent) -> Action {
        match event {
            StreamEvent::Delta { request_id, text }
                if self.active_request_id() == Some(request_id) =>
            {
                if let Some(compaction) = self
                    .active
                    .as_mut()
                    .and_then(|active| active.compacting.as_mut())
                {
                    compaction.text.push_str(&text);
                } else if let Some(message) = self.session.messages.last_mut() {
                    message.content.push_str(&text);
                }
                Action::None
            }
            StreamEvent::Reasoning { request_id, text }
                if self.active_request_id() == Some(request_id) =>
            {
                // A compaction summary is content only; reasoning is ignored.
                if self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.compacting.is_none())
                {
                    if let Some(message) = self.session.messages.last_mut() {
                        message.reasoning.push_str(&text);
                    }
                }
                Action::None
            }
            StreamEvent::ToolProgress {
                request_id,
                name,
                detail,
                bytes,
            } if self.active_request_id() == Some(request_id) => {
                if let Some(active) = self.active.as_mut() {
                    active.tool_progress = Some((name, detail, bytes));
                }
                Action::None
            }
            StreamEvent::Usage {
                request_id,
                prompt_tokens,
                completion_tokens,
            } if self.active_request_id() == Some(request_id) => {
                if let Some(active) = self.active.as_mut() {
                    let usage = active.usage.get_or_insert((0, 0));
                    usage.0 = usage.0.max(prompt_tokens);
                    usage.1 = usage.1.max(completion_tokens);
                }
                Action::None
            }
            StreamEvent::ToolCalls { request_id, calls }
                if self.active_request_id() == Some(request_id)
                    && self
                        .active
                        .as_ref()
                        .is_some_and(|active| active.compacting.is_none()) =>
            {
                let mut calls = calls;
                for call in &mut calls {
                    let name = crate::tools::canonical_tool_name(&call.function.name);
                    if name != call.function.name {
                        call.function.name = name.to_owned();
                    }
                }
                if let Some(message) = self.session.messages.last_mut() {
                    if message.role == Role::Assistant {
                        message.tool_calls = Some(calls.clone());
                    }
                }
                self.pending_tools.extend(calls);
                // No follow_tail snap: tool calls arriving must not yank a
                // user who scrolled up to read earlier replies.
                self.save_session();
                Action::None
            }
            StreamEvent::Retrying {
                request_id,
                attempt,
                delay_secs,
                reason,
            } if self.active_request_id() == Some(request_id) => {
                // The wait is shown in the footer, so a slow provider does not
                // look like a hang.
                self.retry = Some(RetryNotice {
                    attempt,
                    delay_secs,
                    reason,
                });
                Action::None
            }
            StreamEvent::Done { request_id } if self.active_request_id() == Some(request_id) => {
                self.retry = None;
                // Fold this request's billed tokens into the session total.
                if let Some((prompt, completion)) = self.active.as_ref().and_then(|a| a.usage) {
                    self.spent_prompt_tokens = self.spent_prompt_tokens.saturating_add(prompt);
                    self.spent_completion_tokens =
                        self.spent_completion_tokens.saturating_add(completion);
                }
                // A finished compaction folds the summary in, then the queued
                // turn starts against the now-small history.
                let compacted = self
                    .active
                    .as_ref()
                    .and_then(|active| active.compacting.as_ref())
                    .map(|compaction| {
                        (
                            compaction.split,
                            compaction.text.clone(),
                            compaction.attempts,
                        )
                    });
                if let Some((split, summary, attempts)) = compacted {
                    self.active = None;
                    self.markdown_cache.clear();
                    self.tool_frames.clear();
                    if summary.trim().is_empty() {
                        // An empty summary must not retry compaction: a
                        // provider that answers empty every time would loop
                        // the turn forever, and the turn never starts.
                        self.info("Compaction returned nothing; sending the full history.");
                        self.save_session();
                        let messages = self.session.messages.clone();
                        return self.begin_request(messages, None);
                    }
                    self.session.compact(split, summary);
                    self.save_session();
                    return self.begin_turn_after_compaction(attempts + 1);
                }
                // The turn chain finished cleanly: the next one starts with a
                // full stream-resume budget and a full continuation budget.
                self.resume_attempts = 0;
                self.continue_attempts = 0;
                // Record what the reply cost and how long it took, so the
                // transcript can show it under the finished message.
                let finished = self.active.as_ref().map(|active| {
                    let elapsed_ms = if self.pending_tools.is_empty() {
                        self.prompt_started.take().map_or_else(
                            || u64::try_from(active.started.elapsed().as_millis()).unwrap_or(0),
                            |started| {
                                u64::try_from(started.elapsed().as_millis())
                                    .unwrap_or(0)
                                    .max(1)
                            },
                        )
                    } else {
                        u64::try_from(active.started.elapsed().as_millis()).unwrap_or(0)
                    };
                    let (prompt_tokens, completion_tokens) = active
                        .usage
                        .unwrap_or((active.prompt_tokens, estimated_output_tokens(&self.session)));
                    (prompt_tokens, completion_tokens, elapsed_ms)
                });
                if let Some((prompt_tokens, completion_tokens, elapsed_ms)) = finished {
                    if let Some(message) = self.session.messages.last_mut() {
                        if message.role == Role::Assistant {
                            message.usage = Some(session::Usage {
                                prompt_tokens,
                                completion_tokens,
                                elapsed_ms,
                            });
                        }
                    }
                }
                self.active = None;
                if self.pending_tools.is_empty() {
                    // A provider can end the stream without sending anything;
                    // the placeholder is dropped, but the user is told why no
                    // reply appeared instead of seeing nothing.
                    if self.remove_empty_response() {
                        self.info(
                            "The model returned no content for this turn; try sending again.",
                        );
                    } else if self.session.messages.last().is_some_and(|last| {
                        last.role == Role::Assistant
                            && last.content.is_empty()
                            && last.tool_calls.is_none()
                            && last.error.is_none()
                    }) {
                        // Reasoning is kept, but a turn that thought and then
                        // ended with no answer must not look finished.
                        self.info(
                            "The model stopped after thinking, with no answer; send \"continue\".",
                        );
                    }
                    self.save_session();
                    // Subagents that finished mid-turn report now, alerting
                    // the model with a fresh turn.
                    if !self.is_busy() && self.flush_parked_messages() {
                        return self.begin_turn();
                    }
                    return self.maybe_name_session();
                }
                self.save_session();
                self.advance_tools()
            }
            StreamEvent::Stalled {
                request_id,
                message,
            } if self.active_request_id() == Some(request_id) => {
                let compacting = self
                    .active
                    .as_ref()
                    .and_then(|active| active.compacting.as_ref())
                    .map(|compaction| (compaction.split, compaction.attempts));
                // The failed request stays behind as an empty placeholder, or
                // as the partial text a later resume re-sends as history. Push
                // the resume as a new message; leaving the placeholder first
                // would send an assistant turn with no content and the
                // provider would reject the whole retry.
                self.active = None;
                self.retry = None;
                self.remove_empty_response();
                self.resume_attempts = self.resume_attempts.saturating_add(1);
                if self.resume_attempts > MAX_STREAM_RESUMES {
                    // Out of resumes: the stall is reported like any other
                    // failure and the turn goes back to the user.
                    self.resume_attempts = 0;
                    if let Some((_split, _attempts)) = compacting {
                        // The queued turn never started, so continue without
                        // the summary instead of failing it too.
                        self.info("Compaction kept stalling; sending the full history.");
                        self.save_session();
                        let messages = self.session.messages.clone();
                        return self.begin_request(messages, None);
                    }
                    self.prompt_started = None;
                    self.discard_pending_tools(
                        "The response stalled before this tool call could run.",
                    );
                    self.remove_empty_response();
                    self.session.messages.push(Message::provider_error(message));
                    self.follow_tail = true;
                    self.save_session();
                    return Action::None;
                }
                if let Some((split, attempts)) = compacting {
                    // The summary restarts against the same history prefix
                    // with a fresh buffer; the queued turn waits for it.
                    self.info(format!(
                        "The stream stalled: {message}. Retrying the summary automatically \
                         (attempt {} of {MAX_STREAM_RESUMES}).",
                        self.resume_attempts
                    ));
                    return self.begin_compaction(split, attempts);
                }
                // A stall mid-reply keeps the partial text as the trailing
                // assistant message, so the model continues it instead of
                // starting over; a stall before any output drops the empty
                // placeholder and restarts the request.
                let partial = self.session.messages.last().is_some_and(|last| {
                    last.role == Role::Assistant && last.error.is_none() && !last.content.is_empty()
                });
                if !partial {
                    self.remove_empty_response();
                }
                // The retry is a request of its own, but the partial assistant
                // turn is already in the history and ends the last message
                // before the new one. A provider rejects a trailing assistant
                // turn for some models, so a silent resume note goes in as the
                // user message the continuation needs.
                if partial {
                    let note = Message::new(
                        Role::User,
                        "The stream was cut short. Continue the answer from where it stopped, \
                         without repeating what is already there.",
                    );
                    self.session.messages.push(note);
                }
                self.info(format!(
                    "The stream stalled: {message}. Continuing automatically \
                     (attempt {} of {MAX_STREAM_RESUMES}).",
                    self.resume_attempts
                ));
                self.begin_request(self.session.messages.clone(), None)
            }
            StreamEvent::OutputLimit { request_id }
                if self.active_request_id() == Some(request_id) =>
            {
                // A capped reply is still billed: fold this request's usage
                // into the session total before the active request drops.
                if let Some((prompt, completion)) = self.active.as_ref().and_then(|a| a.usage) {
                    self.spent_prompt_tokens = self.spent_prompt_tokens.saturating_add(prompt);
                    self.spent_completion_tokens =
                        self.spent_completion_tokens.saturating_add(completion);
                }
                let compacting = self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.compacting.is_some());
                self.active = None;
                self.retry = None;
                if compacting {
                    // The queued turn never started, so continue without the
                    // summary instead of failing it too.
                    self.info("Compaction hit the output limit; sending the full history.");
                    self.save_session();
                    let messages = self.session.messages.clone();
                    return self.begin_request(messages, None);
                }
                self.continue_attempts = self.continue_attempts.saturating_add(1);
                if self.continue_attempts > MAX_AUTO_CONTINUES {
                    // Out of continuations: the cap is reported and the turn
                    // goes back to the user, partial answer and all.
                    self.continue_attempts = 0;
                    self.prompt_started = None;
                    self.discard_pending_tools(
                        "The reply hit the output limit before this tool call could run.",
                    );
                    self.remove_empty_response();
                    self.session
                        .messages
                        .push(Message::provider_error(crate::backend::OUTPUT_LIMIT));
                    self.follow_tail = true;
                    self.save_session();
                    return Action::None;
                }
                // The partial reply stays in the transcript, and a prompt asks
                // the model to carry on from it, so a capped reply finishes
                // itself instead of waiting for the user to type "continue".
                self.info(format!(
                    "The reply hit the output limit. Continuing automatically \
                     (attempt {} of {MAX_AUTO_CONTINUES}).",
                    self.continue_attempts
                ));
                // A reply that produced nothing leaves no empty bubble: the
                // continuation prompt stands on its own.
                self.remove_empty_response();
                self.session
                    .messages
                    .push(Message::new(Role::User, CONTINUATION_PROMPT));
                self.save_session();
                self.begin_turn()
            }
            StreamEvent::Error {
                request_id,
                message,
            } if self.active_request_id() == Some(request_id) => {
                let compacting = self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.compacting.is_some());
                self.active = None;
                self.retry = None;
                if compacting {
                    // The queued turn never started, so continue without the
                    // summary instead of failing it too.
                    self.info("Compaction failed; sending the full history.");
                    self.save_session();
                    let messages = self.session.messages.clone();
                    return self.begin_request(messages, None);
                }
                self.prompt_started = None;
                self.discard_pending_tools("The response failed before this tool call could run.");
                self.remove_empty_response();
                // Providers fail in the reply block, not the footer: the error
                // belongs where the answer would have been.
                self.session.messages.push(Message::provider_error(message));
                self.follow_tail = true;
                self.save_session();
                Action::None
            }
            _ => Action::None,
        }
    }

    pub(crate) fn on_tool_result(&mut self, event: ToolEvent) -> Action {
        // A subagent result lands even while other delegates still stream:
        // drop its tracker, record the answer, and continue when the queue
        // and every parallel run are done.
        if let Some(position) = self
            .running_delegates
            .iter()
            .position(|running| running.call.id == event.call.id)
        {
            self.running_delegates.remove(position);
            let content = match event.result {
                Ok(output) => output,
                Err(error) => format!("Error: {error}"),
            };
            self.record_tool_result(event.call.id, content);
            // No follow_tail snap: a result landing while the user reads an
            // earlier reply must not shoot the view back down.
            self.save_session();
            return self.advance_tools();
        }
        let Some(running) = self
            .running_tool
            .take()
            .filter(|running| running.call.id == event.call.id)
        else {
            return Action::None;
        };
        let before = running.before;
        let existed = running.existed;
        let content = match event.result {
            Ok(output) => {
                if event.call.function.name == "todo_write" {
                    if let Ok(tasks) = serde_json::from_str::<Vec<TaskState>>(&output) {
                        let done = tasks.iter().filter(|task| task.done).count();
                        let count = tasks.len();
                        self.session.tasks = tasks;
                        // An empty write deliberately clears the checklist.
                        let message = if count == 0 {
                            "Cleared the task list; the Progress panel is closed.".to_owned()
                        } else {
                            format!(
                                "Stored {count} task{} ({done} done); the Progress panel shows them.",
                                if count == 1 { "" } else { "s" }
                            )
                        };
                        self.record_tool_result(event.call.id, message);
                        self.save_session();
                        return self.advance_tools();
                    }
                }
                // A successful move_to re-roots this tab: the tool only
                // validated the target, the tab applies it to its own
                // working directory. Other tabs keep theirs.
                if event.call.function.name == "move_to" {
                    if let Some(target) = tools::move_target(&event.call, &self.working_dir) {
                        self.working_dir = target;
                    }
                }
                let diffs = tools::scoped_workspace(&self.working_dir, || {
                    tools::applied_change_all(&event.call, before, existed, &running.targets)
                });
                if !diffs.is_empty() {
                    self.changes.insert(
                        event.call.id.clone(),
                        ChangeReport {
                            diffs,
                            side_by_side: self.diff_sides,
                        },
                    );
                }
                output
            }
            Err(error) => format!("Error: {error}"),
        };
        let images = if matches!(
            event.call.function.name.as_str(),
            "read_image" | "computer_use"
        ) {
            tools::scoped_workspace(&self.working_dir, || {
                images::tool_image_attachment(&event.call, &content)
            })
            .into_iter()
            .collect()
        } else {
            Vec::new()
        };
        self.record_tool_result_with_images(event.call.id, content, images);
        self.save_session();
        self.advance_tools()
    }

    pub(crate) fn record_tool_result(
        &mut self,
        call_id: impl Into<String>,
        content: impl Into<String>,
    ) {
        self.record_tool_result_with_images(call_id, content, Vec::new());
    }

    /// Store a tool result, optionally with images the model should see.
    pub(crate) fn record_tool_result_with_images(
        &mut self,
        call_id: impl Into<String>,
        content: impl Into<String>,
        images: Vec<ImageAttachment>,
    ) {
        let mut message = Message::tool_result(call_id, content);
        message.images = images;
        self.session.messages.push(message);
    }

    pub(crate) fn discard_pending_tools(&mut self, reason: &str) {
        while let Some(call) = self.pending_tools.pop_front() {
            self.record_tool_result(call.id, reason);
        }
        self.approval = None;
    }

    /// Drop an empty assistant placeholder; `true` when one was dropped.
    ///
    /// A reply with only reasoning is kept: long thinking is real output the
    /// user asked for, so dropping it loses the whole answer.
    pub(crate) fn remove_empty_response(&mut self) -> bool {
        if self.session.messages.last().is_some_and(|message| {
            message.role == Role::Assistant
                && message.content.is_empty()
                && message.reasoning.trim().is_empty()
                && message.tool_calls.is_none()
                && message.error.is_none()
        }) {
            self.session.messages.pop();
            return true;
        }
        false
    }

    pub(crate) fn connect_error(&mut self, message: impl Into<String>) {
        if let Some(connect) = self.connect.as_mut() {
            connect.error = Some(message.into());
        }
    }

    pub(crate) fn info(&mut self, message: impl Into<String>) {
        self.notice = Some(Notice {
            kind: NoticeKind::Info,
            message: message.into(),
            started: std::time::Instant::now(),
        });
    }

    pub(crate) fn error(&mut self, message: impl Into<String>) {
        self.notice = Some(Notice {
            kind: NoticeKind::Error,
            message: message.into(),
            started: std::time::Instant::now(),
        });
    }

    pub(crate) fn save_session(&mut self) {
        if let Some(error) = self.session_saver.take_error() {
            self.error(format!("Could not save session: {error}"));
        }
        let Some(path) = self.session_path.clone() else {
            return;
        };
        self.session_saver.save(path, self.session.clone());
    }

    /// Store the custom system prompt in the saved config.
    pub(crate) fn persist_system_prompt(&mut self, value: Option<String>) {
        if let Err(error) = config::save_system_prompt(&self.config_path, value) {
            self.error(format!("Could not save the system prompt: {error:#}"));
        }
    }

    /// Store the permission mode in the saved config.
    pub(crate) fn persist_permission(&mut self) {
        let value = self.tools_mode.label();
        if let Err(error) = config::save_permission(&self.config_path, value) {
            self.error(format!("Could not save the permission: {error:#}"));
        }
    }

    /// Store the automatic compaction switch in the saved config.
    pub(crate) fn persist_auto_compact(&mut self, value: bool) {
        if let Err(error) = config::save_auto_compact(&self.config_path, value) {
            self.error(format!("Could not save the compact setting: {error:#}"));
        }
    }

    /// Store the reasoning effort in the saved config.
    pub(crate) fn persist_reasoning_effort(&mut self, value: Option<String>) {
        if let Err(error) = config::save_reasoning_effort(&self.config_path, value) {
            self.error(format!("Could not save the reasoning effort: {error:#}"));
        }
    }

    pub(crate) fn persist(&self) -> Result<()> {
        let Some(path) = self.session_path.clone() else {
            return Ok(());
        };
        // ponytail: skip untouched empty session to avoid ghost files; remove check if empty sessions need saving.
        if self.session.messages.is_empty() && !path.exists() {
            return Ok(());
        }
        if self
            .session_saver
            .save_and_wait(path.clone(), self.session.clone())
        {
            return Ok(());
        }
        self.session.save(&path)
    }
}
