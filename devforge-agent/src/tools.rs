use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use walkdir::WalkDir;

use crate::{
    permission::PermissionLevel,
    secrets::{is_sensitive_path, redact_secrets, resolve_under_root},
};

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
}

pub struct ToolContext<'a> {
    pub backend: &'a dyn WorkspaceBackend,
    pub max_file_bytes: usize,
}

pub trait WorkspaceBackend: Send + Sync {
    fn root(&self) -> &Path;
    fn read_file(&self, path: &Path) -> Result<String>;
    fn list_directory(&self, path: &Path) -> Result<Vec<String>>;
    fn search_code(&self, query: &str, max_results: usize) -> Result<Vec<SearchHit>>;
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub path: String,
    pub line: usize,
    pub text: String,
}

/// Filesystem-backed workspace tools (Ask mode).
pub struct FsWorkspaceBackend {
    root: PathBuf,
}

impl FsWorkspaceBackend {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl WorkspaceBackend for FsWorkspaceBackend {
    fn root(&self) -> &Path {
        &self.root
    }

    fn read_file(&self, path: &Path) -> Result<String> {
        fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
    }

    fn list_directory(&self, path: &Path) -> Result<Vec<String>> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let suffix = if entry.file_type()?.is_dir() {
                "/"
            } else {
                ""
            };
            entries.push(format!("{name}{suffix}"));
        }
        entries.sort();
        Ok(entries)
    }

    fn search_code(&self, query: &str, max_results: usize) -> Result<Vec<SearchHit>> {
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let mut hits = Vec::new();
        for entry in WalkDir::new(&self.root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if is_sensitive_path(path) {
                continue;
            }
            if should_skip_search_path(path) {
                continue;
            }
            let Ok(content) = fs::read_to_string(path) else {
                continue;
            };
            for (idx, line) in content.lines().enumerate() {
                if line.contains(query) {
                    let rel = path
                        .strip_prefix(&self.root)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .into_owned();
                    hits.push(SearchHit {
                        path: rel,
                        line: idx + 1,
                        text: redact_secrets(&truncate(line, 240))
                            .trim_end()
                            .to_string(),
                    });
                    if hits.len() >= max_results {
                        return Ok(hits);
                    }
                }
            }
        }
        Ok(hits)
    }
}

fn should_skip_search_path(path: &Path) -> bool {
    path.components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some("target" | "node_modules" | ".git" | "dist" | "build" | ".cargo")
        )
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}…")
    }
}

pub fn ask_tools() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a UTF-8 text file relative to the workspace root.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Relative path from workspace root" }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_directory",
                "description": "List files and directories under a path relative to the workspace root.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Relative directory path (default \".\")" }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "search_code",
                "description": "Lexical search for a string across the workspace (skips target/node_modules/.git).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "max_results": { "type": "integer", "minimum": 1, "maximum": 100 }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_project_structure",
                "description": "Return a shallow tree of the project (depth-limited).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "max_depth": { "type": "integer", "minimum": 1, "maximum": 6 },
                        "max_entries": { "type": "integer", "minimum": 10, "maximum": 500 }
                    }
                }
            }
        }
    ])
}

pub fn tool_permission(name: &str) -> PermissionLevel {
    match name {
        "read_file" | "list_directory" | "search_code" | "get_project_structure" => {
            PermissionLevel::ReadOnly
        }
        _ => PermissionLevel::Dangerous,
    }
}

pub fn execute_tool(ctx: &ToolContext<'_>, call: &ToolCall) -> ToolResult {
    match call.name.as_str() {
        "read_file" => match tool_read_file(ctx, &call.arguments) {
            Ok(s) => ToolResult {
                content: s,
                is_error: false,
            },
            Err(e) => ToolResult {
                content: e.to_string(),
                is_error: true,
            },
        },
        "list_directory" => match tool_list_directory(ctx, &call.arguments) {
            Ok(s) => ToolResult {
                content: s,
                is_error: false,
            },
            Err(e) => ToolResult {
                content: e.to_string(),
                is_error: true,
            },
        },
        "search_code" => match tool_search_code(ctx, &call.arguments) {
            Ok(s) => ToolResult {
                content: s,
                is_error: false,
            },
            Err(e) => ToolResult {
                content: e.to_string(),
                is_error: true,
            },
        },
        "get_project_structure" => match tool_project_structure(ctx, &call.arguments)
        {
            Ok(s) => ToolResult {
                content: s,
                is_error: false,
            },
            Err(e) => ToolResult {
                content: e.to_string(),
                is_error: true,
            },
        },
        other => ToolResult {
            content: format!("unknown or disallowed tool in Ask mode: {other}"),
            is_error: true,
        },
    }
}

fn tool_read_file(ctx: &ToolContext<'_>, args: &Value) -> Result<String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing path"))?;
    let resolved = resolve_under_root(ctx.backend.root(), path)?;
    if is_sensitive_path(&resolved) {
        anyhow::bail!("refusing to read sensitive path: {path}");
    }
    let content = ctx.backend.read_file(&resolved)?;
    if content.len() > ctx.max_file_bytes {
        anyhow::bail!(
            "file too large ({} bytes > {} limit)",
            content.len(),
            ctx.max_file_bytes
        );
    }
    Ok(redact_secrets(&content))
}

fn tool_list_directory(ctx: &ToolContext<'_>, args: &Value) -> Result<String> {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let resolved = resolve_under_root(ctx.backend.root(), path)?;
    let entries = ctx.backend.list_directory(&resolved)?;
    Ok(entries.join("\n"))
}

fn tool_search_code(ctx: &ToolContext<'_>, args: &Value) -> Result<String> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing query"))?;
    let max = args
        .get("max_results")
        .and_then(|v| v.as_u64())
        .unwrap_or(30)
        .clamp(1, 100) as usize;
    let hits = ctx.backend.search_code(query, max)?;
    if hits.is_empty() {
        return Ok("No matches.".into());
    }
    let mut out = String::new();
    for hit in hits {
        let safe = redact_secrets(&hit.text).trim_end().to_string();
        out.push_str(&format!("{}:{}: {}\n", hit.path, hit.line, safe));
    }
    Ok(out)
}

fn tool_project_structure(ctx: &ToolContext<'_>, args: &Value) -> Result<String> {
    let max_depth = args
        .get("max_depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(3)
        .clamp(1, 6) as usize;
    let max_entries = args
        .get("max_entries")
        .and_then(|v| v.as_u64())
        .unwrap_or(200)
        .clamp(10, 500) as usize;

    let mut lines = Vec::new();
    lines.push(format!("{}/", ctx.backend.root().display()));
    let mut count = 0usize;
    for entry in WalkDir::new(ctx.backend.root())
        .max_depth(max_depth)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path == ctx.backend.root() {
            continue;
        }
        if should_skip_search_path(path) {
            continue;
        }
        let rel = path
            .strip_prefix(ctx.backend.root())
            .unwrap_or(path)
            .to_string_lossy();
        let depth = rel.chars().filter(|c| *c == '/' || *c == '\\').count();
        let indent = "  ".repeat(depth);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| rel.into_owned());
        if entry.file_type().is_dir() {
            lines.push(format!("{indent}{name}/"));
        } else {
            lines.push(format!("{indent}{name}"));
        }
        count += 1;
        if count >= max_entries {
            lines.push("… (truncated)".into());
            break;
        }
    }
    Ok(lines.join("\n"))
}
