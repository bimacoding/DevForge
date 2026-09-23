//! Agent assets: Skills, MCPs, Subagents, Rules, Commands and Hooks.
//!
//! Assets are discovered from DevForge directories (`~/.devforge`, `<root>/.devforge`)
//! and, for compatibility, from Cursor directories (`~/.cursor`, `<root>/.cursor`).
//! Markdown assets carry optional YAML-style frontmatter with `name`, `description`
//! and `enabled`; hooks are JSON (`hooks.json`).

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use walkdir::WalkDir;

use crate::secrets::is_sensitive_path;

/// The six asset families surfaced in the Agent Assets page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AssetKind {
    Skills,
    Mcps,
    Subagents,
    Rules,
    Commands,
    Hooks,
}

impl AssetKind {
    /// Display order used by the settings tabs (matches Cursor).
    pub const ALL: [AssetKind; 6] = [
        AssetKind::Mcps,
        AssetKind::Skills,
        AssetKind::Subagents,
        AssetKind::Rules,
        AssetKind::Commands,
        AssetKind::Hooks,
    ];

    /// Stable identifier used for filters and config keys.
    pub fn id(self) -> &'static str {
        match self {
            AssetKind::Skills => "skills",
            AssetKind::Mcps => "mcps",
            AssetKind::Subagents => "subagents",
            AssetKind::Rules => "rules",
            AssetKind::Commands => "commands",
            AssetKind::Hooks => "hooks",
        }
    }

    /// Human-readable tab label.
    pub fn label(self) -> &'static str {
        match self {
            AssetKind::Skills => "Skills",
            AssetKind::Mcps => "MCPs",
            AssetKind::Subagents => "Subagents",
            AssetKind::Rules => "Rules",
            AssetKind::Commands => "Commands",
            AssetKind::Hooks => "Hooks",
        }
    }

    /// Directory name under a DevForge/Cursor config root.
    pub fn dir_name(self) -> &'static str {
        match self {
            AssetKind::Skills => "skills",
            AssetKind::Mcps => "mcp",
            AssetKind::Subagents => "agents",
            AssetKind::Rules => "rules",
            AssetKind::Commands => "commands",
            AssetKind::Hooks => "hooks",
        }
    }

    /// File extension expected for imported assets.
    pub fn extension(self) -> &'static str {
        match self {
            AssetKind::Hooks | AssetKind::Mcps => "json",
            _ => "md",
        }
    }

    /// Whether the asset body is injected into the agent system prompt.
    pub fn is_prompt_asset(self) -> bool {
        matches!(
            self,
            AssetKind::Skills | AssetKind::Subagents | AssetKind::Rules
        )
    }

    /// Parse a stable id back into a kind.
    pub fn from_id(id: &str) -> Option<Self> {
        let id = id.trim().to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|kind| kind.id() == id || kind.dir_name() == id)
    }
}

/// Whether an asset lives in the user profile or the opened project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AssetScope {
    User,
    Project,
}

impl AssetScope {
    pub fn id(self) -> &'static str {
        match self {
            AssetScope::User => "user",
            AssetScope::Project => "project",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AssetScope::User => "User",
            AssetScope::Project => "Project",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "user" => Some(AssetScope::User),
            "project" => Some(AssetScope::Project),
            _ => None,
        }
    }
}

/// A directory that may contain assets of one kind and scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetLocation {
    pub kind: AssetKind,
    pub scope: AssetScope,
    pub dir: PathBuf,
}

impl AssetLocation {
    pub fn new(kind: AssetKind, scope: AssetScope, dir: PathBuf) -> Self {
        Self { kind, scope, dir }
    }
}

/// A discovered asset on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentAsset {
    pub kind: AssetKind,
    pub scope: AssetScope,
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub body: String,
    pub enabled: bool,
}

impl AgentAsset {
    /// One-line subtitle for the asset list.
    pub fn subtitle(&self) -> String {
        if !self.description.trim().is_empty() {
            return self.description.trim().to_string();
        }
        match self.kind {
            AssetKind::Mcps => "MCP server".to_string(),
            AssetKind::Hooks => "Hook bundle".to_string(),
            _ => format!("{} asset", self.scope.label().to_ascii_lowercase()),
        }
    }
}

/// Config roots scanned for assets, in priority order (project first).
pub fn config_roots(workspace_root: Option<&Path>) -> Vec<(AssetScope, PathBuf)> {
    let mut roots = Vec::new();
    if let Some(root) = workspace_root {
        roots.push((AssetScope::Project, root.join(".devforge")));
        roots.push((AssetScope::Project, root.join(".cursor")));
    }
    if let Some(home) = dirs::home_dir() {
        roots.push((AssetScope::User, home.join(".devforge")));
        roots.push((AssetScope::User, home.join(".cursor")));
    }
    roots
}

/// Every directory that can hold a given kind of asset.
pub fn asset_locations(
    kind: AssetKind,
    workspace_root: Option<&Path>,
) -> Vec<AssetLocation> {
    let mut out = Vec::new();
    for (scope, root) in config_roots(workspace_root) {
        let dir = if kind == AssetKind::Hooks {
            root
        } else {
            root.join(kind.dir_name())
        };
        if out.iter().any(|l: &AssetLocation| l.dir == dir) {
            continue;
        }
        out.push(AssetLocation::new(kind, scope, dir));
    }
    out
}

/// Preferred write location for a new asset of the given kind/scope.
pub fn default_location(
    kind: AssetKind,
    scope: AssetScope,
    workspace_root: Option<&Path>,
) -> Result<PathBuf> {
    let base = match scope {
        AssetScope::Project => {
            workspace_root.map(|r| r.join(".devforge")).ok_or_else(|| {
                anyhow!("open a folder workspace to add project assets")
            })?
        }
        AssetScope::User => dirs::home_dir()
            .map(|h| h.join(".devforge"))
            .ok_or_else(|| anyhow!("cannot resolve the user home directory"))?,
    };
    if kind == AssetKind::Hooks {
        Ok(base)
    } else {
        Ok(base.join(kind.dir_name()))
    }
}

/// Split YAML-ish frontmatter from the markdown body.
///
/// Supports `key: value` and folded block scalars (`key: >-`), which is what
/// Cursor-style `SKILL.md` files use for multi-line descriptions.
pub fn parse_frontmatter(text: &str) -> (HashMap<String, String>, String) {
    let mut meta = HashMap::new();
    let normalized = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = normalized.strip_prefix("---") else {
        return (meta, normalized.to_string());
    };
    let rest = rest.strip_prefix('\n').unwrap_or(rest);

    let mut body_start = None;
    let lines = rest.lines().enumerate();
    let mut current_key: Option<String> = None;
    let mut block_indent: Option<usize> = None;

    for (idx, line) in lines {
        if line.trim_end() == "---" {
            body_start = Some(idx + 1);
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if let Some(key) = current_key.clone() {
            if let Some(base) = block_indent {
                if indent > base {
                    let entry = meta.entry(key).or_insert_with(String::new);
                    if !entry.is_empty() {
                        entry.push(' ');
                    }
                    entry.push_str(trimmed);
                    continue;
                }
                block_indent = None;
                current_key = None;
            }
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        if value == ">-" || value == ">" || value == "|" || value == "|-" {
            meta.entry(key.clone()).or_default();
            current_key = Some(key);
            block_indent = Some(indent);
        } else {
            meta.insert(key, unquote(value));
            current_key = None;
        }
    }

    let body = match body_start {
        Some(skip) => rest
            .splitn(skip + 1, '\n')
            .nth(skip)
            .unwrap_or_default()
            .to_string(),
        None => rest.to_string(),
    };
    (meta, body)
}

fn unquote(value: &str) -> String {
    let v = value.trim();
    if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
        || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
    {
        return v[1..v.len() - 1].to_string();
    }
    v.to_string()
}

fn parse_bool(value: &str, default: bool) -> bool {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => true,
        "false" | "no" | "off" | "0" => false,
        _ => default,
    }
}

/// Read one markdown asset (skill/subagent/rule/command) from disk.
pub fn read_markdown_asset(
    kind: AssetKind,
    scope: AssetScope,
    path: &Path,
) -> Result<AgentAsset> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    let (meta, body) = parse_frontmatter(&text);
    let name = meta
        .get("name")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            // `skills/<name>/SKILL.md` falls back to the folder name.
            let stem = path.file_stem().map(|s| s.to_string_lossy().to_string());
            if stem
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("skill"))
            {
                path.parent()
                    .and_then(|p| p.file_name())
                    .map(|s| s.to_string_lossy().to_string())
            } else {
                stem
            }
        })
        .unwrap_or_else(|| "unnamed".to_string());
    let description = meta.get("description").cloned().unwrap_or_default();
    let enabled = meta
        .get("enabled")
        .map(|v| parse_bool(v, true))
        .unwrap_or(true);
    Ok(AgentAsset {
        kind,
        scope,
        name,
        description,
        path: path.to_path_buf(),
        body: body.trim().to_string(),
        enabled,
    })
}

/// Read one JSON asset (hooks bundle or MCP descriptor).
pub fn read_json_asset(
    kind: AssetKind,
    scope: AssetScope,
    path: &Path,
) -> Result<AgentAsset> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    let parsed: Value = serde_json::from_str(&text)
        .with_context(|| format!("parse JSON {}", path.display()))?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());
    let description = match kind {
        AssetKind::Hooks => {
            let count = parsed
                .get("hooks")
                .and_then(|h| h.as_object())
                .map(|h| h.len())
                .unwrap_or(0);
            match count {
                0 => "No hook events".to_string(),
                1 => "1 hook event".to_string(),
                n => format!("{n} hook events"),
            }
        }
        AssetKind::Mcps => parsed
            .get("command")
            .and_then(|c| c.as_str())
            .map(|c| format!("stdio · {c}"))
            .unwrap_or_else(|| "MCP server".to_string()),
        _ => String::new(),
    };
    let enabled = parsed
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    Ok(AgentAsset {
        kind,
        scope,
        name,
        description,
        path: path.to_path_buf(),
        body: text,
        enabled,
    })
}

/// Discover every asset of one kind from the known locations.
pub fn discover_kind(
    kind: AssetKind,
    workspace_root: Option<&Path>,
) -> Vec<AgentAsset> {
    let mut out = Vec::new();
    let mut seen_names = Vec::new();
    for location in asset_locations(kind, workspace_root) {
        let Ok(entries) = fs::read_dir(&location.dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case(kind.extension()))
            })
            .collect();
        // Skills may also use the folder layout `skills/<name>/SKILL.md`.
        if kind == AssetKind::Skills {
            if let Ok(entries) = fs::read_dir(&location.dir) {
                for dir in entries.flatten().map(|e| e.path()) {
                    if !dir.is_dir() {
                        continue;
                    }
                    for candidate in ["SKILL.md", "skill.md"] {
                        let nested = dir.join(candidate);
                        if nested.is_file() {
                            paths.push(nested);
                            break;
                        }
                    }
                }
            }
        }
        paths.sort();
        for path in paths {
            if is_sensitive_path(&path) {
                continue;
            }
            let asset = if kind.extension() == "json" {
                read_json_asset(kind, location.scope, &path)
            } else {
                read_markdown_asset(kind, location.scope, &path)
            };
            let Ok(asset) = asset else {
                continue;
            };
            // Project scope wins over user scope for the same name.
            if seen_names.contains(&asset.name) {
                continue;
            }
            seen_names.push(asset.name.clone());
            out.push(asset);
        }
    }
    out.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });
    out
}

/// Discover every asset across all kinds.
pub fn discover_all(workspace_root: Option<&Path>) -> Vec<AgentAsset> {
    let mut out = Vec::new();
    for kind in AssetKind::ALL {
        out.extend(discover_kind(kind, workspace_root));
    }
    out
}

/// Starting content for a brand-new asset created from the UI.
pub fn asset_template(kind: AssetKind, name: &str) -> String {
    let name = name.trim();
    match kind {
        AssetKind::Skills => format!(
            "---\nname: {name}\ndescription: >-\n  Describe when this skill should be used.\n---\n\n# {name}\n\nWrite the guidance the agent should follow here.\n"
        ),
        AssetKind::Subagents => format!(
            "---\nname: {name}\ndescription: >-\n  Describe when the agent should delegate to this subagent.\n---\n\nYou are a focused assistant. Explain your role and how you report back.\n"
        ),
        AssetKind::Rules => format!(
            "---\nname: {name}\ndescription: Always applied project rule\nenabled: true\n---\n\n- Rule one.\n- Rule two.\n"
        ),
        AssetKind::Commands => format!(
            "---\nname: {name}\ndescription: What this command does\n---\n\nRun the following workflow and report the results:\n\n1. First step.\n2. Second step.\n"
        ),
        AssetKind::Hooks => "{\n  \"version\": 1,\n  \"hooks\": {}\n}\n".to_string(),
        AssetKind::Mcps => "{\n  \"name\": \"my-server\",\n  \"command\": \"npx\",\n  \"args\": [\"-y\", \"@modelcontextprotocol/server-filesystem\", \".\"],\n  \"enabled\": true\n}\n".to_string(),
    }
}

/// Short, copy-pasteable documentation snippet for the asset kind.
pub fn asset_doc_hint(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Skills => {
            "A skill is `skills/<name>/SKILL.md` (or `skills/<name>.md`) with `name` and `description` frontmatter. Loaded into the agent prompt when relevant."
        }
        AssetKind::Mcps => {
            "An MCP entry is `{\"name\", \"command\", \"args\", \"enabled\"}`. DevForge also reads `[[ai.mcp-servers]]` from settings.toml."
        }
        AssetKind::Subagents => {
            "A subagent is `agents/<name>.md` with `name` and `description` frontmatter; the body becomes its system prompt."
        }
        AssetKind::Rules => {
            "A rule is `rules/<name>.md` with `enabled: true`; every enabled rule is appended to the agent prompt."
        }
        AssetKind::Commands => {
            "A command is `commands/<name>.md`; its body describes a reusable workflow you can trigger from chat."
        }
        AssetKind::Hooks => {
            "Hooks live in `hooks.json` as `{\"version\": 1, \"hooks\": {\"afterFileEdit\": [...]}}`."
        }
    }
}

/// Validate an asset name so it is safe to use as a file stem.
pub fn sanitize_asset_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("name cannot be empty"));
    }
    let cleaned: String = trimmed
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('-').to_string();
    if cleaned.is_empty() {
        return Err(anyhow!("name must contain letters or digits"));
    }
    if cleaned.len() > 64 {
        return Err(anyhow!("name must be 64 characters or fewer"));
    }
    Ok(cleaned)
}

/// Create a new asset file from the built-in template.
pub fn create_asset(
    kind: AssetKind,
    scope: AssetScope,
    workspace_root: Option<&Path>,
    name: &str,
) -> Result<PathBuf> {
    let safe = sanitize_asset_name(name)?;
    let dir = default_location(kind, scope, workspace_root)?;
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let file_name = if kind == AssetKind::Hooks {
        "hooks.json".to_string()
    } else {
        format!("{safe}.{}", kind.extension())
    };
    let path = dir.join(file_name);
    if path.exists() {
        return Err(anyhow!("`{}` already exists", path.display()));
    }
    let contents = asset_template(kind, &safe);
    fs::write(&path, contents)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

/// Copy an external file into the assets directory (the "import/upload" action).
pub fn import_asset(
    kind: AssetKind,
    scope: AssetScope,
    workspace_root: Option<&Path>,
    source: &Path,
) -> Result<PathBuf> {
    if !source.is_file() {
        return Err(anyhow!("`{}` is not a file", source.display()));
    }
    if is_sensitive_path(source) {
        return Err(anyhow!("refusing to import a sensitive file"));
    }
    let expected = kind.extension();
    let actual = source
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(anyhow!(
            "{} assets must be .{expected} files (got .{actual})",
            kind.label()
        ));
    }

    let dir = default_location(kind, scope, workspace_root)?;
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;

    let stem = preferred_stem(source);
    let safe = sanitize_asset_name(&stem)?;
    let dest = dir.join(format!("{safe}.{expected}"));
    if dest.exists() {
        return Err(anyhow!("`{}` already exists", dest.display()));
    }
    fs::copy(source, &dest).with_context(|| {
        format!("copy {} -> {}", source.display(), dest.display())
    })?;
    Ok(dest)
}

/// Best name to use for an imported file.
///
/// `skills/<name>/SKILL.md` should import as `<name>`, not as `skill`.
fn preferred_stem(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "imported".to_string());
    if stem.eq_ignore_ascii_case("skill") {
        if let Some(parent) = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
        {
            return parent;
        }
    }
    stem
}

/// Import a folder of assets (recursive), returning the created files.
pub fn import_directory(
    kind: AssetKind,
    scope: AssetScope,
    workspace_root: Option<&Path>,
    source_dir: &Path,
) -> Result<Vec<PathBuf>> {
    if !source_dir.is_dir() {
        return Err(anyhow!("`{}` is not a directory", source_dir.display()));
    }
    let expected = kind.extension();
    let mut created = Vec::new();
    for entry in WalkDir::new(source_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let matches = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case(expected));
        if !matches {
            continue;
        }
        if let Ok(dest) = import_asset(kind, scope, workspace_root, path) {
            created.push(dest);
        }
    }
    if created.is_empty() {
        return Err(anyhow!(
            "no .{expected} files found in `{}`",
            source_dir.display()
        ));
    }
    Ok(created)
}

/// Remove an asset file from disk.
pub fn delete_asset(path: &Path) -> Result<()> {
    if !path.is_file() {
        return Err(anyhow!("`{}` is not a file", path.display()));
    }
    fs::remove_file(path).with_context(|| format!("remove {}", path.display()))
}

/// Read an asset from disk, choosing the parser from the extension.
pub fn read_asset(
    kind: AssetKind,
    scope: AssetScope,
    path: &Path,
) -> Result<AgentAsset> {
    if kind.extension() == "json" {
        read_json_asset(kind, scope, path)
    } else {
        read_markdown_asset(kind, scope, path)
    }
}

/// Toggle the `enabled` flag of a markdown asset in place.
///
/// Rewrites only the frontmatter `enabled:` line (inserting one when missing)
/// so the rest of the file — including comments and formatting — is preserved.
pub fn set_asset_enabled(path: &Path, enabled: bool) -> Result<()> {
    if !path.is_file() {
        return Err(anyhow!("`{}` is not a file", path.display()));
    }
    let text = fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    let value = if enabled { "true" } else { "false" };
    let updated = if let Some(rest) = text.strip_prefix("---\n") {
        let mut out = String::from("---\n");
        let mut replaced = false;
        let mut closed = false;
        for line in rest.split_inclusive('\n') {
            if !closed {
                let trimmed = line.trim_end();
                if trimmed == "---" {
                    if !replaced {
                        out.push_str(&format!("enabled: {value}\n"));
                        replaced = true;
                    }
                    closed = true;
                    out.push_str(line);
                    continue;
                }
                if trimmed
                    .split_once(':')
                    .is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case("enabled"))
                {
                    if !replaced {
                        out.push_str(&format!("enabled: {value}\n"));
                        replaced = true;
                    }
                    continue;
                }
            }
            out.push_str(line);
        }
        out
    } else {
        // No frontmatter: add a minimal one so the flag can be persisted.
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "asset".to_string());
        format!("---\nname: {name}\nenabled: {value}\n---\n\n{text}")
    };
    fs::write(path, updated).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Build the system-prompt section for enabled prompt assets.
///
/// Skills, subagents and rules are injected; commands and hooks are runtime
/// concepts and are intentionally excluded.
pub fn build_prompt(assets: &[AgentAsset]) -> String {
    let mut skills = Vec::new();
    let mut rules = Vec::new();
    let mut agents = Vec::new();
    for asset in assets.iter().filter(|a| a.enabled) {
        let entry = format!("### {}\n{}", asset.name, asset.body.trim());
        match asset.kind {
            AssetKind::Skills => skills.push(entry),
            AssetKind::Rules => rules.push(entry),
            AssetKind::Subagents => agents.push(entry),
            _ => {}
        }
    }

    let mut out = String::new();
    if !skills.is_empty() {
        out.push_str(
            "\n\n# Agent Skills\nApply these skills when their description matches the task:\n\n",
        );
        out.push_str(&skills.join("\n\n"));
    }
    if !agents.is_empty() {
        out.push_str(
            "\n\n# Available Subagents\nDelegate to these when their description matches:\n\n",
        );
        out.push_str(&agents.join("\n\n"));
    }
    if !rules.is_empty() {
        out.push_str("\n\n# Project Rules\nAlways follow these rules:\n\n");
        out.push_str(&rules.join("\n\n"));
    }
    out
}

/// Discover and build the prompt in one step.
pub fn load_assets_prompt(workspace_root: Option<&Path>) -> String {
    let assets = discover_all(workspace_root);
    build_prompt(&assets)
}

/// Read the MCP servers declared as JSON assets (`mcp/<name>.json`).
///
/// These are merged with the `[[ai.mcp-servers]]` entries from settings so the
/// Agent Assets page can install a server without editing settings by hand.
pub fn mcp_asset_specs(
    workspace_root: Option<&Path>,
) -> Vec<(String, crate::mcp::McpServerSpec)> {
    let mut out = Vec::new();
    for asset in discover_kind(AssetKind::Mcps, workspace_root) {
        if !asset.enabled {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<Value>(&asset.body) else {
            continue;
        };
        let Some(command) = parsed.get("command").and_then(|c| c.as_str()) else {
            continue;
        };
        if command.trim().is_empty() {
            continue;
        }
        let name = parsed
            .get("name")
            .and_then(|n| n.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| asset.name.clone());
        let args = parsed
            .get("args")
            .and_then(|a| a.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        out.push((
            asset.path.display().to_string(),
            crate::mcp::McpServerSpec {
                name,
                command: command.to_string(),
                args,
            },
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn kind_ids_round_trip() {
        for kind in AssetKind::ALL {
            assert_eq!(AssetKind::from_id(kind.id()), Some(kind));
            assert_eq!(AssetKind::from_id(kind.dir_name()), Some(kind));
        }
        assert_eq!(AssetKind::from_id("nope"), None);
    }

    #[test]
    fn parse_frontmatter_reads_simple_values() {
        let text =
            "---\nname: my-skill\ndescription: Does things\n---\n\nBody here.\n";
        let (meta, body) = parse_frontmatter(text);
        assert_eq!(meta.get("name").unwrap(), "my-skill");
        assert_eq!(meta.get("description").unwrap(), "Does things");
        assert_eq!(body.trim(), "Body here.");
    }

    #[test]
    fn parse_frontmatter_folds_block_scalars() {
        let text = "---\nname: canvas\ndescription: >-\n  First line\n  second line.\n---\n\nBody.\n";
        let (meta, body) = parse_frontmatter(text);
        assert_eq!(meta.get("description").unwrap(), "First line second line.");
        assert_eq!(body.trim(), "Body.");
    }

    #[test]
    fn parse_frontmatter_without_header_keeps_body() {
        let (meta, body) = parse_frontmatter("# Just markdown\n");
        assert!(meta.is_empty());
        assert_eq!(body, "# Just markdown\n");
    }

    #[test]
    fn sanitize_asset_name_normalizes_input() {
        assert_eq!(sanitize_asset_name("My Skill!").unwrap(), "my-skill");
        assert_eq!(sanitize_asset_name("  a_b-c  ").unwrap(), "a_b-c");
        assert!(sanitize_asset_name("   ").is_err());
        assert!(sanitize_asset_name("!!").is_err());
        assert!(sanitize_asset_name(&"x".repeat(80)).is_err());
    }

    #[test]
    fn create_and_discover_skill_in_project_scope() {
        let root = temp_root();
        let path = create_asset(
            AssetKind::Skills,
            AssetScope::Project,
            Some(root.path()),
            "My Skill",
        )
        .unwrap();
        assert!(path.exists());
        assert!(path.to_string_lossy().ends_with("skills/my-skill.md"));

        let found = discover_kind(AssetKind::Skills, Some(root.path()));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "my-skill");
        assert!(found[0].enabled);
    }

    #[test]
    fn discovers_folder_style_skills() {
        let root = temp_root();
        let dir = root.path().join(".devforge/skills/canvas");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            "---\ndescription: Draws\n---\n\nBody.\n",
        )
        .unwrap();

        let found = discover_kind(AssetKind::Skills, Some(root.path()));
        assert_eq!(found.len(), 1);
        // No `name:` in frontmatter -> folder name is used.
        assert_eq!(found[0].name, "canvas");
        assert_eq!(found[0].description, "Draws");
    }

    #[test]
    fn importing_skill_md_uses_folder_name() {
        let root = temp_root();
        let src_dir = root.path().join("incoming/chart-helper");
        fs::create_dir_all(&src_dir).unwrap();
        let src = src_dir.join("SKILL.md");
        fs::write(&src, "---\ndescription: Charts\n---\n\nBody.\n").unwrap();

        let dest = import_asset(
            AssetKind::Skills,
            AssetScope::Project,
            Some(root.path()),
            &src,
        )
        .unwrap();
        assert!(dest.to_string_lossy().ends_with("skills/chart-helper.md"));
    }

    #[test]
    fn mcp_asset_specs_reads_enabled_servers() {
        let root = temp_root();
        let dir = root.path().join(".devforge/mcp");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("files.json"),
            r#"{"name":"fs","command":"npx","args":["-y","server-fs"],"enabled":true}"#,
        )
        .unwrap();
        fs::write(
            dir.join("off.json"),
            r#"{"name":"off","command":"npx","enabled":false}"#,
        )
        .unwrap();

        let specs = mcp_asset_specs(Some(root.path()));
        assert_eq!(specs.len(), 1);
        let (path, spec) = &specs[0];
        assert!(path.ends_with("mcp/files.json"));
        assert_eq!(spec.name, "fs");
        assert_eq!(spec.command, "npx");
        assert_eq!(spec.args, vec!["-y".to_string(), "server-fs".to_string()]);
    }

    #[test]
    fn build_prompt_injects_enabled_assets_only() {
        let root = temp_root();
        let dir = root.path().join(".devforge/rules");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("on.md"),
            "---\nname: on\ndescription: On\n---\n\nAlways be kind.\n",
        )
        .unwrap();
        fs::write(
            dir.join("off.md"),
            "---\nname: off\ndescription: Off\nenabled: false\n---\n\nNever run.\n",
        )
        .unwrap();

        let prompt = load_assets_prompt(Some(root.path()));
        assert!(prompt.contains("Always be kind."));
        assert!(!prompt.contains("Never run."));
        assert!(prompt.contains("# Project Rules"));
    }

    #[test]
    fn set_asset_enabled_preserves_body() {
        let root = temp_root();
        let dir = root.path().join(".devforge/skills");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("demo.md");
        fs::write(
            &path,
            "---\nname: demo\ndescription: Demo\n---\n\nKeep me intact.\n",
        )
        .unwrap();

        set_asset_enabled(&path, false).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("enabled: false"));
        assert!(text.contains("Keep me intact."));

        let found = discover_kind(AssetKind::Skills, Some(root.path()));
        assert_eq!(found.len(), 1);
        assert!(!found[0].enabled);
    }

    #[test]
    fn create_asset_rejects_duplicates() {
        let root = temp_root();
        create_asset(
            AssetKind::Rules,
            AssetScope::Project,
            Some(root.path()),
            "alpha",
        )
        .unwrap();
        assert!(
            create_asset(
                AssetKind::Rules,
                AssetScope::Project,
                Some(root.path()),
                "alpha"
            )
            .is_err()
        );
    }

    #[test]
    fn project_scope_is_created_under_devforge_dir() {
        let root = temp_root();
        let path = create_asset(
            AssetKind::Subagents,
            AssetScope::Project,
            Some(root.path()),
            "reviewer",
        )
        .unwrap();
        assert!(path.starts_with(root.path().join(".devforge/agents")));
    }

    #[test]
    fn project_scope_requires_a_workspace() {
        assert!(
            create_asset(AssetKind::Skills, AssetScope::Project, None, "x").is_err()
        );
    }

    #[test]
    fn import_asset_copies_matching_file() {
        let root = temp_root();
        let src = root.path().join("incoming.md");
        fs::write(&src, "---\nname: imported\n---\n\nHi.\n").unwrap();

        let dest = import_asset(
            AssetKind::Skills,
            AssetScope::Project,
            Some(root.path()),
            &src,
        )
        .unwrap();
        assert!(dest.exists());
        let found = discover_kind(AssetKind::Skills, Some(root.path()));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "imported");
    }

    #[test]
    fn import_asset_rejects_wrong_extension() {
        let root = temp_root();
        let src = root.path().join("incoming.txt");
        fs::write(&src, "hello").unwrap();
        assert!(
            import_asset(
                AssetKind::Skills,
                AssetScope::Project,
                Some(root.path()),
                &src
            )
            .is_err()
        );
    }

    #[test]
    fn import_asset_rejects_sensitive_files() {
        let root = temp_root();
        let src = root.path().join(".env");
        fs::write(&src, "SECRET=1").unwrap();
        assert!(
            import_asset(
                AssetKind::Skills,
                AssetScope::Project,
                Some(root.path()),
                &src
            )
            .is_err()
        );
    }

    #[test]
    fn import_directory_collects_markdown_files() {
        let root = temp_root();
        let src_dir = root.path().join("bundle");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("one.md"), "---\nname: one\n---\nA\n").unwrap();
        fs::write(src_dir.join("two.md"), "---\nname: two\n---\nB\n").unwrap();
        fs::write(src_dir.join("skip.txt"), "nope").unwrap();

        let created = import_directory(
            AssetKind::Rules,
            AssetScope::Project,
            Some(root.path()),
            &src_dir,
        )
        .unwrap();
        assert_eq!(created.len(), 2);
        assert_eq!(discover_kind(AssetKind::Rules, Some(root.path())).len(), 2);
    }

    #[test]
    fn import_directory_errors_without_matches() {
        let root = temp_root();
        let src_dir = root.path().join("empty-bundle");
        fs::create_dir_all(&src_dir).unwrap();
        assert!(
            import_directory(
                AssetKind::Rules,
                AssetScope::Project,
                Some(root.path()),
                &src_dir
            )
            .is_err()
        );
    }

    #[test]
    fn delete_asset_removes_the_file() {
        let root = temp_root();
        let path = create_asset(
            AssetKind::Commands,
            AssetScope::Project,
            Some(root.path()),
            "run-tests",
        )
        .unwrap();
        delete_asset(&path).unwrap();
        assert!(!path.exists());
        assert!(delete_asset(&path).is_err());
    }

    #[test]
    fn disabled_frontmatter_is_honoured() {
        let root = temp_root();
        let path = create_asset(
            AssetKind::Rules,
            AssetScope::Project,
            Some(root.path()),
            "off",
        )
        .unwrap();
        fs::write(&path, "---\nname: off\nenabled: false\n---\nBody\n").unwrap();
        let found = discover_kind(AssetKind::Rules, Some(root.path()));
        assert_eq!(found.len(), 1);
        assert!(!found[0].enabled);
    }

    #[test]
    fn build_prompt_includes_enabled_assets_only() {
        let assets = vec![
            AgentAsset {
                kind: AssetKind::Skills,
                scope: AssetScope::Project,
                name: "skill-a".into(),
                description: String::new(),
                path: PathBuf::from("a.md"),
                body: "Skill body".into(),
                enabled: true,
            },
            AgentAsset {
                kind: AssetKind::Rules,
                scope: AssetScope::Project,
                name: "rule-a".into(),
                description: String::new(),
                path: PathBuf::from("r.md"),
                body: "Rule body".into(),
                enabled: true,
            },
            AgentAsset {
                kind: AssetKind::Skills,
                scope: AssetScope::Project,
                name: "skill-off".into(),
                description: String::new(),
                path: PathBuf::from("b.md"),
                body: "Hidden".into(),
                enabled: false,
            },
            AgentAsset {
                kind: AssetKind::Hooks,
                scope: AssetScope::Project,
                name: "hooks".into(),
                description: String::new(),
                path: PathBuf::from("hooks.json"),
                body: "{}".into(),
                enabled: true,
            },
        ];
        let prompt = build_prompt(&assets);
        assert!(prompt.contains("# Agent Skills"));
        assert!(prompt.contains("Skill body"));
        assert!(prompt.contains("# Project Rules"));
        assert!(prompt.contains("Rule body"));
        assert!(!prompt.contains("Hidden"));
        assert!(!prompt.contains("hooks.json"));
    }

    #[test]
    fn build_prompt_is_empty_without_assets() {
        assert!(build_prompt(&[]).is_empty());
    }

    #[test]
    fn json_asset_reports_hook_event_count() {
        let root = temp_root();
        let dir = default_location(
            AssetKind::Hooks,
            AssetScope::Project,
            Some(root.path()),
        )
        .unwrap();
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hooks.json");
        fs::write(
            &path,
            r#"{"version":1,"hooks":{"afterFileEdit":[{"command":"fmt.sh"}]}}"#,
        )
        .unwrap();
        let asset =
            read_json_asset(AssetKind::Hooks, AssetScope::Project, &path).unwrap();
        assert_eq!(asset.description, "1 hook event");
        assert!(asset.enabled);
    }

    #[test]
    fn templates_are_non_empty_for_every_kind() {
        for kind in AssetKind::ALL {
            let tpl = asset_template(kind, "sample");
            assert!(!tpl.trim().is_empty(), "empty template for {}", kind.id());
            assert!(!asset_doc_hint(kind).is_empty());
        }
    }

    #[test]
    fn set_asset_enabled_replaces_existing_flag() {
        let root = temp_root();
        let path = root.path().join("rule.md");
        fs::write(&path, "---\nname: rule\nenabled: true\n---\n\nBody\n").unwrap();
        set_asset_enabled(&path, false).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("enabled: false"));
        assert_eq!(text.matches("enabled:").count(), 1);
        assert!(text.contains("Body"));
    }

    #[test]
    fn set_asset_enabled_inserts_missing_flag() {
        let root = temp_root();
        let path = root.path().join("skill.md");
        fs::write(&path, "---\nname: skill\n---\n\nBody\n").unwrap();
        set_asset_enabled(&path, false).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("enabled: false"));
        assert!(text.contains("Body"));
        let asset =
            read_markdown_asset(AssetKind::Skills, AssetScope::Project, &path)
                .unwrap();
        assert!(!asset.enabled);
    }

    #[test]
    fn set_asset_enabled_adds_frontmatter_when_missing() {
        let root = temp_root();
        let path = root.path().join("plain.md");
        fs::write(&path, "# Plain\n").unwrap();
        set_asset_enabled(&path, false).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("---\n"));
        assert!(text.contains("enabled: false"));
        assert!(text.contains("# Plain"));
    }

    #[test]
    fn read_asset_dispatches_on_extension() {
        let root = temp_root();
        let dir = default_location(
            AssetKind::Hooks,
            AssetScope::Project,
            Some(root.path()),
        )
        .unwrap();
        fs::create_dir_all(&dir).unwrap();
        let hooks = dir.join("hooks.json");
        fs::write(&hooks, r#"{"version":1,"hooks":{}}"#).unwrap();
        let asset =
            read_asset(AssetKind::Hooks, AssetScope::Project, &hooks).unwrap();
        assert_eq!(asset.kind, AssetKind::Hooks);
        assert_eq!(asset.description, "No hook events");
    }

    #[test]
    fn hooks_asset_always_uses_hooks_json_name() {
        let root = temp_root();
        let path = create_asset(
            AssetKind::Hooks,
            AssetScope::Project,
            Some(root.path()),
            "ignored",
        )
        .unwrap();
        assert!(path.to_string_lossy().ends_with("hooks.json"));
    }
}
