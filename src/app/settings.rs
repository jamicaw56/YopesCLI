//! The `/settings` popup: one row per user-facing setting, changed with the
//! arrow keys so no command name or key has to be remembered.
//!
//! The rows drive the same setters the rest of the app uses, so a value
//! changed here and a value changed by a typed command never disagree, and
//! the result is saved and reported the same way.

use super::*;

pub(crate) struct SettingRow {
    /// The `/set` key this row drives.
    pub key: &'static str,
    pub label: &'static str,
}

pub(crate) const SETTING_ROWS: [SettingRow; 7] = [
    SettingRow {
        key: "tools",
        label: "Permission",
    },
    SettingRow {
        key: "mode",
        label: "Session mode",
    },
    SettingRow {
        key: "theme",
        label: "Theme",
    },
    SettingRow {
        key: "effort",
        label: "Reasoning effort",
    },
    SettingRow {
        key: "compact",
        label: "Auto compact",
    },
    SettingRow {
        key: "diff",
        label: "Diff view",
    },
    SettingRow {
        key: "discord",
        label: "Discord presence",
    },
];

/// The values a row cycles through, in order.
///
/// `tools` leaves `dangerous` out on purpose: that mode stays reachable by
/// `/set tools dangerous` only, never from a key that sits next to the ones
/// people press all day.
fn choices(key: &str) -> Vec<&'static str> {
    match key {
        "tools" => vec!["on", "yolo", "ask", "off"],
        "mode" => vec!["build", "plan", "chat"],
        "theme" => crate::theme::list(),
        "effort" => vec!["off", "low", "medium", "high", "xhigh"],
        "compact" => vec!["on", "off"],
        "diff" => vec!["side-by-side", "unified"],
        // Only two states: the id itself comes from `/set discord <id>`.
        "discord" => vec!["off", "on"],
        _ => Vec::new(),
    }
}

impl App {
    /// `/settings`: open the popup on the first row.
    pub(crate) fn open_settings(&mut self) {
        if self.approval.is_some() {
            self.info("Approve or deny the pending tool first.");
            return;
        }
        if self.connect.is_some() {
            self.info("Finish the provider dialog first.");
            return;
        }
        self.settings_open = true;
        self.settings_index = 0;
        self.palette_hidden = true;
    }

    /// The value a row shows, read from the live state rather than a copy.
    pub(crate) fn setting_value(&self, key: &str) -> String {
        match key {
            "tools" => self.tools_mode.label().to_owned(),
            "mode" => self.mode.label().to_owned(),
            "theme" => crate::theme::current_name().to_owned(),
            "effort" => self
                .backend
                .reasoning_effort()
                .unwrap_or("default")
                .to_owned(),
            "compact" => if self.auto_compact { "on" } else { "off" }.to_owned(),
            "diff" => if self.diff_sides {
                "side-by-side"
            } else {
                "unified"
            }
            .to_owned(),
            "discord" => if self.discord_client_id.is_some() {
                "on"
            } else {
                "off"
            }
            .to_owned(),
            _ => String::new(),
        }
    }

    /// Move one row's value forward or back, applying and saving it exactly
    /// as the matching `/set` command would.
    pub(crate) fn cycle_setting(&mut self, index: usize, forward: bool) {
        let Some(row) = SETTING_ROWS.get(index) else {
            return;
        };
        let choices = choices(row.key);
        if choices.is_empty() {
            return;
        }
        let current = self.setting_value(row.key);
        // "default" is the effort row's word for "no value set": it sits on
        // the off position, so the first Right press steps past it.
        let position = choices
            .iter()
            .position(|choice| choice.eq_ignore_ascii_case(&current))
            .or_else(|| (current == "default").then_some(0));
        let next = match position {
            Some(position) if forward => (position + 1) % choices.len(),
            Some(position) => (position + choices.len() - 1) % choices.len(),
            // An unknown value starts the cycle at the first choice.
            None => 0,
        };
        let value = choices[next].to_owned();
        match row.key {
            "tools" => self.set_tools_mode(&value),
            "mode" => {
                self.mode = match value.as_str() {
                    "plan" => Mode::Plan,
                    "chat" => Mode::Chat,
                    _ => Mode::Build,
                };
                self.info(format!(
                    "Mode: {} — Tab again to cycle build · plan · chat",
                    self.mode.label()
                ));
            }
            "theme" => self.set_theme(&value),
            "effort" => self.set_effort(&value),
            "compact" => {
                self.set_compact(&value);
            }
            "diff" => {
                let sides = value == "side-by-side";
                if sides != self.diff_sides {
                    self.toggle_diff_view();
                }
                self.info(format!(
                    "Diff view: {}",
                    if self.diff_sides {
                        "side-by-side"
                    } else {
                        "unified"
                    }
                ));
            }
            "discord" => {
                if value == "on" {
                    // Turning it on needs the id of a Discord application; the
                    // row only reports where to put it.
                    if self.discord_client_id.is_none() {
                        self.info(
                            "Discord presence: set the application id with /set discord <id> first",
                        );
                    } else {
                        self.info("Discord presence: on");
                    }
                } else {
                    self.set_discord("off");
                }
            }
            _ => {}
        }
    }

    /// Keys while the popup is open: it owns the keyboard until it closes.
    pub(crate) fn handle_settings_key(&mut self, key: KeyEvent) -> Action {
        let count = SETTING_ROWS.len();
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q' | 'Q') => self.settings_open = false,
            KeyCode::Up | KeyCode::BackTab => {
                self.settings_index = (self.settings_index + count - 1) % count;
            }
            KeyCode::Down | KeyCode::Tab => {
                self.settings_index = (self.settings_index + 1) % count;
            }
            KeyCode::Left => self.cycle_setting(self.settings_index, false),
            KeyCode::Right => self.cycle_setting(self.settings_index, true),
            _ => {}
        }
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::SETTING_ROWS;
    use crate::app::ToolsMode;
    use crate::app::turn::tests::test_app;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn press(app: &mut crate::app::App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// The row driving a `/set` key, so a test names the setting instead of
    /// hard-coding its position in the list.
    fn row(key: &str) -> usize {
        SETTING_ROWS
            .iter()
            .position(|row| row.key == key)
            .expect("row")
    }

    #[test]
    fn every_row_shows_a_value_and_cycles_it() {
        let mut app = test_app();
        app.command("/settings");
        for (index, setting) in SETTING_ROWS.iter().enumerate() {
            assert!(
                !app.setting_value(setting.key).is_empty(),
                "{} shows no value",
                setting.key
            );
            let before = app.setting_value(setting.key);
            app.settings_index = index;
            press(&mut app, KeyCode::Right);
            // Discord presence needs an application id before it can turn on:
            // the row reports that instead of flipping to a presence that
            // never connects.
            if setting.key == "discord" {
                assert_eq!(app.setting_value(setting.key), before);
                continue;
            }
            assert_ne!(
                app.setting_value(setting.key),
                before,
                "{} did not cycle",
                setting.key
            );
        }
    }

    #[test]
    fn the_discord_row_needs_an_id_before_it_turns_on() {
        let dir = crate::temp::unique_dir("yopes-discord-setting");
        let mut app = test_app();
        app.config_path = dir.join("config.json");
        app.command("/settings");
        app.settings_index = row("discord");
        // No application id: the row explains instead of flipping to a
        // presence that could never connect.
        press(&mut app, KeyCode::Right);
        assert_eq!(app.setting_value("discord"), "off");
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|notice| notice.message.contains("application id"))
        );
        // With the id set, the row cycles both ways.
        app.command("/set discord 123456789012345678");
        assert_eq!(app.setting_value("discord"), "on");
        app.command("/settings");
        app.settings_index = row("discord");
        press(&mut app, KeyCode::Right);
        assert_eq!(app.setting_value("discord"), "off");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn slash_settings_opens_the_popup_and_esc_closes_it() {
        let mut app = test_app();
        app.command("/settings");
        assert!(app.settings_open);
        assert_eq!(app.settings_index, 0);
        press(&mut app, KeyCode::Esc);
        assert!(!app.settings_open);
    }

    #[test]
    fn arrows_move_the_selection_and_cycle_the_value() {
        let mut app = test_app();
        app.command("/settings");
        // Row 0 is Permission: Right steps on -> yolo.
        press(&mut app, KeyCode::Right);
        assert_eq!(app.tools_mode, ToolsMode::Auto);
        // Left steps back to on.
        press(&mut app, KeyCode::Left);
        assert_eq!(app.tools_mode, ToolsMode::On);
        // Two rows down is the theme row, and Right picks the next name.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.settings_index, row("theme"));
        let before = crate::theme::current_name();
        press(&mut app, KeyCode::Right);
        assert_ne!(crate::theme::current_name(), before);
        // The selection wraps at both ends instead of sticking.
        let theme = app.settings_index;
        for _ in 0..SETTING_ROWS.len() {
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(app.settings_index, theme);
        for _ in 0..SETTING_ROWS.len() {
            press(&mut app, KeyCode::Up);
        }
        assert_eq!(app.settings_index, theme);
    }

    #[test]
    fn auto_compact_row_steps_between_on_and_off() {
        let mut app = test_app();
        app.command("/settings");
        app.settings_index = row("compact");
        // The test app starts with auto compact off: Right turns it on, and
        // Right again turns it back off.
        assert!(!app.auto_compact);
        press(&mut app, KeyCode::Right);
        assert!(app.auto_compact);
        press(&mut app, KeyCode::Right);
        assert!(!app.auto_compact);
    }

    #[test]
    fn the_session_mode_row_cycles_build_plan_chat() {
        use crate::app::Mode;

        let mut app = test_app();
        app.command("/settings");
        app.settings_index = row("mode");
        assert_eq!(app.mode, Mode::Build);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.mode, Mode::Plan);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.mode, Mode::Chat);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.mode, Mode::Plan);
    }

    #[test]
    fn the_diff_view_row_switches_the_transcript_diffs() {
        let mut app = test_app();
        app.command("/settings");
        app.settings_index = row("diff");
        // The test app draws diffs side by side.
        assert!(app.diff_sides);
        press(&mut app, KeyCode::Right);
        assert!(!app.diff_sides);
        press(&mut app, KeyCode::Right);
        assert!(app.diff_sides);
    }
}
