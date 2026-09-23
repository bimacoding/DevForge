use serde::{Deserialize, Serialize};
use structdesc::FieldNames;

use crate::ai_providers::{McpServerConfig, resolve_api_key, resolve_base_url};

#[derive(FieldNames, Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct AiConfig {
    #[field_names(desc = "Show the AI Assistant panel and related commands")]
    pub enabled: bool,

    #[field_names(desc = "Default chat mode: ask, edit, or agent")]
    pub default_mode: String,

    #[field_names(
        desc = "AI provider: openai, openrouter, anthropic, gemini, grok, hermes, ollama, openai-compatible"
    )]
    pub provider: String,

    #[field_names(desc = "Default model when Auto is selected")]
    pub model: String,

    #[field_names(
        desc = "Extra models for the picker (comma-separated IDs you can add anytime)"
    )]
    #[serde(default)]
    pub extra_models: String,

    #[field_names(
        desc = "API key — prefer env vars (OPENAI_API_KEY, OPENROUTER_API_KEY, GEMINI_API_KEY, XAI_API_KEY, …)"
    )]
    pub api_key: String,

    #[field_names(
        desc = "Provider base URL (auto-filled per provider; override for custom gateways)"
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

    #[field_names(
        desc = "Require explicit approval of the agent plan before editing"
    )]
    pub require_plan_approval: bool,

    #[field_names(
        desc = "Ask for Run / Skip before each tool call (MCP and workspace tools)"
    )]
    #[serde(default = "default_true")]
    pub require_tool_approval: bool,

    #[field_names(desc = "Enable MCP servers listed below")]
    #[serde(default)]
    pub mcp_enabled: bool,

    #[field_names(skip)]
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,

    #[field_names(
        desc = "Load Skills, MCPs, Subagents, Rules, Commands and Hooks from .devforge (and .cursor) into the agent"
    )]
    #[serde(default = "default_true", alias = "skills-enabled")]
    pub assets_enabled: bool,

    #[field_names(
        desc = "Run lifecycle hooks from hooks.json. Hooks execute local shell commands — keep off unless you trust them"
    )]
    #[serde(default)]
    pub hooks_enabled: bool,

    #[field_names(
        desc = "Max concurrent AI runs across chats (1–8). Extra prompts on a busy chat still queue."
    )]
    #[serde(default = "default_max_parallel")]
    pub max_parallel_runs: usize,
}

fn default_true() -> bool {
    true
}

fn default_max_parallel() -> usize {
    2
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_mode: "ask".into(),
            provider: "openrouter".into(),
            model: "openai/gpt-4o-mini".into(),
            extra_models: String::new(),
            api_key: String::new(),
            base_url: "https://openrouter.ai/api/v1".into(),
            temperature: 0.2,
            max_tokens: 4096,
            context_limit: 128_000,
            max_iterations: 8,
            tool_call_limit: 40,
            auto_run_safe_terminal: false,
            require_plan_approval: true,
            require_tool_approval: true,
            mcp_enabled: false,
            mcp_servers: Vec::new(),
            assets_enabled: true,
            hooks_enabled: false,
            max_parallel_runs: 2,
        }
    }
}

impl AiConfig {
    pub fn provider_base_url(&self) -> String {
        resolve_base_url(&self.provider, &self.base_url)
    }

    pub fn resolved_api_key(&self) -> String {
        resolve_api_key(&self.provider, &self.api_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialise a config the same way the app writes `settings.toml`.
    fn to_document(cfg: &AiConfig) -> toml_edit::Document {
        toml_edit::ser::to_document(cfg).expect("serialize AiConfig")
    }

    fn from_document(doc: &toml_edit::Document) -> AiConfig {
        toml::from_str(&doc.to_string()).expect("deserialize AiConfig")
    }

    /// `hooks-enabled` spawns local processes, so it must default to off and
    /// must round-trip through the kebab-case key used in `settings.toml`.
    #[test]
    fn hooks_enabled_is_opt_in_and_kebab_cased() {
        let default = AiConfig::default();
        assert!(default.assets_enabled, "assets should be on by default");
        assert!(!default.hooks_enabled, "hooks must be opt-in");

        let mut doc = to_document(&default);
        assert!(
            doc.contains_key("hooks-enabled"),
            "settings.toml key must be kebab-case, got: {doc}"
        );
        assert!(!doc["hooks-enabled"].as_bool().unwrap());

        doc["hooks-enabled"] = toml_edit::value(true);
        assert!(from_document(&doc).hooks_enabled);
    }

    /// The legacy `skills-enabled` key must keep working as an alias.
    #[test]
    fn legacy_skills_enabled_alias_still_works() {
        let mut doc = to_document(&AiConfig::default());
        doc.remove("assets-enabled");
        doc["skills-enabled"] = toml_edit::value(false);

        let parsed = from_document(&doc);
        assert!(!parsed.assets_enabled);
    }
}
