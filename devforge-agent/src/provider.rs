use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader},
};

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
    /// When set, serialized as OpenAI multimodal `content` array (text + images).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_parts: Option<Vec<Value>>,
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
            content_parts: None,
            tool_call_id: None,
            tool_calls: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            content_parts: None,
            tool_call_id: None,
            tool_calls: None,
        }
    }

    /// User message with optional image data-URLs for vision-capable models.
    pub fn user_with_images(
        text: impl Into<String>,
        image_data_urls: impl IntoIterator<Item = String>,
    ) -> Self {
        let text = text.into();
        let mut parts = vec![json!({ "type": "text", "text": text.clone() })];
        let mut has_image = false;
        for url in image_data_urls {
            has_image = true;
            parts.push(json!({
                "type": "image_url",
                "image_url": { "url": url }
            }));
        }
        Self {
            role: Role::User,
            content: text,
            content_parts: if has_image { Some(parts) } else { None },
            tool_call_id: None,
            tool_calls: None,
        }
    }

    pub fn assistant_text(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            content_parts: None,
            tool_call_id: None,
            tool_calls: None,
        }
    }

    pub fn assistant_tools(content: String, tool_calls: Vec<Value>) -> Self {
        Self {
            role: Role::Assistant,
            content,
            content_parts: None,
            tool_call_id: None,
            tool_calls: Some(tool_calls),
        }
    }

    pub fn tool(
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            content_parts: None,
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
    ToolStart {
        name: String,
        arguments: String,
    },
    ToolEnd {
        name: String,
        /// Tool result content (truncated by the caller for display).
        output: String,
        is_error: bool,
    },
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
    /// Extra env keys to try after `api_key` / DEVFORGE_AI_API_KEY / OPENAI_API_KEY.
    pub extra_env_keys: Vec<String>,
}

impl ProviderConfig {
    pub fn resolve_api_key(&self) -> String {
        if !self.api_key.is_empty() {
            return self.api_key.clone();
        }
        for key in &self.extra_env_keys {
            if let Ok(v) = std::env::var(key) {
                if !v.is_empty() {
                    return v;
                }
            }
        }
        std::env::var("DEVFORGE_AI_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .unwrap_or_default()
    }

    pub fn chat_completions_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if base.ends_with("/v1")
            || base.ends_with("/openai")
            || base.contains("/openai")
        {
            if base.ends_with("/chat/completions") {
                return base.to_string();
            }
            // Gemini OpenAI-compat already ends with /openai
            if base.ends_with("/openai") {
                return format!("{base}/chat/completions");
            }
            return format!("{base}/chat/completions");
        }
        format!("{base}/v1/chat/completions")
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

    /// OpenAI-compatible SSE stream (`stream: true`). Emits text deltas via `on_text`.
    pub fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: Option<&Value>,
        on_text: &mut dyn FnMut(&str),
    ) -> anyhow::Result<ProviderResponse> {
        let mut body = json!({
            "model": self.config.model,
            "temperature": self.config.temperature,
            "max_tokens": self.config.max_tokens,
            "stream": true,
            "messages": messages.iter().map(message_to_json).collect::<Vec<_>>(),
        });
        if let Some(tools) = tools {
            body["tools"] = tools.clone();
            body["tool_choice"] = json!("auto");
        }

        let mut req = self
            .client
            .post(self.config.chat_completions_url())
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream");
        let key = self.config.resolve_api_key();
        if !key.is_empty() {
            req = req.bearer_auth(key);
        }

        let resp = req.json(&body).send()?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().unwrap_or_default();
            anyhow::bail!("provider HTTP {status}: {text}");
        }

        let mut reader = BufReader::new(resp);
        let mut line = String::new();
        let mut acc = StreamAccumulator::default();

        loop {
            line.clear();
            let n = reader.read_line(&mut line)?;
            if n == 0 {
                break;
            }
            let Some(data) = sse_data_payload(&line) else {
                continue;
            };
            if data == "[DONE]" {
                break;
            }
            let parsed: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            // Non-stream fallback body if the server ignored `stream: true`
            if parsed.pointer("/choices/0/message").is_some() {
                let resp = parse_provider_response(&parsed)?;
                if !resp.content.is_empty() {
                    on_text(&resp.content);
                }
                return Ok(resp);
            }
            if let Some(delta) = parsed.pointer("/choices/0/delta") {
                acc.apply_delta(delta, on_text);
            }
        }

        Ok(acc.into_response())
    }
}

#[derive(Debug, Clone)]
pub struct ProviderResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub raw_tool_calls: Vec<Value>,
}

#[derive(Default)]
struct StreamAccumulator {
    content: String,
    tools: BTreeMap<usize, ToolCallBuilder>,
}

#[derive(Default)]
struct ToolCallBuilder {
    id: String,
    name: String,
    arguments: String,
}

impl StreamAccumulator {
    fn apply_delta(&mut self, delta: &Value, on_text: &mut dyn FnMut(&str)) {
        if let Some(c) = delta.get("content").and_then(|v| v.as_str()) {
            if !c.is_empty() {
                self.content.push_str(c);
                on_text(c);
            }
        }
        if let Some(arr) = delta.get("tool_calls").and_then(|v| v.as_array()) {
            for call in arr {
                let idx =
                    call.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let entry = self.tools.entry(idx).or_default();
                if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
                    if !id.is_empty() {
                        entry.id = id.to_string();
                    }
                }
                if let Some(name) =
                    call.pointer("/function/name").and_then(|v| v.as_str())
                {
                    if !name.is_empty() {
                        entry.name = name.to_string();
                    }
                }
                if let Some(args) =
                    call.pointer("/function/arguments").and_then(|v| v.as_str())
                {
                    entry.arguments.push_str(args);
                }
            }
        }
    }

    fn into_response(self) -> ProviderResponse {
        let mut tool_calls = Vec::new();
        let mut raw_tool_calls = Vec::new();
        for (_idx, b) in self.tools {
            let arguments: Value =
                serde_json::from_str(&b.arguments).unwrap_or(json!({}));
            let raw = json!({
                "id": b.id,
                "type": "function",
                "function": {
                    "name": b.name,
                    "arguments": b.arguments,
                }
            });
            raw_tool_calls.push(raw);
            tool_calls.push(ToolCall {
                id: b.id,
                name: b.name,
                arguments,
            });
        }
        ProviderResponse {
            content: self.content,
            tool_calls,
            raw_tool_calls,
        }
    }
}

/// Extract SSE `data:` payload, or `None` for comments / blank / non-data lines.
pub fn sse_data_payload(line: &str) -> Option<&str> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.is_empty() || line.starts_with(':') {
        return None;
    }
    let rest = line.strip_prefix("data:")?;
    Some(rest.trim_start())
}

fn message_to_json(msg: &ChatMessage) -> Value {
    let role = match msg.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    };
    let content = if let Some(parts) = &msg.content_parts {
        json!(parts)
    } else {
        json!(msg.content)
    };
    let mut obj = json!({
        "role": role,
        "content": content,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_data_payload_parses() {
        assert_eq!(sse_data_payload("data: hello"), Some("hello"));
        assert_eq!(sse_data_payload("data:  {\"a\":1}"), Some("{\"a\":1}"));
        assert_eq!(sse_data_payload("data: [DONE]"), Some("[DONE]"));
        assert_eq!(sse_data_payload(": keep-alive"), None);
        assert_eq!(sse_data_payload(""), None);
        assert_eq!(sse_data_payload("event: message"), None);
    }

    #[test]
    fn stream_accumulator_merges_text_and_tools() {
        let mut acc = StreamAccumulator::default();
        let mut chunks = Vec::new();
        let mut on_text = |s: &str| chunks.push(s.to_string());

        acc.apply_delta(&json!({"content": "Wor"}), &mut on_text);
        acc.apply_delta(&json!({"content": "ld"}), &mut on_text);
        acc.apply_delta(
            &json!({
                "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "function": { "name": "read_file", "arguments": "{\"p" }
                }]
            }),
            &mut on_text,
        );
        acc.apply_delta(
            &json!({
                "tool_calls": [{
                    "index": 0,
                    "function": { "arguments": "ath\":\"a.rs\"}" }
                }]
            }),
            &mut on_text,
        );

        assert_eq!(chunks, vec!["Wor", "ld"]);
        let resp = acc.into_response();
        assert_eq!(resp.content, "World");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].name, "read_file");
        assert_eq!(resp.tool_calls[0].id, "call_1");
        assert_eq!(resp.tool_calls[0].arguments["path"], "a.rs");
    }
}
