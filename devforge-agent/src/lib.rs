//! DevForge AI coding agent: Ask/Edit/Agent runtime, tools, and providers.

pub mod assets;
pub mod hooks;
pub mod mcp;
pub mod mode;
pub mod parallel;
pub mod permission;
pub mod prompt;
pub mod provider;
pub mod runtime;
pub mod secrets;
pub mod skills;
pub mod state;
pub mod tools;

pub use assets::{
    AgentAsset, AssetKind, AssetLocation, AssetScope, asset_doc_hint,
    asset_locations, asset_template, build_prompt, config_roots, create_asset,
    default_location, delete_asset, discover_all, discover_kind, import_asset,
    import_directory, load_assets_prompt, mcp_asset_specs, parse_frontmatter,
    read_asset, read_json_asset, read_markdown_asset, sanitize_asset_name,
    set_asset_enabled,
};
pub use hooks::{
    HOOK_TIMEOUT, HookBundle, HookCommand, HookEvent, HookOutcome, HookRunner,
    HookSource, discover_bundles, merged_commands, parse_hook_bundle,
};
pub use mcp::{McpHub, McpServerSpec};
pub use mode::AgentMode;
pub use parallel::{
    RunId, TaggedAgentEvent, can_start_another_run, clamp_max_parallel,
    run_parallel_jobs,
};
pub use permission::PermissionLevel;
pub use provider::{
    AgentEvent, ChatMessage, OpenAiCompatibleProvider, ProviderConfig, Role,
    sse_data_payload,
};
pub use runtime::{AgentRequest, AgentRuntime, HistoryTurn, run_agent, run_ask};
pub use skills::load_skills_prompt;
pub use state::AgentState;
pub use tools::{
    FsWorkspaceBackend, ToolCall, ToolContext, ToolResult, WorkspaceBackend,
    ask_tools, edit_tools, execute_tool, tools_for_mode,
};
