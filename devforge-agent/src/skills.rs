//! Load optional Agent Skills markdown into the system prompt.

use std::path::{Path, PathBuf};

/// Collect skill snippets from well-known directories (Cursor-style).
pub fn load_skills_prompt(workspace_root: Option<&Path>) -> String {
    let mut dirs = Vec::new();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".devforge/skills"));
        dirs.push(home.join(".cursor/skills"));
    }
    if let Some(root) = workspace_root {
        dirs.push(root.join(".devforge/skills"));
        dirs.push(root.join(".cursor/skills"));
    }

    let mut chunks = Vec::new();
    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("md"))
            })
            .collect();
        files.sort();
        for path in files {
            if let Ok(text) = std::fs::read_to_string(&path) {
                let name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "skill".into());
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    chunks.push(format!("### Skill: {name}\n{trimmed}"));
                }
            }
        }
    }

    if chunks.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n# Agent Skills\nFollow these skills when relevant:\n\n{}",
            chunks.join("\n\n")
        )
    }
}
