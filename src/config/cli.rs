//! CLI argument parsing: the flags and subcommands the binary accepts.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use super::ProviderKind;

#[derive(Debug, Parser)]
#[command(name = "yopes", version = crate::update::current_version(), about = "A fast, session-first AI harness")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Model backend: openai or ollama.
    #[arg(long, value_enum, global = true)]
    pub(super) provider: Option<ProviderKind>,

    /// Model name sent to the provider.
    #[arg(long, global = true)]
    pub(super) model: Option<String>,

    /// Provider base URL, without /chat/completions or /api/chat.
    #[arg(long, global = true)]
    pub(super) base_url: Option<String>,

    /// Session JSON path. Defaults to the platform data directory.
    #[arg(long, global = true)]
    pub(super) session: Option<PathBuf>,

    /// Resume the most recent session.
    #[arg(short, long, global = true)]
    pub(super) resume: bool,

    /// Do not load or save the session.
    #[arg(long, global = true)]
    pub(super) no_save: bool,

    /// Custom system prompt text, appended to the built-in one.
    #[arg(long, global = true)]
    pub(super) system_prompt: Option<String>,
}

impl Cli {
    /// Whether `--resume` was asked for. The flag itself stays private to the
    /// config module; `main` needs the answer to continue a cut-off session.
    pub fn resume_requested(&self) -> bool {
        self.resume
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Check for updates, install the latest release, and restart.
    Update,
    /// Run one task without the interface, then exit when the agent is idle.
    /// Exit code 0 on success, non-zero when the provider fails.
    Run {
        /// The task text, sent as one user message.
        task: String,
    },
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command};
    use crate::config::{AuthKind, ProviderKind};
    use clap::Parser;

    #[test]
    fn provider_values_are_stable_cli_names() {
        let cli = Cli::try_parse_from(["yopes", "--provider", "openai"]).unwrap();
        assert_eq!(cli.provider, Some(ProviderKind::OpenAi));
        assert!(Cli::try_parse_from(["yopes", "--provider", "invalid"]).is_err());
        assert!(Cli::try_parse_from(["yopes", "--provider", "custom"]).is_err());
        assert!(Cli::try_parse_from(["yopes", "--provider", "demo"]).is_err());
    }

    #[test]
    fn custom_auth_names_are_stable() {
        assert_eq!(serde_json::to_string(&AuthKind::Jwt).unwrap(), "\"jwt\"");
        assert_eq!(
            serde_json::to_string(&AuthKind::XApiKey).unwrap(),
            "\"x-api-key\""
        );
        assert_eq!(
            serde_json::from_str::<AuthKind>("\"none\"").unwrap(),
            AuthKind::None
        );
    }

    #[test]
    fn the_resume_flag_parses() {
        let cli = Cli::try_parse_from(["yopes", "--resume"]).unwrap();
        assert!(cli.resume);
        let cli = Cli::try_parse_from(["yopes", "-r"]).unwrap();
        assert!(cli.resume);
        let cli = Cli::try_parse_from(["yopes"]).unwrap();
        assert!(!cli.resume);
    }

    #[test]
    fn the_update_subcommand_parses() {
        let cli = Cli::try_parse_from(["yopes", "update"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Update)));
    }

    #[test]
    fn the_run_subcommand_takes_one_task() {
        let cli = Cli::try_parse_from(["yopes", "run", "fix the failing tests"]).unwrap();
        assert!(
            matches!(cli.command, Some(Command::Run { ref task }) if task == "fix the failing tests")
        );
        // The task is required: `run` alone is a usage error.
        assert!(Cli::try_parse_from(["yopes", "run"]).is_err());
    }

    #[test]
    fn flags_parse_after_the_run_subcommand() {
        let cli = Cli::try_parse_from([
            "yopes",
            "run",
            "fix the failing tests",
            "--no-save",
            "--system-prompt",
            "extra",
            "--model",
            "gpt-5",
        ])
        .unwrap();
        assert!(cli.no_save);
        assert_eq!(cli.system_prompt.as_deref(), Some("extra"));
        assert_eq!(cli.model.as_deref(), Some("gpt-5"));
    }
}
