use std::{
    path::{Component, Path, PathBuf},
    sync::LazyLock,
};

use regex::Regex;

static SECRET_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(api[_-]?key|password|passwd|secret|token|private[_-]?key)\s*[=:]\s*\S+",
    )
    .expect("secret regex")
});

/// Paths that Ask/Edit tools must not read by default.
pub fn is_sensitive_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    if lower == ".env" || lower.starts_with(".env.") {
        return true;
    }
    if lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower == "id_rsa"
        || lower == "id_ed25519"
        || lower == "credentials.json"
        || lower == "secrets.json"
    {
        return true;
    }
    false
}

pub fn redact_secrets(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    for line in content.lines() {
        if SECRET_LINE.is_match(line) {
            if let Some((prefix, _)) = line.split_once('=') {
                out.push_str(prefix.trim_end());
                out.push_str("=********\n");
                continue;
            }
            if let Some((prefix, _)) = line.split_once(':') {
                out.push_str(prefix.trim_end());
                out.push_str(": ********\n");
                continue;
            }
            out.push_str("********\n");
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Resolve `relative` under `root`, rejecting path traversal and absolute escapes.
pub fn resolve_under_root(root: &Path, relative: &str) -> anyhow::Result<PathBuf> {
    let relative = Path::new(relative);
    if relative.is_absolute() {
        anyhow::bail!("absolute paths are not allowed");
    }

    let mut normalized = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    anyhow::bail!("path escapes workspace root");
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                anyhow::bail!("absolute paths are not allowed");
            }
        }
    }

    let joined = root.join(&normalized);
    let canonical_root = root
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("cannot resolve workspace root: {e}"))?;

    // Prefer real path when the target exists; otherwise keep logical join under root.
    let candidate = match joined.canonicalize() {
        Ok(path) => path,
        Err(_) => {
            // Ensure parent chain stays under root when possible.
            if let Some(parent) = joined.parent() {
                if parent.exists() {
                    let parent_canon = parent.canonicalize().map_err(|e| {
                        anyhow::anyhow!("cannot resolve parent path: {e}")
                    })?;
                    if !parent_canon.starts_with(&canonical_root) {
                        anyhow::bail!("path escapes workspace root");
                    }
                    return Ok(parent_canon.join(
                        joined
                            .file_name()
                            .ok_or_else(|| anyhow::anyhow!("invalid path"))?,
                    ));
                }
            }
            joined
        }
    };

    if !candidate.starts_with(&canonical_root) {
        anyhow::bail!("path escapes workspace root");
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn redacts_api_key_line() {
        let s = redact_secrets("OPENAI_API_KEY=sk-secret\nhello\n");
        assert!(s.contains("=********"));
        assert!(!s.contains("sk-secret"));
    }

    #[test]
    fn detects_env_file() {
        assert!(is_sensitive_path(Path::new(".env")));
        assert!(is_sensitive_path(Path::new(".env.local")));
        assert!(!is_sensitive_path(Path::new("main.rs")));
    }

    #[test]
    fn rejects_traversal_with_missing_components() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        assert!(resolve_under_root(root, "missing/../../etc/passwd").is_err());
        assert!(resolve_under_root(root, "../outside").is_err());
        assert!(resolve_under_root(root, "/etc/passwd").is_err());
        let ok = resolve_under_root(root, "src/main.rs").unwrap();
        assert!(ok.starts_with(root.canonicalize().unwrap()));
    }
}
