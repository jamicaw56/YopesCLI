//! The `/settings` popup: every `/set` key as a row, changed with the arrow
//! keys so no command name has to be remembered.
//!
//! The rows drive the same setters `/set` uses, so a value changed here and a
//! value changed by a typed command can never disagree, and the result is
//! saved and reported the same way.

use super::*;

pub(crate) struct SettingRow {
    /// The `/set` key this row drives.
    pub key: &'static str,
    pub label: &'static str,
}

pub(crate) const SETTING_ROWS: [SettingRow; 4] = [
    SettingRow {
        key: "tools",
        label: "Permission",
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
];

/// The values a row cycles through, in order.
///
/// `tools` leaves `dangerous` out on purpose: that mode stays reachable by
/// `/set tools dangerous` only, never from a key that sits next to the ones
/// people press all day.
fn choices(key: &str) -> Vec<&'static str> {
    match key {
        "tools" => vec!["on", "yolo", "ask", "off"],
        "theme" => crate::theme::THEMES.iter().map(|(name, _)| *name).collect(),
        "effort" => vec!["off", "low", "medium", "high", "xhigh"],
        "compact" => vec!["on", "off"],
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
            "theme" => crate::theme::current_name().to_owned(),
            "effort" => self
                .backend
                .reasoning_effort()
                .unwrap_or("default")
                .to_owned(),
            "compact" => if self.auto_compact { "on" } else { "off" }.to_owned(),
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
            "theme" => self.set_theme(&value),
            "effort" => self.set_effort(&value),
            "compact" => {
                self.set_compact(&value);
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
        // Down moves to the theme row, and Right picks the next theme name.
        press(&mut app, KeyCode::Down);
        assert_eq!(app.settings_index, 1);
        let before = crate::theme::current_name();
        press(&mut app, KeyCode::Right);
        assert_ne!(crate::theme::current_name(), before);
        // The selection wraps at both ends instead of sticking.
        for _ in 0..SETTING_ROWS.len() {
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(app.settings_index, 1);
        for _ in 0..SETTING_ROWS.len() {
            press(&mut app, KeyCode::Up);
        }
        assert_eq!(app.settings_index, 1);
    }

    #[test]
    fn auto_compact_row_steps_between_on_and_off() {
        let mut app = test_app();
        app.command("/settings");
        app.settings_index = 3;
        // The test app starts with auto compact off: Right turns it on, and
        // Right again turns it back off.
        assert!(!app.auto_compact);
        press(&mut app, KeyCode::Right);
        assert!(app.auto_compact);
        press(&mut app, KeyCode::Right);
        assert!(!app.auto_compact);
    }
}
