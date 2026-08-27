use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::tools::ToolCall;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<Value>>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    pub fn assistant_text(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    pub fn assistant_tools(content: String, tool_calls: Vec<Value>) -> Self {
        Self {
            role: Role::Assistant,
            content,
            tool_call_id: None,
            tool_calls: Some(tool_calls),
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    State(&'static str),
    Activity(String),
    TextDelta(String),
    ToolStart { name: String, arguments: String },
    ToolEnd { name: String, is_error: bool },
    Error(String),
    Done,
}

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
}

impl ProviderConfig {
    pub fn resolve_api_key(&self) -> String {
        if !self.api_key.is_empty() {
            return self.api_key.clone();
        }
        std::env::var("DEVFORGE_AI_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .unwrap_or_default()
    }

    pub fn chat_completions_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if base.ends_with("/v1") {
            format!("{base}/chat/completions")
        } else {
            format!("{base}/v1/chat/completions")
        }
    }
}

pub struct OpenAiCompatibleProvider {
    pub config: ProviderConfig,
    client: reqwest::blocking::Client,
}

impl OpenAiCompatibleProvider {
    pub fn new(config: ProviderConfig) -> anyhow::Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;
        Ok(Self { config, client })
    }

    pub fn chat(
        &self,
        messages: &[ChatMessage],
        tools: Option<&Value>,
    ) -> anyhow::Result<ProviderResponse> {
        let mut body = json!({
            "model": self.config.model,
            "temperature": self.config.temperature,
            "max_tokens": self.config.max_tokens,
            "messages": messages.iter().map(message_to_json).collect::<Vec<_>>(),
        });
        if let Some(tools) = tools {
            body["tools"] = tools.clone();
            body["tool_choice"] = json!("auto");
        }

        let mut req = self
            .client
            .post(self.config.chat_completions_url())
            .header("Content-Type", "application/json");
        let key = self.config.resolve_api_key();
        if !key.is_empty() {
            req = req.bearer_auth(key);
        }

        let resp = req.json(&body).send()?;
        let status = resp.status();
        let text = resp.text()?;
        if !status.is_success() {
            anyhow::bail!("provider HTTP {status}: {text}");
        }
        let parsed: Value = serde_json::from_str(&text)?;
        parse_provider_response(&parsed)
    }
}

#[derive(Debug, Clone)]
pub struct ProviderResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub raw_tool_calls: Vec<Value>,
}

fn message_to_json(msg: &ChatMessage) -> Value {
    let role = match msg.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    };
    let mut obj = json!({
        "role": role,
        "content": msg.content,
    });
    if let Some(id) = &msg.tool_call_id {
        obj["tool_call_id"] = json!(id);
    }
    if let Some(calls) = &msg.tool_calls {
        obj["tool_calls"] = json!(calls);
    }
    obj
}

fn parse_provider_response(parsed: &Value) -> anyhow::Result<ProviderResponse> {
    let choice = parsed
        .pointer("/choices/0/message")
        .ok_or_else(|| anyhow::anyhow!("missing choices[0].message"))?;
    let content = choice
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut tool_calls = Vec::new();
    let mut raw_tool_calls = Vec::new();
    if let Some(arr) = choice.get("tool_calls").and_then(|v| v.as_array()) {
        for call in arr {
            raw_tool_calls.push(call.clone());
            let id = call
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = call
                .pointer("/function/name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let args_str = call
                .pointer("/function/arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let arguments = serde_json::from_str(args_str).unwrap_or(json!({}));
            tool_calls.push(ToolCall {
                id,
                name,
                arguments,
            });
        }
    }
    Ok(ProviderResponse {
        content,
        tool_calls,
        raw_tool_calls,
    })
}
