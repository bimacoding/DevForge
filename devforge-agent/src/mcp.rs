//! Minimal MCP (Model Context Protocol) stdio client for tool listing/calling.

use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::Mutex,
};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

pub struct McpServerSpec {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

struct McpSession {
    name: String,
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

pub struct McpHub {
    sessions: Mutex<Vec<McpSession>>,
}

impl McpHub {
    pub fn connect(specs: &[McpServerSpec]) -> Self {
        let mut sessions = Vec::new();
        for spec in specs {
            match McpSession::spawn(spec) {
                Ok(s) => sessions.push(s),
                Err(e) => {
                    tracing::warn!("MCP `{}` failed to start: {e}", spec.name);
                }
            }
        }
        Self {
            sessions: Mutex::new(sessions),
        }
    }

    pub fn tool_definitions(&self) -> Value {
        let mut tools = Vec::new();
        let mut sessions = self.sessions.lock().unwrap();
        for session in sessions.iter_mut() {
            match session.list_tools() {
                Ok(list) => {
                    for tool in list {
                        let name = tool
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("tool");
                        let description = tool
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let parameters = tool
                            .get("inputSchema")
                            .cloned()
                            .unwrap_or_else(|| json!({ "type": "object" }));
                        tools.push(json!({
                            "type": "function",
                            "function": {
                                "name": format!("mcp__{}__{name}", session.name),
                                "description": format!("[MCP:{}] {description}", session.name),
                                "parameters": parameters,
                            }
                        }));
                    }
                }
                Err(e) => tracing::warn!("MCP list tools {}: {e}", session.name),
            }
        }
        json!(tools)
    }

    pub fn call_tool(
        &self,
        qualified_name: &str,
        arguments: &Value,
    ) -> Result<String> {
        let Some((server, tool)) = parse_mcp_tool_name(qualified_name) else {
            anyhow::bail!("not an MCP tool: {qualified_name}");
        };
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .iter_mut()
            .find(|s| s.name == server)
            .ok_or_else(|| anyhow!("MCP server `{server}` not connected"))?;
        session.call_tool(tool, arguments)
    }
}

impl Drop for McpHub {
    fn drop(&mut self) {
        let mut sessions = self.sessions.lock().unwrap();
        for session in sessions.iter_mut() {
            let _ = session.child.kill();
        }
    }
}

fn parse_mcp_tool_name(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    Some((server, tool))
}

impl McpSession {
    fn spawn(spec: &McpServerSpec) -> Result<Self> {
        let mut child = Command::new(&spec.command)
            .args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawn MCP {}", spec.command))?;
        let stdin = child.stdin.take().context("mcp stdin")?;
        let stdout = child.stdout.take().context("mcp stdout")?;
        let mut session = Self {
            name: sanitize_name(&spec.name),
            child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: 1,
        };
        session.initialize()?;
        Ok(session)
    }

    fn initialize(&mut self) -> Result<()> {
        let _ = self.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "devforge", "version": "0.4.6" }
            }),
        )?;
        self.notify("notifications/initialized", json!({}))?;
        Ok(())
    }

    fn list_tools(&mut self) -> Result<Vec<Value>> {
        let result = self.request("tools/list", json!({}))?;
        Ok(result
            .get("tools")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default())
    }

    fn call_tool(&mut self, name: &str, arguments: &Value) -> Result<String> {
        let result = self.request(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments,
            }),
        )?;
        if let Some(content) = result.get("content").and_then(|c| c.as_array()) {
            let mut out = String::new();
            for part in content {
                if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(t);
                }
            }
            if !out.is_empty() {
                return Ok(out);
            }
        }
        Ok(result.to_string())
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_message(&msg)?;
        loop {
            let reply = self.read_message()?;
            if reply.get("id").and_then(|v| v.as_u64()) == Some(id) {
                if let Some(err) = reply.get("error") {
                    anyhow::bail!("MCP error: {err}");
                }
                return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_message(&msg)
    }

    fn write_message(&mut self, msg: &Value) -> Result<()> {
        let body = serde_json::to_vec(msg)?;
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len())?;
        self.stdin.write_all(&body)?;
        self.stdin.flush()?;
        Ok(())
    }

    fn read_message(&mut self) -> Result<Value> {
        let mut content_length = None;
        loop {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line)?;
            if n == 0 {
                anyhow::bail!("MCP stdout closed");
            }
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                break;
            }
            if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
                content_length = Some(rest.trim().parse::<usize>()?);
            }
        }
        let len = content_length.context("missing Content-Length")?;
        let mut buf = vec![0_u8; len];
        self.reader.read_exact(&mut buf)?;
        Ok(serde_json::from_slice(&buf)?)
    }
}

trait ReadExact {
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()>;
}

impl ReadExact for BufReader<std::process::ChildStdout> {
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        use std::io::Read;
        Read::read_exact(self, buf).map_err(Into::into)
    }
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
