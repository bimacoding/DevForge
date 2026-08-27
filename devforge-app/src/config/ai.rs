use serde::{Deserialize, Serialize};
use structdesc::FieldNames;

#[derive(FieldNames, Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct AiConfig {
    #[field_names(desc = "Enable the built-in AI Assistant panel and commands")]
    pub enabled: bool,

    #[field_names(
        desc = "Default agent mode: ask (read-only), edit (not yet), or agent (not yet)"
    )]
    pub default_mode: String,

    #[field_names(
        desc = "Provider kind: openai-compatible, anthropic (planned), or ollama"
    )]
    pub provider: String,

    #[field_names(desc = "Model name sent to the provider")]
    pub model: String,

    #[field_names(
        desc = "API key (prefer env DEVFORGE_AI_API_KEY / OPENAI_API_KEY instead of storing here)"
    )]
    pub api_key: String,

    #[field_names(
        desc = "Provider base URL (OpenAI-compatible or Ollama, e.g. http://127.0.0.1:11434)"
    )]
    pub base_url: String,

    #[field_names(desc = "Sampling temperature")]
    pub temperature: f32,

    #[field_names(desc = "Maximum completion tokens")]
    pub max_tokens: u32,

    #[field_names(desc = "Soft context token budget used for context selection")]
    pub context_limit: u32,

    #[field_names(desc = "Maximum agent tool/model iterations per task")]
    pub max_iterations: usize,

    #[field_names(desc = "Maximum tool calls per task (hard cap)")]
    pub tool_call_limit: usize,

    #[field_names(
        desc = "Automatically run Safe-level terminal commands without prompting (Agent mode)"
    )]
    pub auto_run_safe_terminal: bool,

    #[field_names(desc = "Require explicit approval of the agent plan before editing")]
    pub require_plan_approval: bool,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_mode: "ask".into(),
            provider: "openai-compatible".into(),
            model: "gpt-4.1".into(),
            api_key: String::new(),
            base_url: "https://api.openai.com/v1".into(),
            temperature: 0.2,
            max_tokens: 4096,
            context_limit: 128_000,
            max_iterations: 5,
            tool_call_limit: 40,
            auto_run_safe_terminal: false,
            require_plan_approval: true,
        }
    }
}

impl AiConfig {
    pub fn provider_base_url(&self) -> String {
        if self.provider.eq_ignore_ascii_case("ollama")
            && (self.base_url.is_empty()
                || self.base_url.contains("openai.com"))
        {
            return "http://127.0.0.1:11434/v1".into();
        }
        self.base_url.clone()
    }
}
