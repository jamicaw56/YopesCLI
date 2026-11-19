//! The slash-command palette: the table the composer matches against and
//! the completion rules built from it.
//!
//! The table stays short on purpose: related actions live under one command
//! (`/session`, `/provider`, `/set`) instead of a dozen one-word entries, so
//! the palette fits on one screen and each verb has one obvious home.

pub(crate) const EFFORT_CHOICES: &str = "Usage: /set effort off|none|low|medium|high|xhigh";

/// The keys `/set` accepts, so an unknown one can list them.
pub(crate) const SET_KEYS: &str = "tools, theme, system, compact, effort";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub argument: Option<&'static str>,
}

impl CommandSpec {
    pub fn display_name(self) -> String {
        self.argument.map_or_else(
            || self.name.to_owned(),
            |argument| format!("{} [{argument}]", self.name),
        )
    }

    pub(crate) fn completion(self) -> String {
        if self.argument.is_some() {
            format!("{} ", self.name)
        } else {
            self.name.to_owned()
        }
    }
}

pub const COMMANDS: [CommandSpec; 12] = [
    CommandSpec {
        name: "/help",
        description: "Show keys, commands and the version",
        argument: None,
    },
    CommandSpec {
        name: "/session",
        description: "Sessions and tabs: list, new, tab, rename, close, next, prev, 1-9",
        argument: Some("action"),
    },
    CommandSpec {
        name: "/provider",
        description: "Connect, disconnect, or pick a model",
        argument: Some("action"),
    },
    CommandSpec {
        name: "/set",
        description: "Change a setting: tools, theme, system, compact, effort",
        argument: Some("key"),
    },
    CommandSpec {
        name: "/settings",
        description: "Change settings with the arrow keys",
        argument: None,
    },
    CommandSpec {
        name: "/run",
        description: "Run a shell command in workspace",
        argument: Some("command"),
    },
    CommandSpec {
        name: "/cd",
        description: "Change the working directory",
        argument: Some("path"),
    },
    CommandSpec {
        name: "/agents",
        description: "View subagent responses from delegate calls",
        argument: None,
    },
    CommandSpec {
        name: "/skills",
        description: "List installed skills",
        argument: None,
    },
    CommandSpec {
        name: "/revert",
        description: "Edit and re-run a previous prompt",
        argument: None,
    },
    CommandSpec {
        name: "/goal",
        description: "Set the objective the model must reach no matter what",
        argument: Some("goal"),
    },
    CommandSpec {
        name: "/quit",
        description: "Save and exit",
        argument: None,
    },
];

pub(crate) fn matching_commands(value: &str) -> Vec<&'static CommandSpec> {
    if !value.starts_with('/') || value.chars().any(char::is_whitespace) {
        return Vec::new();
    }
    let query = value[1..].to_ascii_lowercase();
    COMMANDS
        .iter()
        .filter(|command| command.name[1..].starts_with(query.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{COMMANDS, matching_commands};
    use crate::app::turn::tests::test_app;

    fn command_names(value: &str) -> Vec<&'static str> {
        matching_commands(value)
            .iter()
            .map(|command| command.name)
            .collect()
    }

    #[test]
    fn theme_setting_shows_the_active_theme_and_switches() {
        let mut app = test_app();
        app.command("/set theme");
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|notice| notice.message.starts_with("Theme:"))
        );
        app.command("/set theme dark");
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|notice| notice.message.contains("Theme: dark"))
        );
    }

    #[test]
    fn palette_matches_command_prefixes() {
        assert_eq!(
            command_names("/"),
            vec![
                "/help",
                "/session",
                "/provider",
                "/set",
                "/settings",
                "/run",
                "/cd",
                "/agents",
                "/skills",
                "/revert",
                "/goal",
                "/quit",
            ]
        );
        assert_eq!(command_names("/c"), vec!["/cd"]);
        assert_eq!(command_names("/r"), vec!["/run", "/revert"]);
        assert_eq!(command_names("/g"), vec!["/goal"]);
        assert_eq!(command_names("/s"), vec!["/session", "/set", "/settings", "/skills"]);
        assert_eq!(command_names("/p"), vec!["/provider"]);
        assert_eq!(command_names("/a"), vec!["/agents"]);
        assert_eq!(command_names("/q"), vec!["/quit"]);
        // The query is matched case-insensitively against the name.
        assert!(command_names("/MO").is_empty());
        assert!(command_names("/model gpt").is_empty());
        assert!(command_names("hello").is_empty());
    }

    #[test]
    fn completion_adds_a_space_only_when_arguments_are_expected() {
        let session = COMMANDS
            .iter()
            .find(|command| command.name == "/session")
            .unwrap();
        assert_eq!(session.completion(), "/session ");
        let help = COMMANDS
            .iter()
            .find(|command| command.name == "/help")
            .unwrap();
        assert_eq!(help.completion(), "/help");
    }
}
