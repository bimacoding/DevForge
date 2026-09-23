use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::Result;
use serde_json::{Value, json};

use crate::{
    mcp::McpHub,
    mode::AgentMode,
    permission::PermissionLevel,
    prompt::{AGENT_SYSTEM_PROMPT, ASK_SYSTEM_PROMPT, EDIT_SYSTEM_PROMPT},
    provider::{AgentEvent, ChatMessage, OpenAiCompatibleProvider, ProviderConfig},
    skills::load_skills_prompt,
    tools::{
        ToolCall, ToolContext, ToolResult, WorkspaceBackend, execute_tool,
        tool_permission, tools_for_mode,
    },
};

/// Prior turn for multi-turn chat continuity (user / assistant text only).
#[derive(Debug, Clone)]
pub struct HistoryTurn {
    pub role: crate::provider::Role,
    pub content: String,
}

pub struct AgentRequest {
    pub user_message: String,
    /// Optional image data-URLs (`data:image/...;base64,...`) for vision models.
    pub image_data_urls: Vec<String>,
    /// Earlier user/assistant messages in this conversation (oldest first).
    pub history: Vec<HistoryTurn>,
    pub context_preamble: String,
    pub mode: AgentMode,
    pub max_iterations: usize,
    pub cancel: Arc<AtomicBool>,
    pub require_tool_approval: bool,
    pub skills_enabled: bool,
    /// Optional MCP hub (tools merged into the model tool list).
    pub mcp: Option<Arc<McpHub>>,
}

pub struct AgentRuntime {
    provider: OpenAiCompatibleProvider,
}

impl AgentRuntime {
    pub fn new(config: ProviderConfig) -> Result<Self> {
        Ok(Self {
            provider: OpenAiCompatibleProvider::new(config)?,
        })
    }

    pub fn run_ask(
        &self,
        backend: &dyn WorkspaceBackend,
        request: AgentRequest,
        on_event: &mut dyn FnMut(AgentEvent),
        approve_tool: &mut dyn FnMut(&str, &str) -> bool,
    ) -> Result<()> {
        run_agent(&self.provider, backend, request, on_event, approve_tool)
    }

    pub fn run(
        &self,
        backend: &dyn WorkspaceBackend,
        request: AgentRequest,
        on_event: &mut dyn FnMut(AgentEvent),
        approve_tool: &mut dyn FnMut(&str, &str) -> bool,
    ) -> Result<()> {
        run_agent(&self.provider, backend, request, on_event, approve_tool)
    }
}

/// Back-compat alias for Ask/Edit/Agent loop.
pub fn run_ask(
    provider: &OpenAiCompatibleProvider,
    backend: &dyn WorkspaceBackend,
    request: AgentRequest,
    on_event: &mut dyn FnMut(AgentEvent),
    approve_tool: &mut dyn FnMut(&str, &str) -> bool,
) -> Result<()> {
    run_agent(provider, backend, request, on_event, approve_tool)
}

pub fn run_agent(
    provider: &OpenAiCompatibleProvider,
    backend: &dyn WorkspaceBackend,
    request: AgentRequest,
    on_event: &mut dyn FnMut(AgentEvent),
    approve_tool: &mut dyn FnMut(&str, &str) -> bool,
) -> Result<()> {
    on_event(AgentEvent::State("Analyzing"));

    let mut system = match request.mode {
        AgentMode::Ask => ASK_SYSTEM_PROMPT.to_string(),
        AgentMode::Edit => EDIT_SYSTEM_PROMPT.to_string(),
        AgentMode::Agent => AGENT_SYSTEM_PROMPT.to_string(),
    };
    if request.skills_enabled {
        let skills = load_skills_prompt(Some(backend.root()));
        system.push_str(&skills);
    }

    let mut messages = vec![ChatMessage::system(system)];
    if !request.context_preamble.is_empty() {
        messages.push(ChatMessage::system(request.context_preamble));
    }
    for turn in &request.history {
        match turn.role {
            crate::provider::Role::User => {
                messages.push(ChatMessage::user(turn.content.clone()));
            }
            crate::provider::Role::Assistant => {
                messages.push(ChatMessage::assistant_text(turn.content.clone()));
            }
            crate::provider::Role::System => {
                messages.push(ChatMessage::system(turn.content.clone()));
            }
            crate::provider::Role::Tool => {}
        }
    }
    messages.push(ChatMessage::user_with_images(
        request.user_message,
        request.image_data_urls,
    ));

    let tools = merge_tools(tools_for_mode(request.mode), request.mcp.as_deref());
    let tool_ctx = ToolContext {
        backend,
        max_file_bytes: 200_000,
    };

    let max_iters = request.max_iterations.max(1);
    for round in 0..max_iters {
        if request.cancel.load(Ordering::SeqCst) {
            on_event(AgentEvent::State("Cancelled"));
            on_event(AgentEvent::Done);
            return Ok(());
        }

        on_event(AgentEvent::Activity(format!(
            "Model request (round {})…",
            round + 1
        )));
        on_event(AgentEvent::State("Streaming"));
        let mut on_text = |chunk: &str| {
            if !chunk.is_empty() {
                on_event(AgentEvent::TextDelta(chunk.to_string()));
            }
        };
        let response =
            match provider.chat_stream(&messages, Some(&tools), &mut on_text) {
                Ok(r) => r,
                Err(e) => {
                    on_event(AgentEvent::Activity(
                        "Stream unavailable; retrying without SSE…".into(),
                    ));
                    match provider.chat(&messages, Some(&tools)) {
                        Ok(r) => {
                            if !r.content.is_empty() && r.tool_calls.is_empty() {
                                on_event(AgentEvent::TextDelta(r.content.clone()));
                            }
                            r
                        }
                        Err(e2) => {
                            on_event(AgentEvent::Error(format!(
                                "{e}; fallback: {e2}"
                            )));
                            on_event(AgentEvent::State("Error"));
                            on_event(AgentEvent::Done);
                            return Err(e2);
                        }
                    }
                }
            };

        if response.tool_calls.is_empty() {
            on_event(AgentEvent::State("Completed"));
            on_event(AgentEvent::Done);
            return Ok(());
        }

        messages.push(ChatMessage::assistant_tools(
            response.content,
            response.raw_tool_calls,
        ));

        for call in &response.tool_calls {
            if request.cancel.load(Ordering::SeqCst) {
                on_event(AgentEvent::State("Cancelled"));
                on_event(AgentEvent::Done);
                return Ok(());
            }

            let args = call.arguments.to_string();
            let needs_approval = request.require_tool_approval
                || tool_permission(&call.name) >= PermissionLevel::Confirm;
            if needs_approval && !approve_tool(&call.name, &args) {
                on_event(AgentEvent::Activity(format!(
                    "Skipped tool `{}`",
                    call.name
                )));
                messages.push(ChatMessage::tool(
                    &call.id,
                    "User skipped this tool call.",
                ));
                continue;
            }

            on_event(AgentEvent::State("Executing tool"));
            on_event(AgentEvent::ToolStart {
                name: call.name.clone(),
                arguments: args,
            });
            let result = dispatch_tool(&tool_ctx, call, request.mcp.as_deref());
            on_event(AgentEvent::ToolEnd {
                name: call.name.clone(),
                output: truncate_tool_output(&result.content),
                is_error: result.is_error,
            });
            messages.push(ChatMessage::tool(&call.id, result.content));
        }
    }

    on_event(AgentEvent::Error(
        "Reached max tool iterations without a final answer".into(),
    ));
    on_event(AgentEvent::State("Error"));
    on_event(AgentEvent::Done);
    Ok(())
}

fn merge_tools(base: Value, mcp: Option<&McpHub>) -> Value {
    let mut arr = base.as_array().cloned().unwrap_or_default();
    if let Some(hub) = mcp {
        if let Some(extra) = hub.tool_definitions().as_array() {
            arr.extend(extra.iter().cloned());
        }
    }
    json!(arr)
}

/// Maximum characters of a tool result forwarded to the UI, so a huge file
/// read cannot stall the event channel or blow up the bubble layout.
const MAX_TOOL_OUTPUT_CHARS: usize = 4_000;

fn truncate_tool_output(output: &str) -> String {
    if output.chars().count() <= MAX_TOOL_OUTPUT_CHARS {
        return output.to_string();
    }
    let head: String = output.chars().take(MAX_TOOL_OUTPUT_CHARS).collect();
    let omitted = output.chars().count() - MAX_TOOL_OUTPUT_CHARS;
    format!("{head}\n… ({omitted} more characters omitted)")
}

fn dispatch_tool(
    ctx: &ToolContext<'_>,
    call: &ToolCall,
    mcp: Option<&McpHub>,
) -> ToolResult {
    if call.name.starts_with("mcp__") {
        return match mcp {
            Some(hub) => match hub.call_tool(&call.name, &call.arguments) {
                Ok(content) => ToolResult {
                    content,
                    is_error: false,
                },
                Err(e) => ToolResult {
                    content: e.to_string(),
                    is_error: true,
                },
            },
            None => ToolResult {
                content: "MCP is not enabled".into(),
                is_error: true,
            },
        };
    }
    execute_tool(ctx, call)
}
