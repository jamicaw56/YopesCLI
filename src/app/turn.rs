use super::*;
use chacha20poly1305::aead::{OsRng, rand_core::RngCore};

fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
}

/// Whether the context is due for compaction: certain past the window, and a
/// rising random chance above ~80% of it — about half the turns near 90%,
/// every turn at 100%. A deterministic threshold would compact on the exact
/// same turn every time; the roll spreads it out instead.
fn compaction_due(total: u64, window: u64) -> bool {
    if total >= window {
        return true;
    }
    let mut bytes = [0u8; 8];
    OsRng.fill_bytes(&mut bytes);
    compaction_chance(total, window, u64::from_le_bytes(bytes))
}

/// The chance decision with the roll pinned, so tests can check the ramp.
/// The chance rises linearly from 0 at `start` to 1 at the window itself.
fn compaction_chance(total: u64, window: u64, roll: u64) -> bool {
    let start = window / COMPACT_CHANCE_FRAC * (COMPACT_CHANCE_FRAC - 1);
    if total < start {
        return false;
    }
    let span = window.saturating_sub(start).max(1);
    let progress = total - start;
    roll / (u64::MAX / span) < progress
}

/// Whether a command can run while the model is still answering.
///
/// Commands that only read state or change a setting are safe: they do not
/// touch the turn in flight. The ones that would swap the session, the
/// provider or the conversation are not.
fn command_is_safe_while_streaming(prompt: &str) -> bool {
    let mut words = prompt.split_whitespace();
    let name = words.next().unwrap_or_default();
    // Only the rename half of `/session` touches nothing but the title; the
    // rest swaps, closes or opens what the turn is answering in.
    if name == "/session" {
        return words.next() == Some("rename");
    }
    // An immediate compaction is a request of its own; the rest of `/set`
    // only changes what the next request sends.
    if name == "/set" {
        let key = words.next().unwrap_or_default();
        let value = words.next().unwrap_or_default();
        return !key.eq_ignore_ascii_case("compact") || matches!(value, "on" | "off");
    }
    // `/settings` and the read-only commands change nothing about the turn.
    matches!(
        name,
        "/help" | "/agents" | "/skills" | "/quit" | "/settings"
    )
}

fn normalize_endpoint_url(url: &str) -> (String, &str) {
    let url = url.trim().trim_end_matches('/');
    if let Some((scheme, rest)) = url.split_once("://") {
        if let Some((host, path)) = rest.split_once('/') {
            (
                format!(
                    "{}://{}",
                    scheme.to_ascii_lowercase(),
                    host.to_ascii_lowercase()
                ),
                path,
            )
        } else {
            (
                format!(
                    "{}://{}",
                    scheme.to_ascii_lowercase(),
                    rest.to_ascii_lowercase()
                ),
                "",
            )
        }
    } else {
        (url.to_ascii_lowercase(), "")
    }
}

fn endpoints_match(a: &str, b: &str) -> bool {
    normalize_endpoint_url(a) == normalize_endpoint_url(b)
}

impl App {
    pub(crate) fn scroll_up(&mut self, amount: usize) {
        self.follow_tail = false;
        self.scroll = self.scroll.saturating_sub(amount);
    }

    pub(crate) fn scroll_down(&mut self, amount: usize) {
        self.scroll = self.scroll.saturating_add(amount);
    }

    pub(crate) fn submit(&mut self) -> Action {
        let steering = self.is_streaming();
        // A pending approval still owns the keyboard; a running tool does not.
        if self.approval.is_some() {
            self.info("Approve or deny the pending tool first.");
            return Action::None;
        }
        let tool_busy = self.running_tool.is_some()
            || !self.pending_tools.is_empty()
            // Parallel subagents keep the turn waiting: a message sent while
            // they run queues for the post-tool continuation instead of
            // starting an overlapping turn their results would land inside.
            || !self.running_delegates.is_empty();
        if steering
            && self
                .active
                .as_ref()
                .is_some_and(|active| active.compacting.is_some())
        {
            self.info("Wait for compaction to finish.");
            return Action::None;
        }
        let mut value = self.input.take();
        self.reset_palette();
        let prompt = value.trim().to_owned();
        if prompt.is_empty() && self.attachments.is_empty() {
            return Action::None;
        }
        if prompt.starts_with('!') {
            let cmd = prompt.trim_start_matches('!').trim();
            if cmd.is_empty() {
                self.info("Usage: !<command>");
                return Action::None;
            }
            if tool_busy {
                self.input.set(prompt);
                self.info("Finish or stop the tool before running that command.");
                return Action::None;
            }
            if steering {
                self.stop_streaming();
            }
            return self.run_user_command(cmd);
        }
        if prompt.starts_with('/') {
            // Builtins never lose their name to a skill of the same spelling.
            if COMMANDS
                .iter()
                .any(|command| prompt.split_whitespace().next() == Some(command.name))
            {
                // A command that only reads or changes a setting is safe to
                // run while the model is answering or a tool is running; the
                // ones that would disturb the turn are not.
                if tool_busy {
                    self.input.set(prompt);
                    self.info("Finish or stop the tool before running that command.");
                    return Action::None;
                }
                if steering && !command_is_safe_while_streaming(&prompt) {
                    self.stop_streaming();
                }
                return self.command(&prompt);
            }
            let expanded = skills::expand(&prompt, self.working_dir());
            match expanded {
                // A skill invocation becomes a normal turn that carries the
                // skill body, so it works with tools off too.
                Some(expanded) => value = expanded,
                None => {
                    if tool_busy {
                        self.input.set(prompt);
                        self.info("Finish or stop the tool before running that command.");
                        return Action::None;
                    }
                    if steering && !command_is_safe_while_streaming(&prompt) {
                        self.stop_streaming();
                    }
                    return self.command(&prompt);
                }
            }
        }
        if !self.backend.is_configured() {
            self.input.set(value);
            self.error("No provider connected. Run /provider connect to get started.");
            return Action::None;
        }
        if self.backend.model().trim().is_empty() {
            self.input.set(value);
            self.error(
                "No model selected. Run /provider models or /provider connect to choose a model.",
            );
            return Action::None;
        }
        if self.backend.base_url().trim().is_empty() {
            self.input.set(value);
            self.error(
                "No endpoint URL configured. Run /provider connect to configure this provider.",
            );
            return Action::None;
        }
        if self.backend.auth().requires_token() && self.backend.api_key().is_none() {
            self.input.set(value);
            self.error(
                "No API key configured for this provider. Run /provider connect to enter a key.",
            );
            return Action::None;
        }

        self.notice = None;
        // A new user turn starts with a fresh stream-resume budget and a
        // fresh continuation budget.
        self.resume_attempts = 0;
        self.continue_attempts = 0;
        self.send_pulse = Some(self.spinner);
        self.session.update_title(&prompt);
        self.prompt_started = Some(Instant::now());
        // A message the user sent is the one thing that snaps the transcript
        // back to the tail: streaming events, tool calls and results no longer
        // yank a scrolled-up view (see `follow_tail`).
        self.follow_tail = true;
        // Swap `[Image N]` placeholders back to their real paths: the model
        // and the transcript see the path, the composer never did.
        value = self.resolve_image_placeholders(value);
        if !self.running_delegates.is_empty() {
            // Delegates still run: their tool results must follow the
            // assistant turn that asked for them, so a message typed now
            // queues for the post-delegate continuation instead of landing
            // between the tool calls and their results.
            let images = std::mem::take(&mut self.attachments);
            self.queued_message = match self.queued_message.take() {
                Some(existing) => Some(format!("{existing}\n\n{value}")),
                None => Some(value),
            };
            self.queued_images.extend(images);
            self.info("Queued — it sends when the running agents finish.");
            return Action::None;
        }
        if steering && self.running_delegates.is_empty() {
            // Drop the blank reply the interrupted request left behind, then
            // fold the steer into the conversation and restart the turn.
            // Delegates still running keep the turn instead: their results
            // must not land inside a restarted conversation.
            self.discard_pending_tools("The user steered the response before this tool could run.");
            self.remove_empty_response();
            // Several steers before the model answered read as one message
            // block, not one bubble per line.
            if let Some(last) = self
                .session
                .messages
                .last_mut()
                .filter(|message| message.role == Role::User)
            {
                if !last.content.trim().is_empty() && !value.trim().is_empty() {
                    last.content.push_str("\n\n");
                }
                last.content.push_str(&value);
                last.images.extend(std::mem::take(&mut self.attachments));
                return self.begin_turn();
            }
        }
        // Reports parked while the tab was busy come before the new message.
        if !steering && !tool_busy {
            self.flush_parked_messages();
        }
        let mut message = Message::new(Role::User, value);
        message.images = std::mem::take(&mut self.attachments);
        self.session.messages.push(message);
        // Chat during a tool run queues the message: the post-tool
        // continuation sends it together with the tool result.
        if tool_busy {
            self.save_session();
            return Action::None;
        }
        self.begin_turn()
    }

    /// Start the next turn: the context load decides whether older turns are
    /// summarized first, then the real request is sent.
    pub(crate) fn begin_turn(&mut self) -> Action {
        self.begin_turn_after_compaction(0)
    }

    /// The turn, with `attempts` compaction rounds already behind it. A
    /// compaction retry rolls again here, and the cap keeps a provider that
    /// answers empty (or a summary that never shrinks the load) from looping
    /// the turn forever.
    pub(crate) fn begin_turn_after_compaction(&mut self, attempts: u32) -> Action {
        let total = Session::estimate_tokens(&self.session.messages);
        let window = self.model_context_window().unwrap_or(DEFAULT_CONTEXT_WINDOW);
        let keep = window / COMPACTION_KEEP_FRAC;
        if self.auto_compact && attempts < MAX_COMPACTION_ATTEMPTS {
            if let Some(split) = self.session.compaction_split(keep) {
                if split > 0 && compaction_due(total, window) {
                    return self.begin_compaction(split, attempts);
                }
            }
        }
        let messages = self.session.messages.clone();
        self.begin_request(messages, None)
    }

    /// Summarize everything before `split`, then continue with the turn.
    pub(crate) fn begin_compaction(&mut self, split: usize, attempts: u32) -> Action {
        let mut messages = self.session.messages[..split].to_vec();
        messages.push(Message::new(
            Role::User,
            "Summarize the conversation above for an assistant taking over: what the user wants, decisions made, files changed, and anything still unfinished. Reply with the summary only.",
        ));
        self.begin_request(
            messages,
            Some(Compaction {
                split,
                text: String::new(),
                attempts,
            }),
        )
    }

    pub(crate) fn begin_request(
        &mut self,
        messages: Vec<Message>,
        compacting: Option<Compaction>,
    ) -> Action {
        // A compaction request streams into its own buffer, not the transcript,
        // so no reply placeholder is pushed.
        if compacting.is_none() {
            self.session
                .messages
                .push(Message::new(Role::Assistant, ""));
        }
        self.next_request_id = self.next_request_id.wrapping_add(1);
        let request_id = self.next_request_id;
        let tools_enabled = self.mode != Mode::Chat && self.tools_mode.advertised();
        let mode_note = self.mode.instruction(self.tools_mode);
        let prompt_chars = tools::system_prompt(tools_enabled, self.working_dir())
            .chars()
            .count()
            + self
                .backend
                .system_prompt()
                .map_or(0, |prompt| prompt.chars().count() + 2)
            + mode_note.map_or(0, |note| note.chars().count() + 2)
            + messages
                .iter()
                .map(|message| {
                    message.content.chars().count()
                        + message.tool_calls.as_ref().map_or(0, |calls| {
                            calls
                                .iter()
                                .map(|call| {
                                    call.function.name.chars().count()
                                        + call.function.arguments.chars().count()
                                })
                                .sum::<usize>()
                        })
                })
                .sum::<usize>();
        self.active = Some(ActiveRequest {
            id: request_id,
            started: Instant::now(),
            prompt_tokens: prompt_chars as u64 / 4,
            usage: None,
            tool_progress: None,
            compacting,
        });
        self.save_session();
        Action::Start {
            request_id,
            messages,
            tools_enabled,
            mode_note,
        }
    }

    /// Name the session by hand: the title the header tab, the tab strip and
    /// `/sessions` show. A manual name also stops the auto-namer from
    /// replacing it after the first reply.
    pub(crate) fn rename_session(&mut self, title: &str) {
        let title: String = title.trim().chars().take(48).collect();
        let title = title.trim();
        if title.is_empty() {
            return;
        }
        self.session.title = title.to_owned();
        self.session.auto_named = true;
        self.save_session();
        self.info(format!("Session renamed to {title}"));
    }

    pub(crate) fn command(&mut self, command: &str) -> Action {
        let mut parts = command.split_whitespace();
        match parts.next().unwrap_or_default() {
            "/help" => self.show_help = true,
            "/session" => match parts
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str()
            {
                "" | "list" => self.open_session_picker(),
                "new" => self.new_session(),
                "tab" => self.open_tab(),
                "close" => self.close_tab(),
                "next" => self.cycle_tab(1),
                "prev" | "previous" => self.cycle_tab(-1),
                "rename" => {
                    let title = parts.collect::<Vec<_>>().join(" ");
                    if title.trim().is_empty() {
                        let current = self.session.title.clone();
                        self.info(format!("Session: {current} · /session rename <title>"));
                    } else {
                        self.rename_session(&title);
                    }
                }
                other => match other.parse::<usize>() {
                    Ok(index) if index >= 1 => self.select_tab(index - 1),
                    _ => self
                        .error("Usage: /session list|new|tab|rename <title>|close|next|prev|1-9"),
                },
            },
            "/provider" => match parts
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str()
            {
                "" | "connect" => return self.begin_connect(),
                "disconnect" => {
                    let target = parts.collect::<Vec<_>>().join(" ");
                    if target.is_empty() {
                        let active_endpoint = self.backend.base_url().to_owned();
                        if active_endpoint.is_empty()
                            || self.backend.provider_kind() == ProviderKind::Unconfigured
                        {
                            self.info("No provider currently connected.");
                        } else {
                            self.disconnect_endpoint(&active_endpoint);
                        }
                    } else {
                        self.disconnect_named_provider(&target);
                    }
                }
                "models" => {
                    let model = parts.collect::<Vec<_>>().join(" ");
                    if model.is_empty() {
                        self.open_model_picker();
                    } else {
                        self.backend.set_model(model.clone());
                        self.persist_model(&model);
                        self.info(format!("Model changed to {model}"));
                    }
                }
                _ => self.error("Usage: /provider connect|disconnect [name]|models [name]"),
            },
            "/run" => {
                let cmd = parts.collect::<Vec<_>>().join(" ");
                if cmd.is_empty() {
                    self.info("Usage: /run <command>");
                } else {
                    return self.run_user_command(&cmd);
                }
            }
            "/cd" => {
                let path = parts.collect::<Vec<_>>().join(" ");
                self.change_directory(&path);
            }
            "/set" => {
                let key = parts.next().unwrap_or_default().to_ascii_lowercase();
                let value = parts.collect::<Vec<_>>().join(" ");
                match key.as_str() {
                    "tools" | "permission" | "permissions" => self.set_tools_mode(&value),
                    "theme" => self.set_theme(&value),
                    "system" | "prompt" => self.set_custom_system_prompt(&value),
                    "effort" | "reasoning" => self.set_effort(&value),
                    "compact" => return self.set_compact(&value),
                    "" => self.show_settings(),
                    other => self.error(format!("Unknown setting `{other}` · /set {SET_KEYS}")),
                }
            }
            "/settings" => self.open_settings(),
            "/agents" => self.open_agents_picker(),
            "/skills" => self.show_skills(),
            "/revert" => self.open_revert_picker(),
            "/quit" => {
                self.should_quit = true;
                return Action::Quit;
            }
            unknown => self.error(format!(
                "Unknown command `{unknown}`. Type / to see commands."
            )),
        }
        Action::None
    }

    /// `/set` with no key: every setting and its current value on one line,
    /// so the keys are discoverable without a manual.
    fn show_settings(&mut self) {
        let effort = self
            .backend
            .reasoning_effort()
            .unwrap_or("default")
            .to_owned();
        let system = self.backend.system_prompt().map_or_else(
            || "none".to_owned(),
            |prompt| prompt.chars().take(40).collect::<String>(),
        );
        let auto_compact = if self.auto_compact { "on" } else { "off" };
        let message = format!(
            "Settings · tools {} · theme {} · effort {effort} · auto compact {auto_compact} · \
             system {system} · /set {SET_KEYS}",
            self.tools_mode.label(),
            crate::theme::current_name(),
        );
        self.info(message);
    }

    /// `/set tools <mode>`: the permission mode Shift+Tab cycles.
    pub(crate) fn set_tools_mode(&mut self, value: &str) {
        let value = value.trim().to_ascii_lowercase();
        if value.is_empty() {
            self.info(format!(
                "Tools: {} — {} · /set tools on|yolo|ask|off|dangerous",
                self.tools_mode.label(),
                self.tools_mode.detail()
            ));
            return;
        }
        let Some(mode) = ToolsMode::parse(&value) else {
            self.error("Usage: /set tools on|yolo|ask|off|dangerous");
            return;
        };
        self.tools_mode = mode;
        let suffix = if mode.advertised()
            && !matches!(
                self.backend.provider_kind(),
                ProviderKind::OpenAi
                    | ProviderKind::Custom
                    | ProviderKind::Anthropic
                    | ProviderKind::Gemini
            ) {
            " · tools require an OpenAI, Anthropic, or Gemini provider"
        } else {
            ""
        };
        self.info(format!(
            "Tools: {} — {}{suffix}",
            mode.label(),
            mode.detail()
        ));
        if mode == ToolsMode::Dangerous {
            // Approval is off for everything, so name what that means before
            // the first automated click or command lands.
            self.info(
                "Dangerous: no tool asks for approval, computer_use included. \
                 /set tools on restores the prompts; Shift+Tab leaves this mode.",
            );
        }
    }

    /// `/set theme [name]`: the color theme, saved to the config.
    pub(crate) fn set_theme(&mut self, value: &str) {
        let value = value.trim().to_ascii_lowercase();
        if value.is_empty() {
            self.info(format!(
                "Theme: {} · /set theme <name> · {}",
                crate::theme::current_name(),
                crate::theme::names()
            ));
        } else if crate::theme::set(&value) {
            if let Err(error) = config::save_theme(&self.config_path, &value) {
                self.error(format!("Could not save the theme: {error:#}"));
            }
            self.info(format!("Theme: {}", crate::theme::current_name()));
        } else {
            self.error(format!("Usage: /set theme <{}>", crate::theme::names()));
        }
    }

    /// `/set system [text]`: the custom system prompt, or `off` to clear it.
    fn set_custom_system_prompt(&mut self, value: &str) {
        let value = value.trim().to_owned();
        let current = self.backend.system_prompt().map(str::to_owned);
        if value.is_empty() {
            if let Some(current) = current {
                self.info(format!("Custom system prompt: {current}"));
            } else {
                self.info(
                    "No custom system prompt · /set system <text> to set one, \
                     /set system off to remove it",
                );
            }
        } else if matches!(value.as_str(), "off" | "clear" | "none") {
            self.backend.set_system_prompt(None);
            self.persist_system_prompt(None);
            self.info("Custom system prompt removed");
        } else {
            self.backend.set_system_prompt(Some(value.clone()));
            self.persist_system_prompt(Some(value));
            self.info("Custom system prompt set");
        }
    }

    /// `/set effort [level]`: the reasoning effort, or `off` for the backend
    /// default.
    pub(crate) fn set_effort(&mut self, value: &str) {
        let value = value.trim().to_ascii_lowercase();
        if value.is_empty() {
            match self.backend.reasoning_effort() {
                Some(effort) => {
                    self.info(format!("Reasoning effort: {effort} · {EFFORT_CHOICES}"));
                }
                None => self.info(format!(
                    "Reasoning effort: backend default · {EFFORT_CHOICES}"
                )),
            }
        } else if matches!(value.as_str(), "off" | "none") {
            self.persist_reasoning_effort(None);
            self.backend.set_reasoning_effort(None);
            self.info("Reasoning effort off — the backend default applies");
        } else if matches!(value.as_str(), "low" | "medium" | "high" | "xhigh") {
            self.persist_reasoning_effort(Some(value.clone()));
            self.backend.set_reasoning_effort(Some(value.clone()));
            self.info(format!("Reasoning effort set to {value}"));
        } else {
            self.error(format!("Usage: {EFFORT_CHOICES}"));
        }
    }

    /// `/set compact [on|off|now]`: auto compaction, or summarize right now.
    pub(crate) fn set_compact(&mut self, value: &str) -> Action {
        let value = value.trim().to_ascii_lowercase();
        if value.is_empty() || value == "now" {
            if !self.backend.is_configured() {
                self.error("No provider connected. Run /provider connect to get started.");
            } else if self.backend.model().trim().is_empty() {
                self.error("No model selected. Run /provider models or /provider connect.");
            } else if let Some(split) = self
                .session
                .compaction_split(
                    self.model_context_window()
                        .map(|w| w / COMPACTION_KEEP_FRAC)
                        .unwrap_or(DEFAULT_CONTEXT_WINDOW / COMPACTION_KEEP_FRAC),
                )
                .filter(|split| *split > 0)
            {
                return self.begin_compaction(split, 0);
            } else {
                self.info(format!(
                    "Nothing to compact · auto compact {} · /set compact on|off|now",
                    if self.auto_compact { "on" } else { "off" }
                ));
            }
        } else if matches!(value.as_str(), "on" | "off") {
            self.auto_compact = value == "on";
            self.persist_auto_compact(self.auto_compact);
            self.info(if self.auto_compact {
                "Auto compact on — older turns summarize once the context fills".to_owned()
            } else {
                "Auto compact off — the full history is sent; /set compact now summarizes it \
                 manually"
                    .to_owned()
            });
        } else {
            self.error("Usage: /set compact on|off|now");
        }
        Action::None
    }

    /// List what is installed, one line per skill, including where it came
    /// from. Nothing here reads files; discovery only checks names.
    pub(crate) fn show_skills(&mut self) {
        let message = skills::summary(self.working_dir()).unwrap_or_else(|| {
            "No skills installed · add Skill.md files under ~/.yopes/skills/<name>/ \
             or .yopes/skills/<name>/"
                .to_owned()
        });
        self.info(message);
    }

    /// Switch this tab's working directory, so every path-based tool in the
    /// tab resolves from there. Other tabs keep their own directory and the
    /// process working directory never changes.
    pub(crate) fn change_directory(&mut self, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            self.info(format!("Working directory: {}", self.working_dir.display()));
            return;
        }
        let expanded = if let Some(rest) = value
            .strip_prefix("~/")
            .or_else(|| value.strip_prefix("~\\"))
        {
            home_dir().map(|home| home.join(rest))
        } else if value == "~" {
            home_dir()
        } else {
            Some(PathBuf::from(value))
        };
        let Some(path) = expanded else {
            self.error("Could not resolve the home directory.");
            return;
        };
        let path = if path.is_absolute() {
            path
        } else {
            self.working_dir.join(path)
        };
        let path = match path.canonicalize() {
            Ok(path) => path,
            Err(error) => {
                self.error(format!("Could not open {}: {error}", path.display()));
                return;
            }
        };
        if !path.is_dir() {
            self.error(format!("{} is not a directory.", path.display()));
            return;
        }
        self.working_dir = path.clone();
        self.scroll = 0;
        self.follow_tail = true;
        self.info(format!("Working directory: {}", path.display()));
    }

    /// Run a shell command in the workspace directly for the user.
    /// Run a command the user typed (`!cmd` or `/run cmd`). The command runs
    /// in the background through `Action::RunUserCommand`, so a long build
    /// cannot soft-lock the event loop; the result lands as its own message.
    pub(crate) fn run_user_command(&mut self, command: &str) -> Action {
        let command = command.trim();
        if command.is_empty() {
            self.info("Usage: /run <command> or !<command>");
            return Action::None;
        }
        if self.user_command_running {
            self.input.set(format!("!{command}"));
            self.info("A command is still running; Esc interrupts it.");
            return Action::None;
        }
        self.notice = None;
        self.user_command_running = true;
        let user_msg = Message::new(Role::User, format!("$ {command}"));
        self.session.messages.push(user_msg);
        self.follow_tail = true;
        self.save_session();
        Action::RunUserCommand {
            command: command.to_owned(),
        }
    }

    /// A background user command finished: record its output where the `$ cmd`
    /// message sits.
    pub(crate) fn on_user_command_result(&mut self, result: Result<String, String>) -> Action {
        self.user_command_running = false;
        let output = match result {
            Ok(output) => output,
            Err(error) => format!("Command failed: {error:#}"),
        };
        let mut tool_msg = Message::new(Role::Tool, output);
        tool_msg.tool_call_id = Some(format!("user-cmd-{}", self.session.messages.len()));
        self.session.messages.push(tool_msg);
        self.follow_tail = true;
        self.save_session();
        // A subagent that finished while the command ran reports now.
        if !self.is_busy() && self.flush_parked_messages() {
            return self.begin_turn();
        }
        Action::None
    }

    /// A background bash job finished (or timed out): record the outcome
    /// where the model can read it, so it never needs to poll `bash_output`.
    pub(crate) fn on_bash_notice(&mut self, notice: crate::tools::BashNotice) {
        let mut msg = Message::new(Role::Tool, notice.output);
        msg.tool_call_id = Some(format!("bash-job-{}", notice.job_id));
        self.session.messages.push(msg);
        self.follow_tail = true;
        self.save_session();
    }

    /// Esc while a command or tool runs: kill this session's foreground
    /// commands and discard the queued tools, so a long run cannot hold the
    /// turn hostage — and other tabs' running commands stay untouched.
    pub(crate) fn interrupt_tools(&mut self) -> Action {
        self.discard_pending_tools("The user interrupted before this tool could run.");
        if self.user_command_running {
            self.user_command_running = false;
        }
        self.info("Interrupted. Running commands were stopped.");
        self.save_session();
        // The kill goes through the event loop: the registry is keyed by
        // session id, so only this tab's commands die.
        Action::KillCommands(self.session.id)
    }

    /// Forget the key stored for one endpoint, resetting the session when it is
    /// the provider in use.
    pub(crate) fn disconnect_endpoint(&mut self, target: &str) {
        if self.is_busy() {
            self.info("Finish the current turn before disconnecting.");
            return;
        }
        match config::disconnect_provider(&self.config_path, target) {
            Ok(()) => {
                // The active connection goes with it.
                if self.backend.base_url().trim_end_matches('/') == target.trim_end_matches('/') {
                    self.backend.configure(
                        ProviderKind::Unconfigured,
                        "not connected".to_owned(),
                        String::new(),
                        String::new(),
                        None,
                        AuthKind::Bearer,
                        Vec::new(),
                    );
                }
                self.custom_providers = config::load_custom_providers(&self.config_path);
                if let Some(connect) = self.connect.as_mut() {
                    // The row loses its tick immediately.
                    connect.saved_custom = self.custom_providers.clone();
                    connect.error = None;
                }
                self.info("Disconnected. The key is asked for again next time.");
            }
            Err(error) => self.error(format!("Could not disconnect: {error:#}")),
        }
    }

    /// Remove a saved custom provider, resetting the session when it is the
    /// provider in use.
    pub(crate) fn delete_custom_provider(&mut self, id: &str) {
        if self.is_busy() {
            self.info("Finish the current turn before removing a provider.");
            return;
        }
        let target_endpoint = self
            .custom_providers
            .iter()
            .find(|provider| provider.id == id)
            .map(|provider| provider.base_url.trim_end_matches('/').to_owned());
        match config::delete_custom_provider(&self.config_path, id) {
            Ok(()) => {
                if let Some(target) = target_endpoint {
                    if self.backend.base_url().trim_end_matches('/') == target {
                        self.backend.configure(
                            ProviderKind::Unconfigured,
                            "not connected".to_owned(),
                            String::new(),
                            String::new(),
                            None,
                            AuthKind::Bearer,
                            Vec::new(),
                        );
                    }
                }
                self.custom_providers = config::load_custom_providers(&self.config_path);
                if let Some(connect) = self.connect.as_mut() {
                    connect.saved_custom = self.custom_providers.clone();
                    connect.error = None;
                }
                self.info("Provider removed.");
            }
            Err(error) => self.error(format!("Could not remove provider: {error:#}")),
        }
    }

    /// Rename a saved custom provider, updating active backend if it matches.
    pub(crate) fn rename_custom_provider(&mut self, id: &str, new_name: &str) {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            self.connect_error("Provider name cannot be empty.");
            return;
        }
        let target_endpoint = self
            .custom_providers
            .iter()
            .find(|provider| provider.id == id)
            .map(|provider| provider.base_url.trim_end_matches('/').to_owned());
        match config::rename_custom_provider(&self.config_path, id, new_name) {
            Ok(()) => {
                if let Some(target) = target_endpoint {
                    if self.backend.base_url().trim_end_matches('/') == target {
                        self.backend.set_provider_label(new_name.to_owned());
                    }
                }
                self.custom_providers = config::load_custom_providers(&self.config_path);
                if let Some(connect) = self.connect.as_mut() {
                    connect.saved_custom = self.custom_providers.clone();
                    connect.custom_name = new_name.to_owned();
                    connect.error = None;
                    connect.stage = ConnectStage::CustomModels;
                }
                self.info(format!("Provider renamed to {new_name}."));
            }
            Err(error) => self.connect_error(format!("Could not rename provider: {error:#}")),
        }
    }

    /// Disconnect a provider by name, provider id, or endpoint.
    // ponytail: linear scan over providers; fast enough for typical <20 saved endpoints.
    pub(crate) fn disconnect_named_provider(&mut self, target: &str) {
        let needle = target.trim().to_ascii_lowercase();
        if let Some(custom) = self
            .custom_providers
            .iter()
            .find(|p| p.id.to_ascii_lowercase() == needle || p.name.to_ascii_lowercase() == needle)
        {
            let id = custom.id.clone();
            self.delete_custom_provider(&id);
            return;
        }
        let connected = config::load_connected_providers(&self.config_path);
        if let Some(provider) = connected.iter().find(|p| {
            p.provider_id
                .as_ref()
                .is_some_and(|id| id.to_ascii_lowercase() == needle)
                || p.name.to_ascii_lowercase() == needle
                || endpoints_match(&p.base_url, target)
        }) {
            let endpoint = provider.base_url.clone();
            self.disconnect_endpoint(&endpoint);
            return;
        }
        if self.backend.provider_label().to_ascii_lowercase() == needle
            || endpoints_match(self.backend.base_url(), target)
        {
            let endpoint = self.backend.base_url().to_owned();
            self.disconnect_endpoint(&endpoint);
            return;
        }
        self.info(format!("No saved provider matches '{target}'."));
    }

    /// Store the model of the active connection, so the choice survives a
    /// restart.
    pub(crate) fn persist_model(&mut self, model: &str) {
        if let Err(error) = config::save_model(&self.config_path, model) {
            self.error(format!("Could not save the model: {error:#}"));
        }
    }

    pub(crate) fn begin_connect(&mut self) -> Action {
        if self.is_busy() {
            self.info("Finish the current turn before connecting a provider.");
            return Action::None;
        }
        self.palette_hidden = true;
        let mut connect = ConnectState::loading();
        connect.saved_custom = self.custom_providers.clone();
        if self.backend.provider_kind() == ProviderKind::Custom {
            connect.custom_endpoint = self.backend.base_url().to_owned();
            connect.custom_auth = self.backend.auth();
            connect.custom_models = self.backend.saved_models().to_vec();
            if let Some(matched) = self
                .custom_providers
                .iter()
                .find(|provider| provider.base_url == connect.custom_endpoint)
            {
                connect.custom_provider_id = Some(matched.id.clone());
                connect.custom_name = matched.name.clone();
                connect.custom_token = matched.token.clone().unwrap_or_default();
            }
        }
        self.connect = Some(connect);
        Action::LoadCatalog
    }

    pub(crate) fn new_session(&mut self) {
        if self.is_busy() {
            self.info("Finish the current turn before starting a new session.");
            return;
        }
        self.markdown_cache.clear();
        self.session = Session::new();
        // Future saves land in a fresh file, so this one keeps its history.
        if let Some(dir) = &self.sessions_dir {
            let stamp = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_millis() as u64);
            self.session_path = Some(dir.join(format!("session-{stamp}.json")));
        }
        self.reset_session_view();
        self.save_session();
    }

    pub(crate) fn stop_streaming(&mut self) -> Action {
        let Some(active) = self.active.take() else {
            return Action::None;
        };
        self.prompt_started = None;
        if active.compacting.is_some() {
            // The queued turn never started, so there is nothing to finish.
            self.info("Compaction stopped. Send another message to continue.");
            self.save_session();
            return Action::Cancel(active.id);
        }
        self.discard_pending_tools("The user stopped the response before this tool could run.");
        self.remove_empty_response();
        self.info("Response stopped. Partial output was kept.");
        self.save_session();
        // A subagent that finished while the turn streamed reports now.
        if !self.is_busy() && self.flush_parked_messages() {
            return self.begin_turn();
        }
        Action::Cancel(active.id)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::{AuthKind, Config, ProviderKind};
    use crate::session::ToolCall;
    use std::time::Instant;

    pub(crate) fn test_app() -> App {
        let config = Config {
            provider: ProviderKind::OpenAi,
            provider_label: "OpenAI".to_owned(),
            model: "gpt-4o".to_owned(),
            base_url: "https://api.openai.com/v1".to_owned(),
            api_key: Some("test-key".to_owned()),
            auth: AuthKind::Bearer,
            models: Vec::new(),
            system_prompt: None,
            custom_providers: Vec::new(),
            session_path: None,
            sessions_dir: None,
            config_path: PathBuf::new(),
            tavily_api_key: None,
            auto_compact: false,
            reasoning_effort: None,
            theme: "dark".to_owned(),
        };
        let backend = Backend::new(&config).expect("backend");
        App::new(
            Session::new(),
            backend,
            None,
            None,
            PathBuf::new(),
            Vec::new(),
            None,
            false,
        )
    }

    #[test]
    fn the_compaction_chance_ramps_to_certain() {
        let window = 100_000u64;
        let quarter = u64::MAX / 4;
        let three_quarters = u64::MAX / 4 * 3;
        // Below the 80% start: never, whatever the roll.
        assert!(!compaction_chance(60_000, window, 0));
        // At 80%: the chance just begins; nothing compacts yet.
        assert!(!compaction_chance(80_000, window, 0));
        // At 90%: about half the rolls compact.
        assert!(compaction_chance(90_000, window, quarter));
        assert!(!compaction_chance(90_000, window, three_quarters));
        // At 99%: most rolls compact.
        assert!(compaction_chance(99_000, window, quarter));
        // Past the window: always, whatever the roll.
        assert!(compaction_chance(150_000, window, u64::MAX));
    }

    #[test]
    fn submit_during_tool_run_queues_message_and_blocks_unsafe_commands() {
        let mut app = test_app();
        app.running_tool = Some(RunningTool {
            call: ToolCall::new("call-1", "test_tool", "{}"),
            started: Instant::now(),
            before: None,
            existed: false,
            targets: Vec::new(),
        });

        // Unsafe command while tool runs is blocked and restored to input
        app.input.set("/session new");
        let action = app.submit();
        assert!(matches!(action, Action::None));
        assert_eq!(app.input.value, "/session new");
        assert!(app.notice.is_some());

        // Chat message while tool runs is queued into session messages
        app.input.set("queued turn");
        let action = app.submit();
        assert!(matches!(action, Action::None));
        assert_eq!(app.session.messages.len(), 1);
        assert_eq!(app.session.messages[0].content, "queued turn");
        assert_eq!(app.session.messages[0].role, Role::User);
        assert!(app.follow_tail);

        // Approval gate blocks submit
        let mut app_approval = test_app();
        app_approval.approval = Some(ToolApproval {
            call: ToolCall::new("call-2", "test_tool", "{}"),
            note: None,
            diffs: Vec::new(),
            scroll: 0,
            page_height: 0,
            popup_area: std::cell::Cell::new(ratatui::layout::Rect::default()),
        });
        app_approval.input.set("blocked while waiting approval");
        let action = app_approval.submit();
        assert!(matches!(action, Action::None));
        assert!(app_approval.session.messages.is_empty());
    }

    #[test]
    fn submit_command_while_model_streaming_stops_stream_and_runs_command() {
        let mut app = test_app();
        app.active = Some(ActiveRequest {
            id: 1,
            started: Instant::now(),
            prompt_tokens: 10,
            usage: None,
            tool_progress: None,
            compacting: None,
        });
        assert!(app.is_streaming());

        app.input.set("!echo hello_active");
        let action = app.submit();
        assert!(matches!(action, Action::RunUserCommand { .. }));
        assert!(!app.is_streaming());
        assert!(app.user_command_running);
        assert_eq!(app.session.messages.len(), 1);
        assert_eq!(app.session.messages[0].content, "$ echo hello_active");
        // The result lands through the background channel.
        app.on_user_command_result(Ok("hello_active\n".to_owned()));
        assert!(!app.user_command_running);
        assert_eq!(app.session.messages.len(), 2);
        assert!(app.session.messages[1].content.contains("hello_active"));
    }

    #[test]
    fn user_can_run_commands_via_slash_run_and_exclamation() {
        let mut app = test_app();
        app.input.set("!echo hello_bang");
        let action = app.submit();
        assert!(matches!(action, Action::RunUserCommand { .. }));
        assert_eq!(app.session.messages.len(), 1);
        assert_eq!(app.session.messages[0].role, Role::User);
        assert_eq!(app.session.messages[0].content, "$ echo hello_bang");
        app.on_user_command_result(Ok("hello_bang\n".to_owned()));
        assert_eq!(app.session.messages.len(), 2);
        assert_eq!(app.session.messages[1].role, Role::Tool);
        assert!(app.session.messages[1].content.contains("hello_bang"));

        app.input.set("/run echo hello_slash");
        let action = app.submit();
        assert!(matches!(action, Action::RunUserCommand { .. }));
        assert_eq!(app.session.messages.len(), 3);
        assert_eq!(app.session.messages[2].role, Role::User);
        assert_eq!(app.session.messages[2].content, "$ echo hello_slash");
        app.on_user_command_result(Ok("hello_slash\n".to_owned()));
        assert_eq!(app.session.messages.len(), 4);
        assert_eq!(app.session.messages[3].role, Role::Tool);
        assert!(app.session.messages[3].content.contains("hello_slash"));
    }

    #[test]
    fn connect_custom_provider_without_models() {
        let temp = crate::temp::unique_dir("config-test");
        let config_path = temp.join("config.json");
        let mut app = test_app();
        app.config_path = config_path;
        app.begin_connect();
        if let Some(connect) = app.connect.as_mut() {
            connect.stage = ConnectStage::AddProvider;
            connect.custom_name = "Ollama".to_owned();
            connect.custom_endpoint = "http://localhost:11434/v1".to_owned();
            connect.custom_model_id = String::new();
            connect.custom_model_name = String::new();
        }
        let action = app.submit_add_provider();
        assert!(matches!(action, Action::None));
        assert!(app.connect.is_none());
        assert_eq!(app.backend.provider_label(), "Ollama");
        assert_eq!(app.backend.base_url(), "http://localhost:11434/v1");
        assert_eq!(app.backend.model(), "");
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|n| n.message.contains("Connected to Ollama"))
        );
    }

    #[test]
    fn submit_with_empty_model_warns_and_keeps_input() {
        let mut app = test_app();
        app.backend.set_model(String::new());
        app.input.set("test message".to_owned());
        let action = app.submit();
        assert!(matches!(action, Action::None));
        assert_eq!(app.input.value, "test message");
        assert!(app.session.messages.is_empty());
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|n| n.message.contains("No model selected"))
        );
    }

    #[test]
    fn endpoint_url_matching_preserves_path_case() {
        assert!(endpoints_match(
            "https://Example.COM/v1/chat",
            "https://example.com/v1/chat/"
        ));
        assert!(!endpoints_match(
            "https://example.com/v1/Chat",
            "https://example.com/v1/chat"
        ));
    }

    #[test]
    fn shift_tab_cycles_permission_modes() {
        let mut app = test_app();
        assert_eq!(app.tools_mode, ToolsMode::On);

        // Press Shift+Tab (BackTab) -> Auto (yolo)
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::BackTab,
            crossterm::event::KeyModifiers::empty(),
        ));
        assert_eq!(app.tools_mode, ToolsMode::Auto);
        assert_eq!(app.tools_mode.label(), "yolo");
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|n| n.message.contains("yolo"))
        );

        // Press Shift+Tab (Tab with SHIFT) -> Ask
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Tab,
            crossterm::event::KeyModifiers::SHIFT,
        ));
        assert_eq!(app.tools_mode, ToolsMode::Ask);
        assert_eq!(app.tools_mode.label(), "ask");

        // Press Shift+Tab again -> On
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::BackTab,
            crossterm::event::KeyModifiers::SHIFT,
        ));
        assert_eq!(app.tools_mode, ToolsMode::On);
        assert_eq!(app.tools_mode.label(), "on");
    }

    #[test]
    fn ctrl_r_prefills_the_rename_command_with_the_current_title() {
        let mut app = test_app();
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('r'),
            crossterm::event::KeyModifiers::CONTROL,
        ));
        // An unnamed session seeds an empty title: the command and a space.
        assert_eq!(app.input.value, "/session rename ");
        assert_eq!(app.input.cursor, app.input.value.len());

        app.session.title = "Refactor the parser".to_owned();
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('R'),
            crossterm::event::KeyModifiers::CONTROL,
        ));
        assert_eq!(app.input.value, "/session rename Refactor the parser");
    }

    #[test]
    fn rename_command_sets_the_title_and_keeps_the_auto_namer_off() {
        let mut app = test_app();
        app.command("/session rename  parser refactor ");
        assert_eq!(app.session.title, "parser refactor");
        // The generated name must not overwrite a name the user chose.
        assert!(app.session.auto_named);

        // Bare `/session rename` reports the title instead of clearing it.
        app.command("/session rename");
        assert_eq!(app.session.title, "parser refactor");
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|notice| notice.message.contains("parser refactor"))
        );
    }

    #[test]
    fn shift_tab_during_approval_enables_yolo_and_approves() {
        let mut app = test_app();
        app.approval = Some(ToolApproval {
            call: ToolCall::new(
                "call-1",
                "run_command",
                r#"{"command":"cargo test","risk":true}"#,
            ),
            note: None,
            diffs: Vec::new(),
            scroll: 0,
            page_height: 0,
            popup_area: std::cell::Cell::new(ratatui::layout::Rect::default()),
        });
        app.pending_tools.push_back(ToolCall::new(
            "call-1",
            "run_command",
            r#"{"command":"cargo test","risk":true}"#,
        ));

        // Press Shift+Tab (BackTab) during approval
        let action = app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::BackTab,
            crossterm::event::KeyModifiers::empty(),
        ));
        assert!(matches!(action, Action::RunTool { .. }));
        assert_eq!(app.tools_mode, ToolsMode::Auto);
        assert!(app.approval.is_none());
        assert!(app.running_tool.is_some());
    }

    #[test]
    fn dangerous_mode_skips_every_approval_including_desktop_control() {
        let mut app = test_app();
        app.command("/set tools dangerous");
        assert_eq!(app.tools_mode, ToolsMode::Dangerous);
        // The one call that always asks in every other mode runs unattended.
        let desktop = ToolCall::new(
            "call-cu",
            "computer_use",
            r#"{"action":"open the menu","actions":[{"type":"click","x":4,"y":5}]}"#,
        );
        assert!(tools::requires_approval(&desktop));
        assert!(!app.tools_mode.needs_approval(&desktop, &HashSet::new()));
        // Turning it off is one command away and restores the prompt.
        app.command("/set tools on");
        assert!(app.tools_mode.needs_approval(&desktop, &HashSet::new()));
    }

    #[test]
    fn model_end_sse_shows_elapsed_time_since_prompt() {
        let mut app = test_app();
        app.input.set("hello assistant");
        let action = app.submit();
        assert!(matches!(action, Action::Start { .. }));
        assert!(app.prompt_started.is_some());

        // Backdate prompt_started to simulate 500ms elapsed since prompt
        let prompt_time = Instant::now() - std::time::Duration::from_millis(500);
        app.prompt_started = Some(prompt_time);

        let req_id = app.active_request_id().unwrap();
        app.on_stream_event(StreamEvent::Delta {
            request_id: req_id,
            text: "Hello there!".to_owned(),
        });
        app.on_stream_event(StreamEvent::Done { request_id: req_id });

        assert!(app.prompt_started.is_none());
        let last_message = app.session.messages.last().expect("assistant message");
        assert_eq!(last_message.role, Role::Assistant);
        let usage = last_message.usage.expect("usage recorded");
        assert!(usage.elapsed_ms >= 500);
    }

    #[test]
    fn a_capped_reply_continues_itself_until_the_budget_runs_out() {
        let mut app = test_app();
        app.input.set("write the long version");
        let mut action = app.submit();
        assert!(matches!(&action, Action::Start { .. }));

        let mut caps = 0;
        while let Action::Start { request_id, .. } = action {
            action = app.on_stream_event(StreamEvent::OutputLimit { request_id });
            caps += 1;
            assert!(
                caps <= MAX_AUTO_CONTINUES + 1,
                "the continuation cap must end the loop"
            );
        }

        // Every cap before the budget was spent starts another request; the
        // last one reports the limit instead of a fourth request.
        assert_eq!(caps, MAX_AUTO_CONTINUES + 1);
        assert!(matches!(action, Action::None));
        assert!(
            app.session
                .messages
                .last()
                .is_some_and(|message| message.error.is_some()),
            "the reported cap lands in the reply block"
        );
    }

    #[test]
    fn model_end_sse_with_intermediate_tools_measures_full_turn_elapsed() {
        let mut app = test_app();
        app.input.set("check files");
        app.submit();

        let prompt_time = Instant::now() - std::time::Duration::from_millis(1000);
        app.prompt_started = Some(prompt_time);

        let req_id = app.active_request_id().unwrap();
        // Step 1: Model calls tool
        app.on_stream_event(StreamEvent::ToolCalls {
            request_id: req_id,
            calls: vec![ToolCall::new("call-1", "read_file", r#"{"path":"foo"}"#)],
        });
        app.on_stream_event(StreamEvent::Done { request_id: req_id });

        // Intermediate Done does NOT consume prompt_started because tools are pending
        assert_eq!(app.prompt_started, Some(prompt_time));

        // Advance tools to pop and start the tool
        app.advance_tools();
        assert!(app.running_tool.is_some());

        // Tool completes
        app.on_tool_result(ToolEvent {
            call: ToolCall::new("call-1", "read_file", r#"{"path":"foo"}"#),
            result: Ok("file content".to_owned()),
        });

        // Continue after tools starts new request
        let req2_id = app.active_request_id().unwrap();
        app.on_stream_event(StreamEvent::Delta {
            request_id: req2_id,
            text: "done reading".to_owned(),
        });
        app.on_stream_event(StreamEvent::Done {
            request_id: req2_id,
        });

        // Final response end SSE consumes prompt_started and reflects total elapsed
        assert!(app.prompt_started.is_none());
        let last = app.session.messages.last().unwrap();
        let usage = last.usage.expect("usage on final message");
        assert!(usage.elapsed_ms >= 1000);
    }

    #[test]
    fn revert_lists_prompts_newest_first_and_truncates_to_the_pick() {
        let mut app = test_app();
        app.session
            .messages
            .push(Message::new(Role::User, "first prompt"));
        app.session
            .messages
            .push(Message::new(Role::Assistant, "first reply"));
        app.session
            .messages
            .push(Message::new(Role::User, "second prompt"));
        app.session
            .messages
            .push(Message::new(Role::Assistant, "second reply"));

        app.command("/revert");
        assert!(app.revert_picker.is_some());
        let matches = app.revert_matches();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0], (2, "second prompt".to_owned()));
        assert_eq!(matches[1], (0, "first prompt".to_owned()));

        // Enter on the newest prompt: the session truncates back to it, the
        // composer refills with its text, and the reply after it is gone.
        app.handle_revert_picker_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.revert_picker.is_none());
        assert_eq!(app.session.messages.len(), 2);
        assert_eq!(app.session.messages[0].content, "first prompt");
        assert_eq!(app.input.value, "second prompt");
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|notice| notice.message.contains("Editing message 3 of 4"))
        );
    }

    #[test]
    fn revert_filters_and_requeues_pasted_images() {
        let mut app = test_app();
        let mut first = Message::new(Role::User, "first prompt");
        first.images = vec![crate::session::ImageAttachment {
            name: "shot.png".to_owned(),
            data: "not-base64".to_owned(),
            thumbnail: false,
        }];
        app.session.messages.push(first);
        app.session
            .messages
            .push(Message::new(Role::Assistant, "reply"));
        app.session
            .messages
            .push(Message::new(Role::User, "second prompt"));

        app.command("/revert");
        app.handle_revert_picker_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert_eq!(app.revert_matches().len(), 1, "filter keeps first only");
        app.handle_revert_picker_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        // The picked prompt is the oldest: everything after it leaves, and
        // its pasted image returns to the composer as a placeholder.
        assert!(app.session.messages.is_empty());
        assert_eq!(app.attachments.len(), 1);
        assert_eq!(app.attachments[0].name, "paste-0.png");
        assert_eq!(app.input.value, "first prompt[Image 1]");
    }

    #[test]
    fn revert_with_no_prompts_shows_an_info_notice() {
        let mut app = test_app();
        app.command("/revert");
        assert!(app.revert_picker.is_none());
        assert!(app.notice.is_some());
    }

    #[test]
    fn a_background_command_does_not_keep_the_ticker_alive() {
        let mut app = test_app();
        app.session
            .messages
            .push(Message::new(Role::User, "$ echo hi"));
        app.user_command_running = true;
        assert!(
            !app.ticker_wanted(),
            "a queued or background command draws nothing animated"
        );
    }

    #[test]
    fn visible_animations_keep_the_ticker_alive() {
        let mut app = test_app();
        app.session.messages.push(Message::new(Role::User, "hello"));
        assert!(!app.ticker_wanted(), "an idle session does not tick");

        // A running tool ticks its footer spinner.
        app.user_command_running = false;
        app.running_tool = Some(RunningTool {
            call: crate::session::ToolCall::new("call_1", "read_file", "{}"),
            started: Instant::now(),
            before: None,
            existed: false,
            targets: Vec::new(),
        });
        assert!(app.ticker_wanted(), "a running tool animates the footer");

        // The submitted-prompt flash animates for a few ticks on its own.
        app.running_tool = None;
        app.send_pulse = Some(app.spinner);
        assert!(app.ticker_wanted(), "the send flash animates");
        app.spinner = app.spinner.wrapping_add(6);
        assert!(!app.ticker_wanted(), "the flash ends after a few ticks");
    }
}
