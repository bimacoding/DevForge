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
    fn write_file(&self, path: &Path, content: &str) -> Result<()>;
    fn create_directory(&self, path: &Path) -> Result<()>;
    fn list_directory(&self, path: &Path) -> Result<Vec<String>>;
    fn search_code(&self, query: &str, max_results: usize)
    -> Result<Vec<SearchHit>>;
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

    fn write_file(&self, path: &Path, content: &str) -> Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("create parent {}", parent.display())
                })?;
            }
        }
        fs::write(path, content).with_context(|| format!("write {}", path.display()))
    }

    fn create_directory(&self, path: &Path) -> Result<()> {
        fs::create_dir_all(path).with_context(|| format!("mkdir {}", path.display()))
    }

    fn list_directory(&self, path: &Path) -> Result<Vec<String>> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let suffix = if entry.file_type()?.is_dir() { "/" } else { "" };
            entries.push(format!("{name}{suffix}"));
        }
        entries.sort();
        Ok(entries)
    }

    fn search_code(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchHit>> {
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
        },
        {
            "type": "function",
            "function": {
                "name": "list_agent_assets",
                "description": "List the Skills, MCPs, Subagents, Rules, Commands and Hooks available to this agent, including their scope and source path.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "description": "Optional filter: skills, mcps, subagents, rules, commands or hooks."
                        }
                    }
                }
            }
        }
    ])
}

fn write_tools() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Create or overwrite a UTF-8 text file relative to the workspace root. Creates parent directories as needed.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "str_replace",
                "description": "Replace an exact substring in a file. Fails if old_string is missing or (unless replace_all) matches more than once.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "old_string": { "type": "string" },
                        "new_string": { "type": "string" },
                        "replace_all": { "type": "boolean" }
                    },
                    "required": ["path", "old_string", "new_string"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "create_directory",
                "description": "Create a directory (and parents) relative to the workspace root.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" }
                    },
                    "required": ["path"]
                }
            }
        }
    ])
}

/// Ask + write tools for Edit / Agent modes.
pub fn edit_tools() -> Value {
    merge_tool_arrays(ask_tools(), write_tools())
}

/// Agent mode tools (same write surface as Edit for this phase).
pub fn agent_tools() -> Value {
    edit_tools()
}

fn merge_tool_arrays(a: Value, b: Value) -> Value {
    let mut out = a.as_array().cloned().unwrap_or_default();
    if let Some(extra) = b.as_array() {
        out.extend(extra.iter().cloned());
    }
    json!(out)
}

pub fn tools_for_mode(mode: crate::mode::AgentMode) -> Value {
    match mode {
        crate::mode::AgentMode::Ask => ask_tools(),
        crate::mode::AgentMode::Edit => edit_tools(),
        crate::mode::AgentMode::Agent => agent_tools(),
    }
}

pub fn tool_permission(name: &str) -> PermissionLevel {
    match name {
        "read_file"
        | "list_directory"
        | "search_code"
        | "get_project_structure"
        | "list_agent_assets" => PermissionLevel::ReadOnly,
        "write_file" | "str_replace" | "create_directory" => {
            PermissionLevel::Confirm
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
        "get_project_structure" => {
            match tool_project_structure(ctx, &call.arguments) {
                Ok(s) => ToolResult {
                    content: s,
                    is_error: false,
                },
                Err(e) => ToolResult {
                    content: e.to_string(),
                    is_error: true,
                },
            }
        }
        "list_agent_assets" => match tool_list_agent_assets(ctx, &call.arguments) {
            Ok(s) => ToolResult {
                content: s,
                is_error: false,
            },
            Err(e) => ToolResult {
                content: e.to_string(),
                is_error: true,
            },
        },
        "write_file" => match tool_write_file(ctx, &call.arguments) {
            Ok(s) => ToolResult {
                content: s,
                is_error: false,
            },
            Err(e) => ToolResult {
                content: e.to_string(),
                is_error: true,
            },
        },
        "str_replace" => match tool_str_replace(ctx, &call.arguments) {
            Ok(s) => ToolResult {
                content: s,
                is_error: false,
            },
            Err(e) => ToolResult {
                content: e.to_string(),
                is_error: true,
            },
        },
        "create_directory" => match tool_create_directory(ctx, &call.arguments) {
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
            content: format!("unknown tool: {other}"),
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

/// List the agent assets (skills, MCPs, subagents, rules, commands, hooks)
/// visible from this workspace.
fn tool_list_agent_assets(ctx: &ToolContext<'_>, args: &Value) -> Result<String> {
    let filter = args
        .get("kind")
        .and_then(|v| v.as_str())
        .and_then(crate::assets::AssetKind::from_id);

    let kinds: Vec<crate::assets::AssetKind> = match filter {
        Some(kind) => vec![kind],
        None => crate::assets::AssetKind::ALL.to_vec(),
    };

    let mut lines = Vec::new();
    for kind in kinds {
        let assets = crate::assets::discover_kind(kind, Some(ctx.backend.root()));
        lines.push(format!("## {} ({})", kind.label(), assets.len()));
        if assets.is_empty() {
            lines.push("  (none)".to_string());
            continue;
        }
        for asset in assets {
            let state = if asset.enabled { "enabled" } else { "disabled" };
            lines.push(format!(
                "  - {} [{} · {}] {}",
                asset.name,
                asset.scope.label().to_ascii_lowercase(),
                state,
                asset.path.display()
            ));
            let description = asset.description.trim();
            if !description.is_empty() {
                lines.push(format!("      {description}"));
            }
        }
    }
    Ok(lines.join("\n"))
}

fn tool_write_file(ctx: &ToolContext<'_>, args: &Value) -> Result<String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing path"))?;
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing content"))?;
    let resolved = resolve_under_root(ctx.backend.root(), path)?;
    if is_sensitive_path(&resolved) {
        anyhow::bail!("refusing to write sensitive path: {path}");
    }
    ctx.backend.write_file(&resolved, content)?;
    Ok(format!("Wrote {} bytes to `{path}`", content.len()))
}

fn tool_str_replace(ctx: &ToolContext<'_>, args: &Value) -> Result<String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing path"))?;
    let old = args
        .get("old_string")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing old_string"))?;
    let new = args
        .get("new_string")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing new_string"))?;
    let replace_all = args
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if old.is_empty() {
        anyhow::bail!("old_string must not be empty");
    }
    let resolved = resolve_under_root(ctx.backend.root(), path)?;
    if is_sensitive_path(&resolved) {
        anyhow::bail!("refusing to edit sensitive path: {path}");
    }
    let content = ctx.backend.read_file(&resolved)?;
    let matches = content.matches(old).count();
    if matches == 0 {
        anyhow::bail!("old_string not found in `{path}`");
    }
    if matches > 1 && !replace_all {
        anyhow::bail!(
            "old_string matched {matches} times in `{path}`; set replace_all=true or provide a more unique string"
        );
    }
    let updated = if replace_all {
        content.replace(old, new)
    } else {
        content.replacen(old, new, 1)
    };
    ctx.backend.write_file(&resolved, &updated)?;
    Ok(format!(
        "Updated `{path}` ({} replacement{})",
        if replace_all { matches } else { 1 },
        if replace_all && matches != 1 { "s" } else { "" }
    ))
}

fn tool_create_directory(ctx: &ToolContext<'_>, args: &Value) -> Result<String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing path"))?;
    let resolved = resolve_under_root(ctx.backend.root(), path)?;
    if is_sensitive_path(&resolved) {
        anyhow::bail!("refusing to create sensitive path: {path}");
    }
    ctx.backend.create_directory(&resolved)?;
    Ok(format!("Created directory `{path}`"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn write_and_str_replace_roundtrip() {
        let dir = tempdir().unwrap();
        let backend = FsWorkspaceBackend::new(dir.path());
        let tool_ctx = ToolContext {
            backend: &backend,
            max_file_bytes: 200_000,
        };

        let write = execute_tool(
            &tool_ctx,
            &ToolCall {
                id: "1".into(),
                name: "write_file".into(),
                arguments: json!({
                    "path": "src/hello.rs",
                    "content": "fn main() {\n    println!(\"hi\");\n}\n"
                }),
            },
        );
        assert!(!write.is_error, "{}", write.content);

        let replace = execute_tool(
            &tool_ctx,
            &ToolCall {
                id: "2".into(),
                name: "str_replace".into(),
                arguments: json!({
                    "path": "src/hello.rs",
                    "old_string": "hi",
                    "new_string": "hello"
                }),
            },
        );
        assert!(!replace.is_error, "{}", replace.content);

        let read = execute_tool(
            &tool_ctx,
            &ToolCall {
                id: "3".into(),
                name: "read_file".into(),
                arguments: json!({ "path": "src/hello.rs" }),
            },
        );
        assert!(!read.is_error);
        assert!(read.content.contains("hello"));
    }

    #[test]
    fn ask_tools_exclude_writes() {
        let tools = ask_tools();
        let names: Vec<_> = tools
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.pointer("/function/name")?.as_str())
            .collect();
        assert!(names.contains(&"read_file"));
        assert!(!names.contains(&"write_file"));
    }

    #[test]
    fn edit_tools_include_writes() {
        let tools = edit_tools();
        let names: Vec<_> = tools
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.pointer("/function/name")?.as_str())
            .collect();
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"str_replace"));
        assert_eq!(tool_permission("write_file"), PermissionLevel::Confirm);
    }
}
