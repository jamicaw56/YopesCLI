//! The `/provider connect` wizard's state: the stage machine, its list items, and the
//! helpers every handler reaches for.
use ratatui::layout::Rect;

use crate::config::{self, AuthKind, SavedModel};
pub const CUSTOM_PROVIDER_ID: &str = "custom";
/// Prefix of every reserved action row id: never a real model id.
pub(super) const ACTION_ID_PREFIX: &str = "__action:";
pub(super) const ADD_MODEL_ID: &str = "__action:add-model__";
/// Rows that edit an existing saved endpoint from the model list.
pub(super) const EDIT_ENDPOINT_ID: &str = "__action:edit-endpoint__";
pub(super) const EDIT_NAME_ID: &str = "__action:rename-provider__";
pub(super) const EDIT_TOKEN_ID: &str = "__action:edit-token__";
/// Row that edits the endpoint serving a custom provider's model list.
pub(super) const EDIT_MODELS_URL_ID: &str = "__action:edit-models-url__";
/// Row that refetches the model list from that endpoint.
pub(super) const FETCH_MODELS_ID: &str = "__action:fetch-models__";
/// Row that deletes an existing saved custom provider.
pub(super) const REMOVE_PROVIDER_ID: &str = "__action:remove-provider__";
/// Context window presets the wizard offers, ascending in tokens.
pub(super) const CONTEXT_WINDOW_PRESETS: &[(u64, &str)] = &[
    (32_000, "32K"),
    (64_000, "64K"),
    (128_000, "128K"),
    (256_000, "256K"),
    (512_000, "512K"),
    (786_000, "786K"),
    (960_000, "960K"),
    (1_000_000, "1M"),
];
/// Row id of the Custom context window entry: its number is typed, not picked.
pub(super) const CUSTOM_CONTEXT_WINDOW_ID: &str = "__action:custom-context__";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddProviderField {
    Name = 0,
    CompletionUrl = 1,
    Token = 2,
    Done = 3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectStage {
    Providers,
    Models,
    ApiKey,
    Endpoint,
    /// Optional URL serving the provider's model list; defaults to `<base>/models`.
    ModelsEndpoint,
    CustomToken,
    ModelId,
    ModelName,
    ModelContextWindow,
    CustomModels,
    Password,
    /// Unlock a key that is already stored for the endpoint.
    Unlock,
    AddProvider,
    RenameProvider,
}
impl ConnectStage {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Providers => "provider",
            Self::Models => "model",
            Self::ApiKey => "api key",
            Self::Endpoint => "endpoint",
            Self::ModelsEndpoint => "models endpoint",
            Self::CustomToken => "token",
            Self::ModelId => "model id",
            Self::ModelName => "model name",
            Self::ModelContextWindow => "context window",
            Self::CustomModels => "models",
            Self::Password => "password",
            Self::Unlock => "password",
            Self::AddProvider => "add custom provider",
            Self::RenameProvider => "rename provider",
        }
    }

    /// Where this stage sits in the flow, shown in the dialog's bottom title:
    /// a user always knows which step they are on and what comes next.
    pub const fn hint(self) -> &'static str {
        match self {
            Self::Providers => "pick a provider",
            Self::Models => "pick a model",
            Self::ApiKey => "paste the API key",
            Self::Endpoint => "type the endpoint URL",
            Self::ModelsEndpoint => "type the models URL",
            Self::CustomToken => "paste the token",
            Self::ModelId => "type the model id",
            Self::ModelName => "name the model",
            Self::ModelContextWindow => "pick the context window",
            Self::CustomModels => "pick a saved model",
            Self::Password => "set a passphrase",
            Self::Unlock => "unlock the stored key",
            Self::AddProvider => "3 fields · models come from the endpoint",
            Self::RenameProvider => "rename the provider",
        }
    }
}
#[derive(Clone, Debug)]
pub struct ConnectItem {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub logo: String,
    /// Set when the item is a saved custom endpoint, holding its provider id.
    pub custom_id: Option<String>,
    /// True for the provider and model the session is currently using.
    pub active: bool,
}

/// One-letter badge for a saved custom provider: user names are arbitrary,
/// so the first non-space character is the only reliable identity marker.
fn provider_monogram(name: &str) -> String {
    name.chars()
        .find(|character| !character.is_whitespace())
        .map(|initial| initial.to_uppercase().next().unwrap_or(initial).to_string())
        .unwrap_or_else(|| "?".to_owned())
}

/// The glyph each endpoint gets: one column of a plain terminal, no Nerd Font,
/// no emoji, so a row keeps its columns whatever the terminal is set to.
/// Both the saved URL and the name are matched against these keys, so an
/// endpoint named `groq` that points at a proxy still gets the groq mark.
const PROVIDER_GLYPHS: &[(&[&str], &str)] = &[
    (&["openrouter"], "◇"),
    (&["openai", "azure"], "◆"),
    (&["anthropic", "claude"], "✳"),
    (&["generativelanguage", "google", "gemini"], "✦"),
    (&["deepseek"], "◈"),
    (&["groq"], "❖"),
    (&["mistral"], "▲"),
    (&["x.ai", "xai", "grok"], "✕"),
    (&["together"], "⬢"),
    (&["fireworks"], "✺"),
    (&["cerebras"], "◉"),
    (&["perplexity"], "⌕"),
    (&["cohere"], "◍"),
    (&["qwen", "dashscope", "alibaba"], "❂"),
    (&["moonshot", "kimi"], "☾"),
    (&["ollama", "localhost", "127.0.0.1", "0.0.0.0"], "⌂"),
    (&["github"], "⌥"),
];

/// The badge of a provider row: the glyph of an endpoint the list knows, so
/// two providers of the same kind read apart at a glance, and the name's
/// initial for everything else.
fn provider_logo(name: &str, base_url: &str) -> String {
    let host = endpoint_host(base_url).to_ascii_lowercase();
    let label = name.trim().to_ascii_lowercase();
    for (keys, glyph) in PROVIDER_GLYPHS {
        if keys
            .iter()
            .any(|key| host.contains(key) || label.contains(key))
        {
            return (*glyph).to_owned();
        }
    }
    provider_monogram(name)
}

/// The line under a provider's name: where it points and what it holds.
fn provider_detail(provider: &config::CustomProvider) -> String {
    let models = match provider.models.len() {
        0 => "no models".to_owned(),
        1 => "1 model".to_owned(),
        count => format!("{count} models"),
    };
    match endpoint_host(&provider.base_url) {
        host if host.is_empty() => models,
        host => format!("{host} · {models}"),
    }
}

/// One rendered row of the connect list: an item, a blank spacer, or a
/// rule line naming the section below it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ListRow {
    Item(usize),
    Spacer,
    Rule(&'static str),
}
/// The host of an endpoint URL, for naming where a fetch goes.
///
/// `/provider connect` shows the loading row while a model list is on its way;
/// a custom endpoint fetches from its own host, not from models.dev.
pub(super) fn endpoint_host(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split('/').next().unwrap_or_default().to_owned()
}

pub struct ConnectState {
    pub stage: ConnectStage,
    pub loading: bool,
    pub error: Option<String>,
    pub filter: String,
    pub selected: usize,
    pub(crate) saved_custom: Vec<config::CustomProvider>,
    /// Base URL of the endpoint in use, so the list both draws and resolves
    /// its rows in one order: the connected endpoint on top.
    pub(crate) active_base_url: String,
    pub(crate) model_id: Option<String>,
    pub(crate) api_key: String,
    pub(crate) password: String,
    pub(crate) custom_flow: bool,
    pub(crate) custom_provider_id: Option<String>,
    pub(crate) custom_name: String,
    pub(crate) custom_endpoint: String,
    pub(crate) custom_models_url: String,
    pub(crate) custom_auth: AuthKind,
    pub(crate) custom_auth_explicit: bool,
    pub(crate) custom_token: String,
    /// The token was edited during this visit, so it needs saving again.
    pub(crate) custom_token_dirty: bool,
    pub(crate) custom_models: Vec<SavedModel>,
    pub(crate) custom_model_id: String,
    pub(crate) custom_model_name: String,
    pub(crate) custom_model_context_window: String,
    /// Highlighted row of the context window preset picker.
    pub(crate) context_row: usize,
    pub(crate) add_provider_field: AddProviderField,
    pub(crate) popup_inner: std::cell::Cell<Rect>,
    pub(crate) list_area: std::cell::Cell<Rect>,
    /// The key already stored for the endpoint being connected, if any.
    pub(crate) stored_secret: Option<config::StoredSecret>,
    /// The endpoint whose stored key is being unlocked.
    pub(crate) unlock_endpoint: String,
    /// A key that was unlocked during this visit.
    pub(crate) unlocked_key: Option<String>,
    /// The key editor was used on a saved endpoint: the next password prompt
    /// writes just that key, leaving the model and the rest of the connection
    /// alone.
    pub(crate) token_only_save: bool,
}
impl ConnectState {
    pub(crate) fn loading() -> Self {
        Self {
            stage: ConnectStage::Providers,
            loading: false,
            error: None,
            filter: String::new(),
            selected: 0,
            saved_custom: Vec::new(),
            active_base_url: String::new(),
            model_id: None,
            api_key: String::new(),
            password: String::new(),
            custom_flow: false,
            custom_provider_id: None,
            custom_name: String::new(),
            custom_endpoint: String::new(),
            custom_models_url: String::new(),
            custom_auth: AuthKind::Bearer,
            custom_auth_explicit: false,
            custom_token: String::new(),
            custom_token_dirty: false,
            custom_models: Vec::new(),
            custom_model_id: String::new(),
            custom_model_name: String::new(),
            custom_model_context_window: String::new(),
            context_row: 0,
            add_provider_field: AddProviderField::Name,
            popup_inner: std::cell::Cell::new(Rect::default()),
            list_area: std::cell::Cell::new(Rect::default()),
            stored_secret: None,
            unlock_endpoint: String::new(),
            unlocked_key: None,
            token_only_save: false,
        }
    }
    pub fn items(&self) -> Vec<ConnectItem> {
        self.items_using(&self.active_base_url, "")
    }
    /// Build the list, marking whatever the session is already using.
    ///
    /// The active endpoint is the backend's base URL, which uniquely identifies
    /// a provider (two saved custom endpoints can share a name but not a URL).
    pub fn items_using(&self, active_base_url: &str, active_model: &str) -> Vec<ConnectItem> {
        let filter = self.filter.to_ascii_lowercase();
        let active_base_url = active_base_url.trim_end_matches('/');
        let is_active = |endpoint: &str| {
            !active_base_url.is_empty() && endpoint.trim_end_matches('/') == active_base_url
        };
        match self.stage {
            ConnectStage::Providers => {
                // Saved endpoints, in a readable order: the one this session
                // is using first, the rest by name. Insertion order told the
                // user nothing once there were more than a handful.
                let mut saved: Vec<&config::CustomProvider> = self
                    .saved_custom
                    .iter()
                    .filter(|saved| {
                        filter.is_empty()
                            || saved.name.to_ascii_lowercase().contains(filter.as_str())
                    })
                    .collect();
                saved.sort_by(|a, b| {
                    is_active(&b.base_url)
                        .cmp(&is_active(&a.base_url))
                        .then_with(|| {
                            a.name
                                .trim()
                                .to_ascii_lowercase()
                                .cmp(&b.name.trim().to_ascii_lowercase())
                        })
                        .then_with(|| a.base_url.cmp(&b.base_url))
                });
                let mut items: Vec<ConnectItem> = saved
                    .into_iter()
                    .map(|saved| ConnectItem {
                        id: saved.id.clone(),
                        title: saved.name.clone(),
                        detail: provider_detail(saved),
                        logo: provider_logo(&saved.name, &saved.base_url),
                        custom_id: Some(saved.id.clone()),
                        active: is_active(&saved.base_url),
                    })
                    .collect();
                // The action that adds another endpoint sits below the ones
                // already saved, the way the endpoint actions sit below the
                // model rows in the custom stage.
                if filter.is_empty() || "custom".contains(filter.as_str()) {
                    items.push(ConnectItem {
                        id: CUSTOM_PROVIDER_ID.to_owned(),
                        title: "Add custom API".to_owned(),
                        detail: "connect your own API endpoint".to_owned(),
                        logo: "⚙".to_owned(),
                        custom_id: None,
                        active: false,
                    });
                }
                items
            }
            ConnectStage::Models => Vec::new(),
            ConnectStage::CustomModels => {
                let mut items: Vec<ConnectItem> = self
                    .custom_models
                    .iter()
                    .map(|model| {
                        let detail = model.context_window.map_or_else(
                            || {
                                if model.name.trim().is_empty() {
                                    "model id".to_owned()
                                } else {
                                    model.id.clone()
                                }
                            },
                            |context| {
                                if context >= 1000 {
                                    format!("{} · {}k context", model.id, context / 1000)
                                } else {
                                    format!("{} · {context} context", model.id)
                                }
                            },
                        );
                        ConnectItem {
                            id: model.id.clone(),
                            title: if model.name.trim().is_empty() {
                                model.id.clone()
                            } else {
                                model.name.clone()
                            },
                            detail,
                            logo: "▸".to_owned(),
                            custom_id: None,
                            active: !active_model.is_empty() && model.id == active_model,
                        }
                    })
                    .collect();
                if filter.is_empty() || "add".contains(filter.as_str()) {
                    items.push(ConnectItem {
                        id: ADD_MODEL_ID.to_owned(),
                        title: "Add new model".to_owned(),
                        detail: "enter a model id".to_owned(),
                        logo: "+".to_owned(),
                        custom_id: None,
                        active: false,
                    });
                }
                // Editing only makes sense for an endpoint that is already saved.
                if self.custom_provider_id.is_some() {
                    // Match the same way every other row filters: against the title.
                    let matches = |title: &str| {
                        filter.is_empty() || title.to_ascii_lowercase().contains(filter.as_str())
                    };
                    if matches("Rename provider") {
                        items.push(ConnectItem {
                            id: EDIT_NAME_ID.to_owned(),
                            title: "Rename provider".to_owned(),
                            detail: self.custom_name.clone(),
                            logo: "✎".to_owned(),
                            custom_id: None,
                            active: false,
                        });
                    }
                    if matches("Change endpoint") {
                        items.push(ConnectItem {
                            id: EDIT_ENDPOINT_ID.to_owned(),
                            title: "Change endpoint".to_owned(),
                            detail: self.custom_endpoint.clone(),
                            logo: "✎".to_owned(),
                            custom_id: None,
                            active: false,
                        });
                    }
                    if matches("Change API key") {
                        items.push(ConnectItem {
                            id: EDIT_TOKEN_ID.to_owned(),
                            title: "Change API key".to_owned(),
                            detail: if self.custom_token.is_empty() {
                                "not set".to_owned()
                            } else {
                                format!("set · {} chars", self.custom_token.chars().count())
                            },
                            logo: "✎".to_owned(),
                            custom_id: None,
                            active: false,
                        });
                    }
                }
                // The model list can be read from any OpenAI-compatible
                // endpoint; the field defaults to `<base>/models`.
                if filter.is_empty() || "models endpoint".contains(filter.as_str()) {
                    items.push(ConnectItem {
                        id: EDIT_MODELS_URL_ID.to_owned(),
                        title: "Models endpoint".to_owned(),
                        detail: if self.custom_models_url.trim().is_empty() {
                            "default <endpoint>/models".to_owned()
                        } else {
                            self.custom_models_url.clone()
                        },
                        logo: "✎".to_owned(),
                        custom_id: None,
                        active: false,
                    });
                }
                if filter.is_empty() || "fetch models".contains(filter.as_str()) {
                    items.push(ConnectItem {
                        id: FETCH_MODELS_ID.to_owned(),
                        title: "Fetch models".to_owned(),
                        detail: "read the list from the models endpoint".to_owned(),
                        logo: "↻".to_owned(),
                        custom_id: None,
                        active: false,
                    });
                }
                if self.custom_provider_id.is_some()
                    && (filter.is_empty() || "remove provider".contains(filter.as_str()))
                {
                    items.push(ConnectItem {
                        id: REMOVE_PROVIDER_ID.to_owned(),
                        title: "Remove provider".to_owned(),
                        detail: "delete this endpoint and its models".to_owned(),
                        logo: "✕".to_owned(),
                        custom_id: None,
                        active: false,
                    });
                }
                items
            }
            ConnectStage::ModelContextWindow => {
                let current = self.custom_model_context_window.trim();
                let matches_preset = |tokens: u64| tokens.to_string() == current;
                let mut items: Vec<ConnectItem> = CONTEXT_WINDOW_PRESETS
                    .iter()
                    .map(|(tokens, label)| ConnectItem {
                        id: tokens.to_string(),
                        title: (*label).to_owned(),
                        detail: format!("{tokens} tokens"),
                        logo: "▸".to_owned(),
                        custom_id: None,
                        active: matches_preset(*tokens),
                    })
                    .collect();
                items.push(ConnectItem {
                    id: CUSTOM_CONTEXT_WINDOW_ID.to_owned(),
                    title: "Custom".to_owned(),
                    detail: if current.is_empty() {
                        "type a number".to_owned()
                    } else {
                        format!("{current} tokens")
                    },
                    logo: "▸".to_owned(),
                    custom_id: None,
                    active: !current.is_empty()
                        && !CONTEXT_WINDOW_PRESETS
                            .iter()
                            .any(|(tokens, _)| matches_preset(*tokens)),
                });
                items
            }
            ConnectStage::ApiKey
            | ConnectStage::Endpoint
            | ConnectStage::ModelsEndpoint
            | ConnectStage::CustomToken
            | ConnectStage::ModelId
            | ConnectStage::ModelName
            | ConnectStage::Password
            | ConnectStage::Unlock
            | ConnectStage::AddProvider
            | ConnectStage::RenameProvider => Vec::new(),
        }
    }
    /// Drop the model row the user is on in the saved endpoint's model list.
    ///
    /// Returns the model id so the caller can persist the deletion; special
    /// rows (add model, change endpoint, change key) are never removable.
    pub(super) fn remove_selected_model(&mut self) -> Option<String> {
        if self.stage != ConnectStage::CustomModels {
            return None;
        }
        let items = self.items();
        let item = items.get(self.selected.min(items.len().saturating_sub(1)))?;
        if matches!(
            item.id.as_str(),
            ADD_MODEL_ID
                | EDIT_ENDPOINT_ID
                | EDIT_TOKEN_ID
                | EDIT_MODELS_URL_ID
                | FETCH_MODELS_ID
                | REMOVE_PROVIDER_ID
        ) {
            return None;
        }
        let id = item.id.clone();
        self.custom_models.retain(|model| model.id != id);
        self.selected = self.selected.min(self.items().len().saturating_sub(1));
        self.error = None;
        Some(id)
    }
    pub fn input(&self) -> String {
        if self.stage == ConnectStage::CustomModels {
            let items = self.items();
            return items
                .get(self.selected.min(items.len().saturating_sub(1)))
                .map_or_else(|| "none".to_owned(), |item| item.title.clone());
        }
        let value = match self.stage {
            ConnectStage::ApiKey => &self.api_key,
            ConnectStage::Password | ConnectStage::Unlock => &self.password,
            ConnectStage::RenameProvider => &self.custom_name,
            ConnectStage::Endpoint => &self.custom_endpoint,
            ConnectStage::ModelsEndpoint => &self.custom_models_url,
            ConnectStage::CustomToken => &self.custom_token,
            ConnectStage::ModelId => &self.custom_model_id,
            ConnectStage::ModelName => &self.custom_model_name,
            ConnectStage::ModelContextWindow => &self.custom_model_context_window,
            ConnectStage::Providers | ConnectStage::Models => &self.filter,
            ConnectStage::CustomModels | ConnectStage::AddProvider => {
                unreachable!()
            }
        };
        if matches!(
            self.stage,
            ConnectStage::ApiKey
                | ConnectStage::Password
                | ConnectStage::CustomToken
                | ConnectStage::Unlock
        ) && !value.is_empty()
        {
            "•".repeat(value.chars().count().min(64))
        } else {
            value.clone()
        }
    }
    pub const fn input_label(&self) -> &'static str {
        match self.stage {
            ConnectStage::Providers | ConnectStage::Models => "search",
            ConnectStage::ApiKey => "api key",
            ConnectStage::RenameProvider => "new provider name",
            ConnectStage::Endpoint => "completion endpoint",
            ConnectStage::ModelsEndpoint => "models endpoint (blank: <endpoint>/models)",
            ConnectStage::CustomToken => "token",
            ConnectStage::ModelId => "model id",
            ConnectStage::ModelName => "model name",
            ConnectStage::ModelContextWindow => "context window",
            ConnectStage::CustomModels => "model",
            ConnectStage::Password => "encryption password (optional)",
            ConnectStage::Unlock => "key password",
            ConnectStage::AddProvider => "add provider",
        }
    }
    pub const fn uses_cursor(&self) -> bool {
        if matches!(self.stage, ConnectStage::ModelContextWindow) {
            return self.context_row == CONTEXT_WINDOW_PRESETS.len();
        }
        !matches!(
            self.stage,
            ConnectStage::CustomModels | ConnectStage::AddProvider
        )
    }

    pub(crate) fn field_at_mouse(&self, col: u16, row: u16) -> Option<AddProviderField> {
        let inner = self.popup_inner.get();
        if inner.width == 0 || inner.height == 0 {
            return None;
        }
        if col < inner.x || col >= inner.x.saturating_add(inner.width) {
            return None;
        }
        if row < inner.y || row >= inner.y.saturating_add(inner.height) {
            return None;
        }
        let rel_y = row.saturating_sub(inner.y);
        match rel_y {
            0 | 1 => Some(AddProviderField::Name),
            2 | 3 => Some(AddProviderField::CompletionUrl),
            4 | 5 => Some(AddProviderField::Token),
            7 | 8 => Some(AddProviderField::Done),
            _ => None,
        }
    }

    pub(crate) fn item_at_mouse(&self, col: u16, row: u16) -> Option<usize> {
        let area = self.list_area.get();
        if area.width == 0 || area.height == 0 {
            return None;
        }
        if col < area.x || col >= area.x.saturating_add(area.width) {
            return None;
        }
        if row < area.y || row >= area.y.saturating_add(area.height) {
            return None;
        }
        let items = self.items();
        if items.is_empty() {
            return None;
        }
        let height = usize::from(area.height);
        let rows = self.list_rows(&items);
        let selected = self.selected.min(items.len() - 1);
        let selected_row = list_row_of(&rows, selected);
        let start_row = selected_row.saturating_sub(height.saturating_sub(1));
        let start = start_row.min(rows.len().saturating_sub(height));
        let offset = usize::from(row.saturating_sub(area.y));
        rows.get(start + offset).and_then(|r| match r {
            ListRow::Item(index) => Some(*index),
            ListRow::Spacer | ListRow::Rule(_) => None,
        })
    }

    /// The rows of the connect list for the given items: item indices with
    /// spacer and rule rows where the stage mixes sections. The renderer and
    /// the mouse mapping share this so both show and skip the same rows.
    pub(crate) fn list_rows(&self, items: &[ConnectItem]) -> Vec<ListRow> {
        let mut rows = Vec::with_capacity(items.len() + 3);
        match self.stage {
            ConnectStage::Providers => {
                for index in 0..items.len() {
                    // The add row is an action, not a saved endpoint: a rule
                    // keeps it from reading as provider number n.
                    if index > 0
                        && items[index].id == CUSTOM_PROVIDER_ID
                        && items[index].custom_id.is_none()
                    {
                        rows.push(ListRow::Rule(" add "));
                    }
                    rows.push(ListRow::Item(index));
                }
            }
            ConnectStage::CustomModels => {
                // The endpoint actions sit below the model rows, set apart
                // so neither section hides the other.
                let first_action = items
                    .iter()
                    .position(|item| item.id.starts_with(ACTION_ID_PREFIX))
                    .filter(|first| *first > 0);
                for index in 0..items.len() {
                    if first_action == Some(index) {
                        rows.push(ListRow::Spacer);
                        rows.push(ListRow::Spacer);
                        rows.push(ListRow::Rule(" manage "));
                    }
                    rows.push(ListRow::Item(index));
                }
            }
            _ => {
                for index in 0..items.len() {
                    rows.push(ListRow::Item(index));
                }
            }
        }
        rows
    }

    pub(crate) fn active_add_provider_field_mut(&mut self) -> Option<&mut String> {
        match self.add_provider_field {
            AddProviderField::Name => Some(&mut self.custom_name),
            AddProviderField::CompletionUrl => Some(&mut self.custom_endpoint),
            AddProviderField::Token => Some(&mut self.custom_token),
            AddProviderField::Done => None,
        }
    }
    pub fn has_custom_models(&self) -> bool {
        !self.custom_models.is_empty()
    }
    /// What the loading row of /connect names as its source: a custom endpoint.
    pub fn loading_detail(&self) -> String {
        if self.custom_flow || self.stage == ConnectStage::CustomModels {
            let host = endpoint_host(&self.custom_endpoint);
            if host.is_empty() {
                "  fetching models from the endpoint".to_owned()
            } else {
                format!("  fetching models from {host}")
            }
        } else {
            "  loading custom APIs".to_owned()
        }
    }
    /// The endpoint of the provider being connected.
    /// The stored key, ready to use without asking for anything.
    ///
    /// A secret saved without a password opens right away; an encrypted one
    /// needs its password first, so it is not returned here.
    pub(super) fn selected_item(&self) -> Option<ConnectItem> {
        let items = self.items();
        if items.is_empty() {
            return None;
        }
        items.get(self.list_row().min(items.len() - 1)).cloned()
    }
    /// The endpoint of the provider row under the selection, when that row is
    /// a real provider rather than an action.
    pub(crate) fn selected_endpoint(&self) -> Option<String> {
        if self.stage != ConnectStage::Providers {
            return None;
        }
        let item = self.selected_item()?;
        if item.id == CUSTOM_PROVIDER_ID {
            return None;
        }
        if item.custom_id.is_some() {
            return self
                .saved_custom
                .iter()
                .find(|provider| provider.id == item.id)
                .map(|provider| provider.base_url.clone());
        }
        None
    }
    pub(super) fn move_selection(&mut self, forward: bool) {
        let len = self.items().len();
        if len == 0 {
            return;
        }
        if matches!(self.stage, ConnectStage::ModelContextWindow) {
            let current = self.context_row.min(len - 1);
            self.context_row = if forward {
                (current + 1) % len
            } else {
                (current + len - 1) % len
            };
            // The input mirrors the highlighted preset; Custom keeps the field.
            if self.context_row != CONTEXT_WINDOW_PRESETS.len() {
                let (tokens, _) = CONTEXT_WINDOW_PRESETS[self.context_row];
                self.custom_model_context_window = tokens.to_string();
            }
            return;
        }
        let current = self.selected.min(len - 1);
        self.selected = if forward {
            (current + 1) % len
        } else {
            (current + len - 1) % len
        };
    }
    pub(super) fn move_selection_by(&mut self, delta: isize) {
        let len = self.items().len();
        if len == 0 {
            return;
        }
        let current = self.selected.min(len - 1) as isize;
        self.selected = (current + delta).clamp(0, len as isize - 1) as usize;
    }
    /// Land the context preset picker on the row matching the current value,
    /// or on Custom when it is blank or not one of the presets.
    pub(crate) fn sync_context_selection(&mut self) {
        let current = self.custom_model_context_window.trim();
        self.context_row = CONTEXT_WINDOW_PRESETS
            .iter()
            .position(|(tokens, _)| tokens.to_string() == current)
            .unwrap_or(CONTEXT_WINDOW_PRESETS.len());
    }
    /// Jump the context preset picker to Custom so typed input lands there.
    pub(crate) fn select_custom_context(&mut self) {
        self.context_row = CONTEXT_WINDOW_PRESETS.len();
    }
    /// The row the connect list highlights: the context preset picker keeps
    /// its own row so the model list cursor survives leaving the stage.
    pub fn list_row(&self) -> usize {
        if matches!(self.stage, ConnectStage::ModelContextWindow) {
            self.context_row
        } else {
            self.selected
        }
    }
}

/// The screen row of one item index, counting spacer and rule rows.
pub(crate) fn list_row_of(rows: &[ListRow], item: usize) -> usize {
    rows.iter()
        .position(|row| matches!(row, ListRow::Item(index) if *index == item))
        .unwrap_or(0)
}

pub(super) fn custom_field_mut(
    connect: &mut ConnectState,
    stage: ConnectStage,
) -> Option<&mut String> {
    match stage {
        ConnectStage::Endpoint => Some(&mut connect.custom_endpoint),
        ConnectStage::ModelsEndpoint => Some(&mut connect.custom_models_url),
        ConnectStage::CustomToken => Some(&mut connect.custom_token),
        ConnectStage::ModelId => Some(&mut connect.custom_model_id),
        ConnectStage::ModelName => Some(&mut connect.custom_model_name),
        ConnectStage::ModelContextWindow => Some(&mut connect.custom_model_context_window),
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    /// A saved endpoint for the provider tests below.
    fn saved_provider(id: &str, name: &str, base_url: &str) -> crate::config::CustomProvider {
        crate::config::CustomProvider {
            id: id.to_owned(),
            name: name.to_owned(),
            base_url: base_url.to_owned(),
            auth: crate::config::AuthKind::Bearer,
            models: Vec::new(),
            models_url: None,
            token: None,
            encrypted: false,
            secret: crate::config::StoredSecret::default(),
        }
    }

    #[test]
    fn saved_providers_show_name_monograms() {
        use super::{ConnectStage, ConnectState};
        let mut connect = ConnectState::loading();
        connect.stage = ConnectStage::Providers;
        connect.saved_custom = vec![
            saved_provider("one", "  Biggie Solutions", "https://example.test/v1"),
            saved_provider("two", "", "https://example.test/other"),
        ];
        let items = connect.items();
        // Empty names sort first, and the action row is last.
        assert_eq!(items[0].logo, "?");
        assert_eq!(items[1].logo, "B");
        assert_eq!(items[2].title, "Add custom API");
    }

    #[test]
    fn providers_are_sorted_by_name_with_the_active_one_first() {
        use super::{ConnectStage, ConnectState};
        let mut connect = ConnectState::loading();
        connect.stage = ConnectStage::Providers;
        connect.saved_custom = vec![
            saved_provider("zed", "Zed", "https://zed.test/v1"),
            saved_provider("alpha", "alpha", "https://alpha.test/v1"),
            saved_provider("mid", "Middle", "https://mid.test/v1"),
        ];
        let titles = |connect: &ConnectState| -> Vec<String> {
            connect.items().into_iter().map(|item| item.title).collect()
        };
        assert_eq!(
            titles(&connect),
            ["alpha", "Middle", "Zed", "Add custom API"]
        );
        // The endpoint in use is pinned above the alphabetical order.
        let pinned = connect.items_using("https://zed.test/v1", "model");
        assert_eq!(
            pinned
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["Zed", "alpha", "Middle", "Add custom API"]
        );
        assert!(pinned[0].active);
        assert!(!pinned[1].active);
    }

    #[test]
    fn known_hosts_get_a_glyph_and_unknown_ones_the_initial() {
        use super::{ConnectStage, ConnectState};
        let mut connect = ConnectState::loading();
        connect.stage = ConnectStage::Providers;
        connect.saved_custom = vec![
            saved_provider("a", "Home GPU", "https://api.groq.com/openai/v1"),
            saved_provider("b", "Router", "https://openrouter.ai/api/v1"),
            saved_provider("c", "Mine", "https://mine.test/v1"),
        ];
        let logos: Vec<String> = connect.items().into_iter().map(|item| item.logo).collect();
        // Sorted by name: Home GPU (groq host), Mine (unknown), Router.
        assert_eq!(
            logos,
            [
                "❖".to_owned(),
                "M".to_owned(),
                "◇".to_owned(),
                "⚙".to_owned()
            ]
        );
    }

    #[test]
    fn provider_rows_name_their_host_and_model_count() {
        use super::{ConnectStage, ConnectState};
        use crate::config::SavedModel;
        let mut connect = ConnectState::loading();
        connect.stage = ConnectStage::Providers;
        let mut empty = saved_provider("a", "Empty", "https://empty.test/v1");
        empty.models = Vec::new();
        let mut one = saved_provider("b", "One", "https://one.test/v1");
        one.models = vec![SavedModel {
            id: "m".to_owned(),
            ..SavedModel::default()
        }];
        connect.saved_custom = vec![empty, one];
        let details: Vec<String> = connect
            .items()
            .into_iter()
            .map(|item| item.detail)
            .take(2)
            .collect();
        assert_eq!(details, ["empty.test · no models", "one.test · 1 model"]);
    }

    #[test]
    fn the_add_row_sits_under_a_rule_of_its_own() {
        use super::{ConnectStage, ConnectState, ListRow};
        let mut connect = ConnectState::loading();
        connect.stage = ConnectStage::Providers;
        connect.saved_custom = vec![saved_provider("a", "Alpha", "https://alpha.test/v1")];
        let items = connect.items();
        let rows = connect.list_rows(&items);
        assert!(matches!(rows[0], ListRow::Item(0)));
        assert!(matches!(rows[1], ListRow::Rule(" add ")));
        assert!(matches!(rows[2], ListRow::Item(1)));
        // With nothing saved there is no section to divide.
        let empty = ConnectState::loading();
        let rows = empty.list_rows(&empty.items());
        assert_eq!(rows, vec![ListRow::Item(0)]);
    }

    #[test]
    fn saved_provider_row_is_identified_by_custom_id() {
        use super::{CUSTOM_PROVIDER_ID, ConnectStage, ConnectState};
        use crate::config::{AuthKind, CustomProvider};
        // A legacy config with no custom_providers list is folded in with the
        // slug of its name, which can equal the reserved new-provider id.
        let mut connect = ConnectState::loading();
        connect.stage = ConnectStage::Providers;
        connect.saved_custom.push(CustomProvider {
            id: CUSTOM_PROVIDER_ID.to_owned(),
            name: "Custom".to_owned(),
            base_url: "https://example.test/v1".to_owned(),
            auth: AuthKind::Bearer,
            models: Vec::new(),
            models_url: None,
            token: None,
            encrypted: false,
            secret: crate::config::StoredSecret::default(),
        });
        let items = connect.items_using("https://example.test/v1", "model");
        assert_eq!(
            items[0].custom_id.as_deref(),
            Some(CUSTOM_PROVIDER_ID),
            "the saved endpoint keeps its id even when it reads like the action"
        );
        assert!(items[0].active);
        assert_eq!(items[1].custom_id, None, "the last row adds a provider");
    }
    #[test]
    fn custom_filter_hides_other_sections() {
        use super::{ConnectStage, ConnectState};
        let mut connect = ConnectState::loading();
        connect.stage = ConnectStage::Providers;
        connect.filter = "custom".to_owned();
        let items = connect.items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "custom");
    }
    #[test]
    fn delete_removes_only_a_model_row() {
        use super::{ConnectStage, ConnectState, FETCH_MODELS_ID, REMOVE_PROVIDER_ID};
        use crate::config::SavedModel;
        let mut connect = ConnectState::loading();
        connect.stage = ConnectStage::CustomModels;
        connect.custom_models = vec![
            SavedModel {
                id: "one".to_owned(),
                name: "One".to_owned(),
                ..SavedModel::default()
            },
            SavedModel {
                id: "two".to_owned(),
                name: "Two".to_owned(),
                ..SavedModel::default()
            },
        ];
        assert_eq!(connect.remove_selected_model().as_deref(), Some("one"));
        assert_eq!(connect.custom_models.len(), 1);
        // The action rows (add model, change endpoint) are never removable.
        let last = connect.items().len().saturating_sub(1);
        connect.selected = last;
        assert!(connect.remove_selected_model().is_none());
        assert_eq!(connect.custom_models.len(), 1);
        assert_eq!(connect.items()[last].id, FETCH_MODELS_ID);
        connect.custom_provider_id = Some("endpoint".to_owned());
        let last = connect.items().len().saturating_sub(1);
        connect.selected = last;
        assert!(connect.remove_selected_model().is_none());
        assert_eq!(connect.items()[last].id, REMOVE_PROVIDER_ID);
        assert_eq!(connect.custom_models.len(), 1);
        assert_eq!(connect.custom_models[0].id, "two");
    }
    #[test]
    fn custom_model_items_display_context_window() {
        use super::{ConnectStage, ConnectState};
        use crate::config::SavedModel;
        let mut connect = ConnectState::loading();
        connect.stage = ConnectStage::CustomModels;
        connect.custom_models = vec![
            SavedModel {
                id: "one".to_owned(),
                name: "One".to_owned(),
                context_window: Some(128_000),
                ..SavedModel::default()
            },
            SavedModel {
                id: "two".to_owned(),
                name: "Two".to_owned(),
                context_window: Some(500),
                ..SavedModel::default()
            },
        ];
        let items = connect.items();
        assert_eq!(items[0].detail, "one · 128k context");
        assert_eq!(items[1].detail, "two · 500 context");
    }

    #[test]
    fn add_provider_field_selection_and_mouse_detection() {
        use super::{AddProviderField, ConnectState};
        use ratatui::layout::Rect;

        let mut connect = ConnectState::loading();
        connect.popup_inner.set(Rect::new(10, 10, 80, 20));

        assert_eq!(connect.field_at_mouse(15, 10), Some(AddProviderField::Name));
        assert_eq!(
            connect.field_at_mouse(15, 12),
            Some(AddProviderField::CompletionUrl)
        );
        assert_eq!(
            connect.field_at_mouse(20, 14),
            Some(AddProviderField::Token)
        );
        assert_eq!(connect.field_at_mouse(15, 17), Some(AddProviderField::Done));

        connect.add_provider_field = AddProviderField::Name;
        connect
            .active_add_provider_field_mut()
            .unwrap()
            .push_str("LocalLLM");
        assert_eq!(connect.custom_name, "LocalLLM");

        connect.add_provider_field = AddProviderField::Token;
        connect
            .active_add_provider_field_mut()
            .unwrap()
            .push_str("sk-test");
        assert_eq!(connect.custom_token, "sk-test");
    }

    #[test]
    fn custom_model_id_does_not_collide_with_action_ids() {
        use super::{ConnectStage, ConnectState, REMOVE_PROVIDER_ID};
        use crate::config::SavedModel;

        let mut connect = ConnectState::loading();
        connect.stage = ConnectStage::CustomModels;
        connect.custom_models = vec![SavedModel {
            id: "remove-provider".to_owned(),
            name: "ModelNamedRemoveProvider".to_owned(),
            ..SavedModel::default()
        }];
        let items = connect.items();
        let model_item = items.iter().find(|i| i.id == "remove-provider").unwrap();
        assert_ne!(model_item.id, REMOVE_PROVIDER_ID);
        assert_eq!(model_item.title, "ModelNamedRemoveProvider");
    }

    #[test]
    fn custom_actions_sit_apart_from_the_model_rows() {
        use super::{ConnectStage, ConnectState, ListRow};
        use crate::config::SavedModel;
        use ratatui::layout::Rect;

        let mut connect = ConnectState::loading();
        connect.stage = ConnectStage::CustomModels;
        connect.custom_models = vec![
            SavedModel {
                id: "one".to_owned(),
                name: "One".to_owned(),
                ..SavedModel::default()
            },
            SavedModel {
                id: "two".to_owned(),
                name: "Two".to_owned(),
                ..SavedModel::default()
            },
        ];
        let items = connect.items();
        let rows = connect.list_rows(&items);
        assert!(matches!(rows[0], ListRow::Item(0)));
        assert!(matches!(rows[1], ListRow::Item(1)));
        let rule = rows
            .iter()
            .position(|row| matches!(row, ListRow::Rule(_)))
            .expect("the action section has a rule");
        assert!(matches!(rows[rule - 1], ListRow::Spacer));
        assert!(matches!(rows[rule - 2], ListRow::Spacer));
        assert!(matches!(rows[rule + 1], ListRow::Item(2)));
        // The mouse maps across the gap and skips the spacer rows.
        connect.list_area.set(Rect::new(0, 0, 40, 20));
        assert_eq!(connect.item_at_mouse(1, 0), Some(0));
        assert_eq!(connect.item_at_mouse(1, 1), Some(1));
        assert_eq!(connect.item_at_mouse(1, rule as u16), None);
        assert_eq!(connect.item_at_mouse(1, rule as u16 + 1), Some(2));
    }

    #[test]
    fn item_at_mouse_resolves_row_in_connect_list() {
        use super::ConnectState;
        use ratatui::layout::Rect;

        let connect = ConnectState::loading();
        connect.list_area.set(Rect::new(5, 5, 50, 10));

        // Click within list bounds on first item
        assert_eq!(connect.item_at_mouse(10, 5), Some(0));

        // Click outside bounds
        assert_eq!(connect.item_at_mouse(2, 5), None);
        assert_eq!(connect.item_at_mouse(60, 5), None);
        assert_eq!(connect.item_at_mouse(10, 2), None);
        assert_eq!(connect.item_at_mouse(10, 25), None);
    }

    #[test]
    fn context_window_stage_lists_presets_then_custom() {
        use super::{CONTEXT_WINDOW_PRESETS, ConnectStage, ConnectState};

        let mut connect = ConnectState::loading();
        connect.stage = ConnectStage::ModelContextWindow;
        connect.custom_model_context_window = "128000".to_owned();
        connect.sync_context_selection();

        let items = connect.items();
        assert_eq!(items.len(), CONTEXT_WINDOW_PRESETS.len() + 1);
        assert_eq!(items[0].title, "32K");
        assert_eq!(items[0].id, "32000");
        assert_eq!(items[2].id, "128000");
        assert!(items[2].active, "matching preset is marked active");
        let custom = items.last().unwrap();
        assert_eq!(custom.title, "Custom");
        assert!(!custom.active);
        assert_eq!(connect.list_row(), 2);
    }

    #[test]
    fn context_preset_navigation_wraps_and_previews_values() {
        use super::{CONTEXT_WINDOW_PRESETS, ConnectStage, ConnectState};

        let mut connect = ConnectState::loading();
        connect.stage = ConnectStage::ModelContextWindow;
        connect.custom_model_context_window = "400000".to_owned();
        connect.sync_context_selection();
        assert_eq!(connect.list_row(), CONTEXT_WINDOW_PRESETS.len());

        connect.move_selection(false);
        assert_eq!(connect.list_row(), CONTEXT_WINDOW_PRESETS.len() - 1);
        assert_eq!(connect.custom_model_context_window, "1000000");

        connect.move_selection(true);
        assert_eq!(connect.list_row(), CONTEXT_WINDOW_PRESETS.len());
    }

    #[test]
    fn blank_context_window_lands_on_custom_with_hint() {
        use super::{CONTEXT_WINDOW_PRESETS, ConnectStage, ConnectState};

        let mut connect = ConnectState::loading();
        connect.stage = ConnectStage::ModelContextWindow;
        connect.sync_context_selection();
        assert_eq!(connect.list_row(), CONTEXT_WINDOW_PRESETS.len());

        let items = connect.items();
        let custom = items.last().unwrap();
        assert_eq!(custom.detail, "type a number");
        assert!(!custom.active);
    }

    #[test]
    fn context_presets_cover_the_requested_sizes_in_order() {
        use super::CONTEXT_WINDOW_PRESETS;

        let tokens: Vec<u64> = CONTEXT_WINDOW_PRESETS.iter().map(|(t, _)| *t).collect();
        assert_eq!(
            tokens,
            vec![
                32_000, 64_000, 128_000, 256_000, 512_000, 786_000, 960_000, 1_000_000
            ]
        );
        assert!(tokens.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
