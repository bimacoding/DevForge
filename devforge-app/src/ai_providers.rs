//! Built-in AI provider presets and model catalogs (OpenAI-compatible endpoints).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub default_base_url: &'static str,
    pub env_keys: &'static [&'static str],
    pub models: &'static [&'static str],
}

pub const PROVIDER_PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "openai",
        label: "OpenAI",
        default_base_url: "https://api.openai.com/v1",
        env_keys: &["OPENAI_API_KEY", "DEVFORGE_AI_API_KEY"],
        models: &["gpt-4.1", "gpt-4o", "gpt-4o-mini", "o4-mini", "o3-mini"],
    },
    ProviderPreset {
        id: "openrouter",
        label: "OpenRouter",
        default_base_url: "https://openrouter.ai/api/v1",
        env_keys: &["OPENROUTER_API_KEY", "DEVFORGE_AI_API_KEY"],
        models: &[
            "openai/gpt-4o",
            "anthropic/claude-sonnet-4",
            "google/gemini-2.0-flash-001",
            "x-ai/grok-2",
            "nousresearch/hermes-3-llama-3.1-405b",
        ],
    },
    ProviderPreset {
        id: "anthropic",
        label: "Claude (Anthropic via OpenAI-compat / OpenRouter)",
        default_base_url: "https://openrouter.ai/api/v1",
        env_keys: &[
            "OPENROUTER_API_KEY",
            "ANTHROPIC_API_KEY",
            "DEVFORGE_AI_API_KEY",
        ],
        models: &[
            "anthropic/claude-sonnet-4",
            "anthropic/claude-3.5-sonnet",
            "anthropic/claude-3-haiku",
        ],
    },
    ProviderPreset {
        id: "gemini",
        label: "Google Gemini",
        default_base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        env_keys: &["GEMINI_API_KEY", "GOOGLE_API_KEY", "DEVFORGE_AI_API_KEY"],
        models: &[
            "gemini-2.0-flash",
            "gemini-2.0-flash-lite",
            "gemini-1.5-pro",
            "gemini-1.5-flash",
        ],
    },
    ProviderPreset {
        id: "grok",
        label: "Grok (xAI)",
        default_base_url: "https://api.x.ai/v1",
        env_keys: &["XAI_API_KEY", "DEVFORGE_AI_API_KEY"],
        models: &["grok-2", "grok-2-vision-1212", "grok-3"],
    },
    ProviderPreset {
        id: "hermes",
        label: "Hermes (Nous / OpenRouter)",
        default_base_url: "https://openrouter.ai/api/v1",
        env_keys: &["OPENROUTER_API_KEY", "DEVFORGE_AI_API_KEY"],
        models: &[
            "nousresearch/hermes-3-llama-3.1-405b",
            "nousresearch/hermes-3-llama-3.1-70b",
            "nousresearch/hermes-2-pro-llama-3-8b",
        ],
    },
    ProviderPreset {
        id: "ollama",
        label: "Ollama (local)",
        default_base_url: "http://127.0.0.1:11434/v1",
        env_keys: &[],
        models: &[
            "llama3.2",
            "llama3.1",
            "qwen2.5-coder",
            "mistral",
            "codellama",
        ],
    },
    ProviderPreset {
        id: "openai-compatible",
        label: "Custom OpenAI-compatible",
        default_base_url: "https://api.openai.com/v1",
        env_keys: &["DEVFORGE_AI_API_KEY", "OPENAI_API_KEY"],
        models: &["gpt-4o-mini", "llama3.2"],
    },
];

pub fn provider_preset(id: &str) -> Option<&'static ProviderPreset> {
    let id = id.trim();
    PROVIDER_PRESETS
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(id))
}

pub fn provider_ids() -> Vec<&'static str> {
    PROVIDER_PRESETS.iter().map(|p| p.id).collect()
}

/// Resolve base URL for a provider id, falling back to stored URL.
pub fn resolve_base_url(provider: &str, configured: &str) -> String {
    let configured = configured.trim();
    if let Some(preset) = provider_preset(provider) {
        if provider.eq_ignore_ascii_case("openai-compatible")
            && !configured.is_empty()
        {
            return configured.to_string();
        }
        if provider.eq_ignore_ascii_case("ollama")
            && (configured.is_empty() || configured.contains("openai.com"))
        {
            return preset.default_base_url.to_string();
        }
        if configured.is_empty()
            || configured == "https://api.openai.com/v1"
            || (provider != "openai"
                && provider != "openai-compatible"
                && configured.contains("api.openai.com"))
        {
            return preset.default_base_url.to_string();
        }
    }
    if configured.is_empty() {
        "https://api.openai.com/v1".into()
    } else {
        configured.to_string()
    }
}

/// Resolve API key from config field then provider-specific env vars.
pub fn resolve_api_key(provider: &str, configured: &str) -> String {
    if !configured.trim().is_empty() {
        return configured.trim().to_string();
    }
    if let Some(preset) = provider_preset(provider) {
        for key in preset.env_keys {
            if let Ok(v) = std::env::var(key) {
                if !v.is_empty() {
                    return v;
                }
            }
        }
    }
    std::env::var("DEVFORGE_AI_API_KEY")
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub enabled: bool,
}

/// Models shown in the picker: Auto + provider presets + user extras.
pub fn model_picker_list(provider: &str, extra_models: &str) -> Vec<String> {
    let mut out = vec!["Auto".to_string()];
    let mut seen = std::collections::HashSet::new();
    seen.insert("auto".to_string());

    if let Some(preset) = provider_preset(provider) {
        for m in preset.models {
            let key = m.to_ascii_lowercase();
            if seen.insert(key) {
                out.push((*m).to_string());
            }
        }
    }

    for part in extra_models.split(|c| c == ',' || c == '\n' || c == ';') {
        let m = part.trim();
        if m.is_empty() {
            continue;
        }
        let key = m.to_ascii_lowercase();
        if seen.insert(key) {
            out.push(m.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_picker_includes_auto_and_provider_models() {
        let list = model_picker_list("openai", "my-custom-model");
        assert_eq!(list[0], "Auto");
        assert!(list.iter().any(|m| m == "gpt-4o"));
        assert!(list.iter().any(|m| m == "my-custom-model"));
    }

    #[test]
    fn resolve_base_url_for_presets() {
        assert!(resolve_base_url("openrouter", "").contains("openrouter.ai"));
        assert!(resolve_base_url("grok", "").contains("api.x.ai"));
        assert!(
            resolve_base_url("gemini", "https://api.openai.com/v1")
                .contains("generativelanguage.googleapis.com")
        );
        assert_eq!(
            resolve_base_url("openai-compatible", "http://localhost:8080/v1"),
            "http://localhost:8080/v1"
        );
    }

    #[test]
    fn provider_preset_lookup() {
        assert!(provider_preset("OpenRouter").is_some());
        assert!(provider_preset("hermes").is_some());
        assert!(provider_preset("nope").is_none());
    }
}
