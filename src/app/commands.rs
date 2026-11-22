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

/// One row of the command palette: a command name, or an argument for the
/// command already typed in the composer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Suggestion {
    /// What the row shows: the command, or the argument word.
    pub label: String,
    /// What the composer holds after Tab or Enter.
    pub completion: String,
    pub description: &'static str,
    /// A command runs on Enter; an argument only completes, because the line
    /// is still incomplete.
    pub submit: bool,
}

/// The words a command takes, keyed by the command and its words so far. The
/// palette offers them once the command name and a space are typed, so no
/// usage line has to be memorised before the first use.
fn argument_words(command: &str, words: &[&str]) -> Vec<(String, &'static str)> {
    let list = |entries: &[(&str, &'static str)]| -> Vec<(String, &'static str)> {
        entries
            .iter()
            .map(|(word, description)| ((*word).to_owned(), *description))
            .collect()
    };
    match (command, words) {
        ("/set", []) => list(&[
            ("tools", "Permission mode for tool calls"),
            ("theme", "Color palette"),
            ("system", "Custom system prompt"),
            ("compact", "Automatic context compaction"),
            ("effort", "Reasoning effort"),
        ]),
        ("/set", ["tools"]) => list(&[
            ("on", "Edits run automatically; risky commands ask"),
            ("yolo", "Every command and edit runs without asking"),
            ("ask", "Every risky command asks first"),
            ("off", "Tools are not offered to the model"),
            (
                "dangerous",
                "Everything runs unattended, computer_use included",
            ),
        ]),
        ("/set", ["theme"]) => crate::theme::list()
            .into_iter()
            .map(|name| (name.to_owned(), "Color palette"))
            .collect(),
        ("/set", ["effort"]) => list(&[
            ("off", "Provider default"),
            ("low", "Low reasoning effort"),
            ("medium", "Medium reasoning effort"),
            ("high", "High reasoning effort"),
            ("xhigh", "Maximum reasoning effort"),
        ]),
        ("/set", ["compact"]) => list(&[
            ("on", "Summarize history when the context fills"),
            ("off", "Never compact automatically"),
        ]),
        ("/provider", []) => list(&[
            ("connect", "Connect a provider or a custom endpoint"),
            ("disconnect", "Forget a saved provider"),
            ("models", "Pick a model"),
        ]),
        ("/session", []) => list(&[
            ("list", "Load a saved session"),
            ("new", "Start a fresh session"),
            ("tab", "Open a new tab"),
            ("rename", "Rename this session"),
            ("close", "Close this tab"),
            ("next", "Next tab"),
            ("prev", "Previous tab"),
        ]),
        _ => Vec::new(),
    }
}

/// Argument suggestions for the line being typed: the trailing word is the
/// one completed, so `/set theme da` offers `dark`.
pub(crate) fn argument_suggestions(value: &str) -> Vec<Suggestion> {
    let Some((command, rest)) = value.split_once(' ') else {
        return Vec::new();
    };
    if !command.starts_with('/') {
        return Vec::new();
    }
    let (head, word) = match rest.rsplit_once(' ') {
        Some((head, word)) => (head, word),
        None => ("", rest),
    };
    let words: Vec<&str> = if head.is_empty() {
        Vec::new()
    } else {
        head.split(' ').collect()
    };
    let prefix = word.to_ascii_lowercase();
    argument_words(command, &words)
        .into_iter()
        .filter(|(candidate, _)| candidate.starts_with(&prefix))
        .map(|(candidate, description)| {
            let mut completion = format!("{command} ");
            for word in &words {
                completion.push_str(word);
                completion.push(' ');
            }
            completion.push_str(&candidate);
            Suggestion {
                label: candidate,
                completion,
                description,
                submit: false,
            }
        })
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
        assert_eq!(
            command_names("/s"),
            vec!["/session", "/set", "/settings", "/skills"]
        );
        assert_eq!(command_names("/p"), vec!["/provider"]);
        assert_eq!(command_names("/a"), vec!["/agents"]);
        assert_eq!(command_names("/q"), vec!["/quit"]);
        // The query is matched case-insensitively against the name.
        assert!(command_names("/MO").is_empty());
        assert!(command_names("/model gpt").is_empty());
        assert!(command_names("hello").is_empty());
    }

    #[test]
    fn arguments_complete_once_the_command_name_is_typed() {
        use super::argument_suggestions;

        let labels = |value: &str| {
            argument_suggestions(value)
                .into_iter()
                .map(|suggestion| suggestion.label)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            labels("/set "),
            vec!["tools", "theme", "system", "compact", "effort"]
        );
        assert_eq!(labels("/set t"), vec!["tools", "theme"]);
        assert_eq!(labels("/set theme da"), vec!["dark"]);
        assert_eq!(
            labels("/provider "),
            vec!["connect", "disconnect", "models"]
        );
        assert_eq!(labels("/session n"), vec!["new", "next"]);
        // The completion keeps the words already typed.
        let dark = argument_suggestions("/set theme da")
            .into_iter()
            .next()
            .expect("suggestion");
        assert_eq!(dark.completion, "/set theme dark");
        assert!(!dark.submit, "an argument completion stays editable");
        // Free-text arguments, and a line past its known words, offer nothing.
        assert!(labels("/goal ").is_empty());
        assert!(labels("/set theme dark ").is_empty());
        assert!(labels("hello ").is_empty());
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
