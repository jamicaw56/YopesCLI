//! Built-in model context window table and fuzzy resolution rules.

// ponytail: curated static table + family prefix matching; add dynamic fetch cache fallback for newly released models.

/// Clean a model identifier for lookup: strip provider namespaces (`provider/model`),
/// strip reasoning/effort/release suffixes (`:medium`, `:thinking`, `:free`),
/// and strip dated snapshots (`-20240229`).
pub fn normalize_model_id(raw: &str) -> String {
    let mut s = raw.trim();
    if let Some(pos) = s.rfind('/') {
        s = &s[pos + 1..];
    }
    if let Some(pos) = s.find(':') {
        s = &s[..pos];
    }
    let lower = s.to_ascii_lowercase();

    // Strip common date/snapshot suffixes: e.g. -20241022, -20250101, -0125
    if let Some(prefix) = strip_date_suffix(&lower) {
        prefix
    } else {
        lower
    }
}

fn strip_date_suffix(s: &str) -> Option<String> {
    let (prefix, suffix) = s.rsplit_once('-')?;
    // YYYYMMDD (8 digits) or MMDD (4 digits)
    if (suffix.len() == 8 || suffix.len() == 4) && suffix.chars().all(|c| c.is_ascii_digit()) {
        Some(prefix.to_owned())
    } else {
        None
    }
}

/// Known models with exact context window sizes in tokens.
static KNOWN_MODELS: &[(&str, u64)] = &[
    // Anthropic Claude
    ("claude-3-7-sonnet", 200_000),
    ("claude-3-7-sonnet-latest", 200_000),
    ("claude-3-5-sonnet", 200_000),
    ("claude-3-5-sonnet-latest", 200_000),
    ("claude-3-5-haiku", 200_000),
    ("claude-3-5-haiku-latest", 200_000),
    ("claude-3-opus", 200_000),
    ("claude-3-opus-latest", 200_000),
    ("claude-3-sonnet", 200_000),
    ("claude-3-haiku", 200_000),
    ("claude-2.1", 200_000),
    ("claude-2.0", 100_000),
    // OpenAI
    ("gpt-4o", 128_000),
    ("gpt-4o-mini", 128_000),
    ("gpt-4-turbo", 128_000),
    ("gpt-4-turbo-preview", 128_000),
    ("gpt-4.5-preview", 128_000),
    ("chatgpt-4o-latest", 128_000),
    ("o1", 200_000),
    ("o1-mini", 200_000),
    ("o1-preview", 200_000),
    ("o3", 200_000),
    ("o3-mini", 200_000),
    ("o4-mini", 200_000),
    ("gpt-4", 8_192),
    ("gpt-4-32k", 32_768),
    ("gpt-3.5-turbo", 16_385),
    ("gpt-3.5-turbo-16k", 16_385),
    // Google Gemini
    ("gemini-2.5-pro", 2_097_152),
    ("gemini-2.5-flash", 1_048_576),
    ("gemini-2.0-pro", 2_097_152),
    ("gemini-2.0-pro-exp", 2_097_152),
    ("gemini-2.0-flash", 1_048_576),
    ("gemini-2.0-flash-exp", 1_048_576),
    ("gemini-2.0-flash-thinking-exp", 1_048_576),
    ("gemini-1.5-pro", 2_097_152),
    ("gemini-1.5-flash", 1_048_576),
    ("gemini-1.5-flash-8b", 1_048_576),
    ("gemini-exp-1206", 2_097_152),
    // DeepSeek
    ("deepseek-chat", 128_000),
    ("deepseek-reasoner", 128_000),
    ("deepseek-v3", 128_000),
    ("deepseek-r1", 128_000),
    ("deepseek-coder", 128_000),
    // Meta Llama
    ("llama-3.3-70b-instruct", 128_000),
    ("llama-3.2-1b-instruct", 128_000),
    ("llama-3.2-3b-instruct", 128_000),
    ("llama-3.1-8b-instruct", 128_000),
    ("llama-3.1-70b-instruct", 128_000),
    ("llama-3.1-405b-instruct", 128_000),
    ("llama-3-8b-instruct", 8_192),
    ("llama-3-70b-instruct", 8_192),
    // Mistral
    ("codestral-latest", 256_000),
    ("codestral-2501", 256_000),
    ("mistral-large-latest", 128_000),
    ("mistral-small-latest", 128_000),
    ("mistral-nemo", 128_000),
    ("pixtral-12b", 128_000),
    ("pixtral-large-latest", 128_000),
    ("open-mixtral-8x22b", 65_536),
    ("open-mixtral-8x7b", 32_768),
    ("open-mistral-7b", 32_768),
    // Qwen
    ("qwen-2.5-coder-32b-instruct", 131_072),
    ("qwen-2.5-72b-instruct", 131_072),
    ("qwen-2.5-7b-instruct-1m", 1_000_000),
    ("qwq-32b-preview", 131_072),
    // Moonshot / Kimi
    ("kimi-k2.6", 1_048_576),
    ("kimi-k3.1-pro", 1_048_576),
    ("moonshot-v1-128k", 128_000),
    ("moonshot-v1-32k", 32_768),
    ("moonshot-v1-8k", 8_192),
    // xAI
    ("grok-2", 131_072),
    ("grok-3", 1_000_000),
    ("grok-3-mini", 1_000_000),
    ("grok-4.1-fast", 2_000_000),
    // Zhipu / GLM
    ("glm-5.3-flash", 1_000_000),
    ("glm-4-plus", 128_000),
    ("glm-4-flash", 128_000),
    // Cohere
    ("command-r-plus", 128_000),
    ("command-r", 128_000),
    // StepFun
    ("step-5-preview", 1_000_000),
    ("step-2", 1_000_000),
];

/// Resolve context window for a given model string:
/// 1. Exact match against known models after normalization.
/// 2. Pattern and family fallbacks (Gemini, Claude, GPT, DeepSeek, Qwen, etc.).
pub fn context_window(raw: &str) -> Option<u64> {
    let clean = normalize_model_id(raw);

    // 1. Exact table match
    for (name, window) in KNOWN_MODELS {
        if clean == *name {
            return Some(*window);
        }
    }

    // 2. Family and pattern rules
    if clean.contains("gemini") {
        if clean.contains("pro") {
            return Some(2_097_152);
        }
        return Some(1_048_576);
    }
    if clean.contains("claude") {
        return Some(200_000);
    }
    if clean.starts_with("o1") || clean.starts_with("o3") || clean.starts_with("o4") {
        return Some(200_000);
    }
    if clean.starts_with("gpt-4o")
        || clean.starts_with("gpt-4.5")
        || clean.starts_with("gpt-4-turbo")
    {
        return Some(128_000);
    }
    if clean.starts_with("gpt-4") {
        return Some(8_192);
    }
    if clean.starts_with("gpt-3.5") {
        return Some(16_385);
    }
    if clean.contains("deepseek") {
        return Some(128_000);
    }
    if clean.contains("codestral") {
        return Some(256_000);
    }
    if clean.contains("pixtral")
        || clean.contains("mistral-large")
        || clean.contains("mistral-small")
        || clean.contains("mistral-nemo")
    {
        return Some(128_000);
    }
    if clean.contains("mixtral-8x22b") {
        return Some(65_536);
    }
    if clean.contains("mixtral") || clean.contains("mistral") {
        return Some(32_768);
    }
    if clean.contains("llama-3") || clean.contains("llama3") {
        return Some(128_000);
    }
    if clean.contains("llama-2") || clean.contains("llama2") {
        return Some(4_096);
    }
    if clean.contains("qwen") {
        if clean.contains("1m") {
            return Some(1_000_000);
        }
        if clean.contains("2.5") || clean.contains("2") {
            return Some(131_072);
        }
        return Some(32_768);
    }
    if clean.contains("kimi") {
        return Some(1_048_576);
    }
    if clean.contains("moonshot") {
        return Some(128_000);
    }
    if clean.contains("glm-5") {
        return Some(1_000_000);
    }
    if clean.contains("glm-4") {
        return Some(128_000);
    }
    if clean.contains("grok-4") || clean.contains("grok-3") {
        return Some(1_000_000);
    }
    if clean.contains("grok") {
        return Some(131_072);
    }
    if clean.contains("step-5") || clean.contains("step-2") {
        return Some(1_000_000);
    }
    if clean.contains("step-1") {
        return Some(256_000);
    }
    if clean.contains("command-r") {
        return Some(128_000);
    }
    if clean.contains("sonar") {
        return Some(128_000);
    }
    if clean.contains("minimax") {
        return Some(1_000_000);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_model_identifier() {
        assert_eq!(
            normalize_model_id("anthropic/claude-3-7-sonnet-20250219"),
            "claude-3-7-sonnet"
        );
        assert_eq!(
            normalize_model_id("gemini-3.8-flash:medium"),
            "gemini-3.8-flash"
        );
        assert_eq!(normalize_model_id("openai/gpt-4o"), "gpt-4o");
        assert_eq!(normalize_model_id("o3-mini:high"), "o3-mini");
        assert_eq!(
            normalize_model_id("deepseek-ai/deepseek-r1:free"),
            "deepseek-r1"
        );
    }

    #[test]
    fn resolves_exact_and_pattern_windows() {
        assert_eq!(context_window("gemini-3.8-flash:medium"), Some(1_048_576));
        assert_eq!(context_window("gemini-2.0-pro"), Some(2_097_152));
        assert_eq!(
            context_window("anthropic/claude-3-7-sonnet-latest"),
            Some(200_000)
        );
        assert_eq!(context_window("openai/gpt-4o"), Some(128_000));
        assert_eq!(context_window("deepseek-v4-flash-chat"), Some(128_000));
        assert_eq!(context_window("codestral-latest"), Some(256_000));
        assert_eq!(context_window("o3-mini:high"), Some(200_000));
        assert_eq!(context_window("kimi-k2.6"), Some(1_048_576));
        assert_eq!(
            context_window("meta-llama/llama-3.3-70b-instruct"),
            Some(128_000)
        );
        assert_eq!(context_window("unknown-alien-model"), None);
    }
}
