//! Fetch model IDs from a user-configured OpenAI-compatible API endpoint.

use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config::AuthKind;

/// Fetch an OpenAI-compatible model list from a custom provider's models endpoint.
///
/// The endpoint is whatever the user stored, so providers whose model list
/// lives somewhere other than `<base>/models` still work.
pub async fn fetch_models(
    url: &str,
    auth: AuthKind,
    key: Option<&str>,
) -> Result<Vec<(String, String)>> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("yopes/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("creating the models client")?;
    let mut request = client.get(url.trim());
    if let Some(key) = key.map(str::trim).filter(|key| !key.is_empty()) {
        request = match auth {
            AuthKind::Bearer | AuthKind::Jwt => request.bearer_auth(key),
            AuthKind::XApiKey => request.header("x-api-key", key),
            AuthKind::None => request,
        };
    }
    let parsed: OpenAiModels = request
        .send()
        .await
        .context("contacting the models endpoint")?
        .error_for_status()
        .context("fetching the model list")?
        .json()
        .await
        .context("reading the model list")?;
    Ok(parsed
        .data
        .into_iter()
        .map(|model| (model.id, model.name.trim().to_owned()))
        .collect())
}

#[derive(Deserialize)]
struct OpenAiModels {
    #[serde(default)]
    data: Vec<OpenAiModel>,
}

#[derive(Deserialize)]
struct OpenAiModel {
    id: String,
    #[serde(default)]
    name: String,
}

#[cfg(test)]
mod tests {
    use super::OpenAiModels;

    #[test]
    fn openai_models_response_parses() {
        let parsed: OpenAiModels = serde_json::from_str(
            r#"{"object":"list","data":[{"id":"model-a"},{"id":"model-b","name":"Model B"}]}"#,
        )
        .unwrap();
        assert_eq!(parsed.data.len(), 2);
        assert_eq!(parsed.data[0].id, "model-a");
        assert_eq!(parsed.data[1].name, "Model B");
    }
}
