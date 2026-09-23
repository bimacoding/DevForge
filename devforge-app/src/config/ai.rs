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
        desc = "Load Agent Skills from ~/.devforge/skills and .devforge/skills into the system prompt"
    )]
    #[serde(default = "default_true")]
    pub skills_enabled: bool,

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
            skills_enabled: true,
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
