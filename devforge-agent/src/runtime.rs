use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::Result;

use crate::{
    mode::AgentMode,
    prompt::ASK_SYSTEM_PROMPT,
    provider::{AgentEvent, ChatMessage, OpenAiCompatibleProvider, ProviderConfig},
    tools::{ToolContext, WorkspaceBackend, ask_tools, execute_tool},
};

pub struct AgentRequest {
    pub user_message: String,
    pub context_preamble: String,
    pub mode: AgentMode,
    pub max_iterations: usize,
    pub cancel: Arc<AtomicBool>,
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
    ) -> Result<()> {
        if request.mode != AgentMode::Ask {
            anyhow::bail!("Phase 1 runtime currently supports Ask mode only");
        }
        run_ask(&self.provider, backend, request, on_event)
    }
}

pub fn run_ask(
    provider: &OpenAiCompatibleProvider,
    backend: &dyn WorkspaceBackend,
    request: AgentRequest,
    on_event: &mut dyn FnMut(AgentEvent),
) -> Result<()> {
    on_event(AgentEvent::State("Analyzing"));

    let mut messages = vec![ChatMessage::system(ASK_SYSTEM_PROMPT)];
    if !request.context_preamble.is_empty() {
        messages.push(ChatMessage::system(request.context_preamble));
    }
    messages.push(ChatMessage::user(request.user_message));

    let tools = ask_tools();
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
        let response = match provider.chat(&messages, Some(&tools)) {
            Ok(r) => r,
            Err(e) => {
                on_event(AgentEvent::Error(e.to_string()));
                on_event(AgentEvent::State("Error"));
                on_event(AgentEvent::Done);
                return Err(e);
            }
        };

        if response.tool_calls.is_empty() {
            if !response.content.is_empty() {
                on_event(AgentEvent::TextDelta(response.content));
            }
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
            on_event(AgentEvent::State("Executing tool"));
            on_event(AgentEvent::ToolStart {
                name: call.name.clone(),
                arguments: call.arguments.to_string(),
            });
            let result = execute_tool(&tool_ctx, call);
            on_event(AgentEvent::ToolEnd {
                name: call.name.clone(),
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
