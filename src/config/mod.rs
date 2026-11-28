//! Configuration: CLI arguments, the resolved `Config`, provider kinds, and
//! the paths where state lives.
//!
//! `mod.rs` holds the types every other module touches. `cli` owns the
//! argument parsing, and `saved` owns the saved-config JSON and everything
//! that reads or writes it.

mod cli;
mod saved;

use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::credentials;

pub use cli::{Cli, Command};
use saved::{custom_providers, load_saved, resolve_api_key};
pub(crate) use saved::{
    delete_custom_provider, delete_saved_model, disconnect_provider, enable_discord,
    last_seen_version, load_connected_providers, load_custom_providers, load_discord_client_id,
    load_endpoint_models, load_endpoint_secret, load_weather, rename_custom_provider,
    save_auto_compact, save_connection, save_diff_sides, save_discord_client_id,
    save_endpoint_secret, save_last_seen_version, save_model, save_permission,
    save_reasoning_effort, save_system_prompt, save_theme, save_weather, update_endpoint_models,
    update_model_context_window,
};

const CONFIG_VERSION: u32 = 1;

pub const FREE_PROVIDER_LABEL: &str = "free";
pub const FREE_BASE_URL: &str = "https://umpirer.us.ci/v1";
pub const FREE_API_KEY: &str = "ump_071482742051b4e2a4037bb2f0f9fcb8c16e671551232ecd";
pub const FREE_MODEL_ID: &str = "deepseek-v4.1-flash-1017";
pub const FREE_MODEL_NAME: &str = "Auto";

pub fn free_saved_model() -> SavedModel {
    SavedModel {
        id: FREE_MODEL_ID.to_owned(),
        name: FREE_MODEL_NAME.to_owned(),
        input_cost: Some(0.0),
        output_cost: Some(0.0),
        context_window: Some(128_000),
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    #[default]
    #[value(skip)]
    Unconfigured,
    #[value(name = "openai")]
    OpenAi,
    Ollama,
    #[value(name = "anthropic")]
    Anthropic,
    #[value(name = "gemini")]
    Gemini,
    #[value(skip)]
    Custom,
}

impl ProviderKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unconfigured => "unconfigured",
            Self::OpenAi => "openai",
            Self::Ollama => "ollama",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::Custom => "custom",
        }
    }

    fn parse_env(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "none" | "unconfigured" => Ok(Self::Unconfigured),
            "openai" | "openai-compatible" => Ok(Self::OpenAi),
            "ollama" => Ok(Self::Ollama),
            "anthropic" | "claude" => Ok(Self::Anthropic),
            "gemini" | "google" => Ok(Self::Gemini),
            _ => bail!(
                "invalid YOPES_PROVIDER `{value}`; expected openai, ollama, anthropic, or gemini"
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthKind {
    #[default]
    Bearer,
    Jwt,
    XApiKey,
    None,
}

impl AuthKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Bearer => "Bearer token",
            Self::Jwt => "JWT token",
            Self::XApiKey => "X-API-Key",
            Self::None => "No auth",
        }
    }

    pub const fn requires_token(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[cfg(test)]
mod auth_tests {
    use super::AuthKind;

    #[test]
    fn custom_auth_schemes_have_expected_labels_and_saved_names() {
        assert_eq!(AuthKind::Bearer.label(), "Bearer token");
        assert_eq!(AuthKind::Jwt.label(), "JWT token");
        assert_eq!(AuthKind::XApiKey.label(), "X-API-Key");
        assert_eq!(AuthKind::None.label(), "No auth");
        for (auth, json) in [
            (AuthKind::Bearer, "\"bearer\""),
            (AuthKind::Jwt, "\"jwt\""),
            (AuthKind::XApiKey, "\"x-api-key\""),
            (AuthKind::None, "\"none\""),
        ] {
            assert_eq!(serde_json::to_string(&auth).unwrap(), json);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedModel {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Dollars per million input tokens, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_cost: Option<f64>,
    /// Dollars per million output tokens, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_cost: Option<f64>,
    /// Manually set context-window size in tokens (e.g. 128000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub auth: AuthKind,
    pub models: Vec<SavedModel>,
    /// Where to fetch the provider's model list; defaults to `<base>/models`.
    pub models_url: Option<String>,
    /// Plaintext token when stored without encryption; `None` when encrypted or absent.
    pub token: Option<String>,
    pub encrypted: bool,
    /// The stored secret, encrypted or not.
    pub secret: StoredSecret,
}

/// A provider the user has connected, remembered so its key survives a switch
/// to another provider.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedProvider {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub provider_kind: ProviderKind,
    #[serde(default)]
    pub provider_name: String,
    #[serde(default)]
    pub secret: StoredSecret,
    /// The model list this provider last offered, so /models can show models
    /// from every connected provider, not only the active one.
    #[serde(default)]
    pub models: Vec<SavedModel>,
}

/// A connected provider, for the interface to show what is already set up.
#[derive(Clone, Debug)]
pub struct ConnectedProvider {
    pub base_url: String,
    pub provider_id: Option<String>,
    pub provider_kind: ProviderKind,
    pub name: String,
    #[allow(dead_code)]
    pub has_key: bool,
    #[allow(dead_code)]
    pub encrypted: bool,
    pub models: Vec<SavedModel>,
}

pub struct Config {
    pub provider: ProviderKind,
    pub provider_label: String,
    pub model: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub auth: AuthKind,
    pub models: Vec<SavedModel>,
    /// Custom system prompt text, appended to the built-in one.
    pub system_prompt: Option<String>,
    pub custom_providers: Vec<CustomProvider>,
    pub session_path: Option<PathBuf>,
    /// Directory holding every saved session file, when saving is on.
    pub sessions_dir: Option<PathBuf>,
    pub config_path: PathBuf,
    /// Tavily key backing the web_search tool.
    pub tavily_api_key: Option<String>,
    /// Whether compaction runs automatically when the context grows large.
    pub auto_compact: bool,
    /// Reasoning effort to request from OpenAI-compatible backends.
    pub reasoning_effort: Option<String>,
    /// UI theme name; one of the built-ins in `crate::theme`.
    pub theme: String,
    /// Saved permission mode (`on`, `yolo`, `ask`), when one was set.
    pub permission: Option<String>,
    /// Saved diff layout, when one was set.
    pub diff_sides: Option<bool>,
}

pub struct Connection {
    pub provider: ProviderKind,
    pub provider_name: String,
    pub model: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    pub auth: AuthKind,
    pub models: Vec<SavedModel>,
    /// Endpoint serving the provider's model list, when it differs from the
    /// default `<base>/models`.
    pub models_url: Option<String>,
    /// Id of the saved custom provider to update; `None` creates a new entry.
    pub custom_provider_id: Option<String>,
    pub provider_id: Option<String>,
    /// The key for this endpoint is already stored: keep that secret as it is
    /// rather than writing a new one.
    pub reuse_secret: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StoredSecret {
    #[serde(default)]
    pub encrypted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ciphertext: Option<String>,
}

impl StoredSecret {
    /// Whether a key is stored here at all.
    pub fn has_key(&self) -> bool {
        self.api_key.is_some() || self.ciphertext.is_some()
    }

    /// The key as stored, when it was saved without a password.
    pub fn plaintext(&self) -> Option<&str> {
        if self.encrypted {
            return None;
        }
        self.api_key
            .as_deref()
            .filter(|api_key| !api_key.trim().is_empty())
    }

    /// Unlock the key with a password. A secret saved without one opens with
    /// any password, so callers can treat the two the same way.
    pub fn unlock(&self, password: &str) -> Result<String> {
        if !self.encrypted {
            return Ok(self.plaintext().unwrap_or_default().to_owned());
        }
        let (Some(salt), Some(nonce), Some(ciphertext)) =
            (&self.salt, &self.nonce, &self.ciphertext)
        else {
            bail!("the stored API key is missing its encryption data");
        };
        credentials::open(password, salt, nonce, ciphertext)
    }

    /// Prompt for the password and unlock the key.
    fn resolve(&self) -> Result<Option<String>> {
        if !self.encrypted {
            return Ok(self
                .api_key
                .clone()
                .filter(|api_key| !api_key.trim().is_empty()));
        }
        let password = credentials::prompt_password("API key password: ")?;
        if password.is_empty() {
            bail!("an encryption password is required to unlock the stored API key");
        }
        Ok(Some(self.unlock(&password)?))
    }
}

impl Config {
    pub fn from_cli(cli: Cli) -> Result<Self> {
        Self::resolve(cli)
    }

    fn resolve(cli: Cli) -> Result<Self> {
        let config_path = default_config_path();
        migrate_legacy_config(&config_path);
        let saved = load_saved(&config_path)?;

        let env_provider = env::var("YOPES_PROVIDER")
            .ok()
            .map(|value| ProviderKind::parse_env(&value))
            .transpose()?;
        let env_key = env::var("OPENAI_API_KEY")
            .or_else(|_| env::var("ANTHROPIC_API_KEY"))
            .or_else(|_| env::var("GEMINI_API_KEY"))
            .or_else(|_| env::var("GOOGLE_API_KEY"))
            .ok()
            .filter(|api_key| !api_key.trim().is_empty());
        let provider = cli
            .provider
            .or(env_provider)
            .or_else(|| saved.as_ref().map(|saved| saved.provider))
            .or_else(|| {
                if env::var("ANTHROPIC_API_KEY").is_ok_and(|k| !k.trim().is_empty()) {
                    Some(ProviderKind::Anthropic)
                } else if env::var("GEMINI_API_KEY")
                    .or_else(|_| env::var("GOOGLE_API_KEY"))
                    .is_ok_and(|k| !k.trim().is_empty())
                {
                    Some(ProviderKind::Gemini)
                } else {
                    env_key.as_ref().map(|_| ProviderKind::OpenAi)
                }
            })
            .unwrap_or(ProviderKind::Unconfigured);
        let saved_for_provider = saved.as_ref().filter(|saved| saved.provider == provider);

        let explicit_session = cli
            .session
            .or_else(|| env::var("YOPES_SESSION").ok().map(PathBuf::from));

        let sessions_dir = if cli.no_save {
            None
        } else if let Some(ref path) = explicit_session {
            let parent = path.parent().unwrap_or_else(|| Path::new("."));
            if parent.file_name().and_then(|s| s.to_str()) == Some("sessions") {
                Some(parent.to_path_buf())
            } else {
                Some(parent.join("sessions"))
            }
        } else {
            Some(default_sessions_dir())
        };

        if let Some(dir) = &sessions_dir {
            migrate_legacy_sessions(dir);
            migrate_legacy_session(dir);
        }

        let session_path = if cli.no_save {
            None
        } else if let Some(path) = explicit_session {
            Some(path)
        } else if cli.resume {
            sessions_dir
                .as_ref()
                .and_then(|dir| newest_session_path(dir))
                .or_else(|| sessions_dir.as_ref().map(|dir| new_session_path(dir)))
        } else {
            sessions_dir.as_ref().map(|dir| new_session_path(dir))
        };

        if provider == ProviderKind::Unconfigured {
            return Ok(Self {
                provider: ProviderKind::OpenAi,
                provider_label: FREE_PROVIDER_LABEL.to_owned(),
                model: FREE_MODEL_ID.to_owned(),
                base_url: FREE_BASE_URL.to_owned(),
                api_key: Some(FREE_API_KEY.to_owned()),
                auth: AuthKind::Bearer,
                models: vec![free_saved_model()],
                system_prompt: None,
                custom_providers: custom_providers(saved.as_ref()),
                session_path,
                sessions_dir,
                config_path,
                tavily_api_key: None,
                auto_compact: saved.as_ref().is_none_or(|saved| saved.auto_compact),
                reasoning_effort: saved
                    .as_ref()
                    .and_then(|saved| saved.reasoning_effort.clone()),
                theme: saved
                    .as_ref()
                    .and_then(|saved| saved.theme.clone())
                    .unwrap_or_else(|| "dark".to_owned()),
                permission: saved.as_ref().and_then(|saved| saved.permission.clone()),
                diff_sides: saved.as_ref().and_then(|saved| saved.diff_sides),
            });
        }

        let provider_label = saved_for_provider
            .map(|saved| saved.provider_name.trim())
            .filter(|name| !name.is_empty())
            .unwrap_or(provider.as_str())
            .to_owned();

        let model = cli
            .model
            .or_else(|| env::var("YOPES_MODEL").ok())
            .or_else(|| {
                saved_for_provider
                    .map(|saved| saved.model.clone())
                    .filter(|model| !model.trim().is_empty())
            })
            .unwrap_or_else(|| default_model(provider));

        let provider_base_var = match provider {
            ProviderKind::OpenAi => Some("OPENAI_BASE_URL"),
            ProviderKind::Ollama => Some("OLLAMA_BASE_URL"),
            ProviderKind::Anthropic => Some("ANTHROPIC_BASE_URL"),
            ProviderKind::Gemini => Some("GEMINI_BASE_URL"),
            ProviderKind::Unconfigured | ProviderKind::Custom => None,
        };
        let base_url = cli
            .base_url
            .or_else(|| env::var("YOPES_BASE_URL").ok())
            .or_else(|| provider_base_var.and_then(|name| env::var(name).ok()))
            .or_else(|| {
                saved_for_provider
                    .map(|saved| saved.base_url.clone())
                    .filter(|base_url| !base_url.trim().is_empty())
            })
            .unwrap_or_else(|| default_base_url(provider).to_owned());

        let auth = saved_for_provider.map_or(AuthKind::Bearer, |saved| saved.auth);
        let models = saved_for_provider
            .map(|saved| saved.models.clone())
            .unwrap_or_default();
        let env_system_prompt = env::var("YOPES_SYSTEM_PROMPT")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let system_prompt = cli
            .system_prompt
            .filter(|value| !value.trim().is_empty())
            .or(env_system_prompt)
            .or_else(|| {
                saved
                    .as_ref()
                    .and_then(|saved| saved.system_prompt.clone())
                    .filter(|value| !value.trim().is_empty())
            })
            .map(|value| value.trim().to_owned());
        let api_key = resolve_api_key(saved_for_provider, env_key)?;
        let tavily_api_key = env::var("TAVILY_API_KEY")
            .ok()
            .filter(|key| !key.trim().is_empty())
            .or_else(|| {
                saved
                    .as_ref()
                    .and_then(|saved| saved.tavily_api_key.clone())
                    .filter(|key| !key.trim().is_empty())
            });
        if matches!(
            provider,
            ProviderKind::OpenAi | ProviderKind::Anthropic | ProviderKind::Gemini
        ) && api_key.is_none()
            && saved_for_provider.is_none()
        {
            let env_var = match provider {
                ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
                ProviderKind::Gemini => "GEMINI_API_KEY or GOOGLE_API_KEY",
                _ => "OPENAI_API_KEY",
            };
            bail!("no API key for {provider:?}; set {env_var} or run /provider connect");
        }

        Ok(Self {
            provider,
            provider_label,
            model,
            base_url,
            api_key,
            auth,
            models,
            system_prompt,
            custom_providers: custom_providers(saved.as_ref()),
            session_path,
            sessions_dir,
            config_path,
            tavily_api_key,
            auto_compact: saved.as_ref().is_none_or(|saved| saved.auto_compact),
            reasoning_effort: saved
                .as_ref()
                .and_then(|saved| saved.reasoning_effort.clone()),
            theme: saved
                .as_ref()
                .and_then(|saved| saved.theme.clone())
                .unwrap_or_else(|| "dark".to_owned()),
            permission: saved.as_ref().and_then(|saved| saved.permission.clone()),
            diff_sides: saved.as_ref().and_then(|saved| saved.diff_sides),
        })
    }
}

fn default_model(provider: ProviderKind) -> String {
    match provider {
        ProviderKind::Unconfigured | ProviderKind::Custom => "",
        ProviderKind::OpenAi => "gpt-4o-mini",
        ProviderKind::Ollama => "llama3.2",
        ProviderKind::Anthropic => "claude-3-5-sonnet-latest",
        ProviderKind::Gemini => "gemini-2.0-flash",
    }
    .to_owned()
}

fn default_base_url(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Unconfigured | ProviderKind::Custom => "",
        ProviderKind::OpenAi => "https://api.openai.com/v1",
        ProviderKind::Ollama => "http://localhost:11434",
        ProviderKind::Anthropic => "https://api.anthropic.com/v1",
        ProviderKind::Gemini => "https://generativelanguage.googleapis.com/v1beta",
    }
}

/// The global `~/.yopes` directory: config, sessions, skills and `mcp.json`.
pub(crate) fn yopes_home() -> PathBuf {
    directories::BaseDirs::new().map_or_else(
        || PathBuf::from(".yopes"),
        |dirs| dirs.home_dir().join(".yopes"),
    )
}

fn default_config_path() -> PathBuf {
    yopes_home().join("config.json")
}

fn default_sessions_dir() -> PathBuf {
    yopes_home().join("sessions")
}

/// Copy the config the old per-OS app directory held into `~/.yopes`, once.
/// Copies rather than moves, so an older yopes build still finds its config.
fn migrate_legacy_config(path: &Path) {
    if path.exists() {
        return;
    }
    let Some(dirs) = ProjectDirs::from("dev", "Yopes", "Yopes") else {
        return;
    };
    let legacy = dirs.config_dir().join("config.json");
    if legacy.exists() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::copy(&legacy, path);
    }
}

/// Copy session files from the old per-OS data directory, skipping names
/// already present.
fn migrate_legacy_sessions(dir: &Path) {
    let Some(dirs) = ProjectDirs::from("dev", "Yopes", "Yopes") else {
        return;
    };
    let Ok(entries) = fs::read_dir(dirs.data_local_dir().join("sessions")) else {
        return;
    };
    let _ = fs::create_dir_all(dir);
    for entry in entries.flatten() {
        let target = dir.join(entry.file_name());
        if !target.exists() {
            let _ = fs::copy(entry.path(), target);
        }
    }
}

// ponytail: millisecond timestamp names max 1 session/ms; loop increments stamp on collision.
fn new_session_path(dir: &Path) -> PathBuf {
    let mut stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    loop {
        let path = dir.join(format!("session-{stamp}.json"));
        if !path.exists() {
            return path;
        }
        stamp += 1;
    }
}

fn newest_session_path(dir: &Path) -> Option<PathBuf> {
    let list = fs::read_dir(dir).ok()?;
    let mut sessions: Vec<(SystemTime, PathBuf)> = list
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|x| x == "json"))
        .filter_map(|path| {
            fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .ok()
                .map(|modified| (modified, path))
        })
        .collect();
    sessions.sort();
    sessions.pop().map(|(_, newest)| newest)
}

fn migrate_legacy_session(dir: &Path) {
    let Some(dirs) = ProjectDirs::from("dev", "Yopes", "Yopes") else {
        return;
    };
    let legacy = dirs.data_local_dir().join("session.json");
    if legacy.exists()
        && fs::metadata(&legacy)
            .map(|metadata| metadata.len() > 0)
            .unwrap_or(false)
    {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis());
        let _ = fs::create_dir_all(dir);
        let _ = fs::rename(&legacy, dir.join(format!("session-{stamp}.json")));
    }
}

pub fn load_session(path: Option<&Path>) -> Result<crate::session::Session> {
    path.map_or_else(
        || Ok(crate::session::Session::new()),
        |path| {
            crate::session::Session::load(path)
                .with_context(|| format!("loading {}", path.display()))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_path_picks_unused_file_and_newest_finds_latest() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp = env::temp_dir().join(format!("yopes-session-test-{unique}"));
        fs::create_dir_all(&temp).unwrap();

        assert!(newest_session_path(&temp).is_none());

        let first = new_session_path(&temp);
        assert!(!first.exists());
        fs::write(&first, "{}").unwrap();

        let second = new_session_path(&temp);
        assert_ne!(first, second);
        assert!(!second.exists());
        fs::write(&second, "{}").unwrap();

        let newest = newest_session_path(&temp).unwrap();
        assert_eq!(newest, second);

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn custom_provider_with_empty_model_does_not_bail() {
        let temp = env::temp_dir().join(format!(
            "yopes-config-empty-model-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp).unwrap();
        let config_file = temp.join(".yopes-config.json");
        fs::write(
            &config_file,
            r#"{
                "version": 1,
                "provider": "custom",
                "provider_name": "Local",
                "model": "",
                "base_url": "http://localhost:11434/v1"
            }"#,
        )
        .unwrap();

        let saved = load_saved(&config_file).unwrap().unwrap();
        assert_eq!(saved.model, "");
        assert_eq!(saved.provider, ProviderKind::Custom);

        let _ = fs::remove_dir_all(&temp);
    }
}
