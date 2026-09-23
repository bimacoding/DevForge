//! Read / write OpenSSH `~/.ssh/config` Host entries (Cursor-style).

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use devforge_core::directory::Directory;

use crate::workspace::SshHost;

/// Path to the user OpenSSH config: `~/.ssh/config`.
pub fn ssh_config_path() -> Result<PathBuf> {
    let home = Directory::home_dir()
        .ok_or_else(|| anyhow!("cannot resolve home directory"))?;
    Ok(home.join(".ssh").join("config"))
}

/// Ensure `~/.ssh` and `~/.ssh/config` exist.
pub fn ensure_ssh_config_exists() -> Result<PathBuf> {
    let path = ssh_config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    if !path.exists() {
        fs::write(&path, "# DevForge / OpenSSH user config\n")?;
    }
    Ok(path)
}

/// Load concrete Host entries from `~/.ssh/config` (skips wildcards).
/// Follows `Include` directives (relative to the including file / `~/.ssh`).
pub fn read_ssh_config_hosts() -> Result<Vec<SshHost>> {
    let path = ensure_ssh_config_exists()?;
    let mut visited = HashSet::new();
    let mut hosts = Vec::new();
    collect_hosts_from_file(&path, &mut visited, &mut hosts, 0)?;
    dedupe_and_sort_hosts(&mut hosts);
    Ok(hosts)
}

/// Append a Host block to `~/.ssh/config` and return the config path.
pub fn append_host_block(block: &str) -> Result<PathBuf> {
    let path = ensure_ssh_config_exists()?;
    let mut existing = fs::read_to_string(&path).unwrap_or_default();
    if !existing.is_empty() && !existing.ends_with('\n') {
        existing.push('\n');
    }
    if !existing.ends_with("\n\n") && !existing.is_empty() {
        existing.push('\n');
    }
    existing.push_str(block.trim_end());
    existing.push('\n');
    fs::write(&path, existing)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

const MAX_INCLUDE_DEPTH: usize = 8;

fn collect_hosts_from_file(
    path: &Path,
    visited: &mut HashSet<PathBuf>,
    hosts: &mut Vec<SshHost>,
    depth: usize,
) -> Result<()> {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !visited.insert(canon) {
        return Ok(());
    }
    if depth > MAX_INCLUDE_DEPTH {
        return Ok(());
    }

    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let base_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    parse_ssh_config_into(&text, &base_dir, visited, hosts, depth)
}

/// Parse Host blocks, skipping wildcards and unknown leading directives.
pub fn parse_ssh_config_hosts_lenient(text: &str) -> Result<Vec<SshHost>> {
    let mut hosts = Vec::new();
    let mut visited = HashSet::new();
    let base = Directory::home_dir()
        .map(|h| h.join(".ssh"))
        .unwrap_or_else(|| PathBuf::from("."));
    parse_ssh_config_into(text, &base, &mut visited, &mut hosts, 0)?;
    dedupe_and_sort_hosts(&mut hosts);
    Ok(hosts)
}

fn parse_ssh_config_into(
    text: &str,
    base_dir: &Path,
    visited: &mut HashSet<PathBuf>,
    hosts: &mut Vec<SshHost>,
    depth: usize,
) -> Result<()> {
    let mut current: Option<HostBuilder> = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (key, value) = split_key_value(line);
        let key_lower = key.to_ascii_lowercase();

        if key_lower == "include" {
            if let Some(builder) = current.take() {
                if let Ok(host) = builder.build() {
                    hosts.push(host);
                }
            }
            for pattern in value.split_whitespace() {
                for included in expand_include_pattern(pattern, base_dir) {
                    let _ = collect_hosts_from_file(
                        &included,
                        visited,
                        hosts,
                        depth + 1,
                    );
                }
            }
            continue;
        }

        if key_lower == "host" {
            if let Some(builder) = current.take() {
                if let Ok(host) = builder.build() {
                    hosts.push(host);
                }
            }

            // OpenSSH allows multiple patterns; take the first concrete name.
            let alias = value
                .split_whitespace()
                .find(|p| !p.contains('*') && !p.contains('?') && !p.is_empty())
                .map(str::to_string);

            if let Some(alias) = alias {
                current = Some(HostBuilder {
                    alias,
                    hostname: None,
                    user: None,
                    port: None,
                    identity_file: None,
                    identities_only: None,
                    server_alive_interval: None,
                    server_alive_count_max: None,
                });
            } else {
                current = None; // wildcard-only Host block
            }
            continue;
        }

        // Match / other top-level directives end the current Host block
        // (OpenSSH does not nest Host under Match).
        if key_lower == "match" {
            if let Some(builder) = current.take() {
                if let Ok(host) = builder.build() {
                    hosts.push(host);
                }
            }
            continue;
        }

        let Some(builder) = current.as_mut() else {
            continue;
        };

        match key_lower.as_str() {
            "hostname" => builder.hostname = Some(value.trim().to_string()),
            "user" => builder.user = Some(value.trim().to_string()),
            "port" => {
                if let Ok(port) = value.trim().parse() {
                    builder.port = Some(port);
                }
            }
            "identityfile" => {
                builder.identity_file = Some(value.trim().to_string());
            }
            "identitiesonly" => {
                if let Ok(v) = parse_yes_no(value) {
                    builder.identities_only = Some(v);
                }
            }
            "serveraliveinterval" => {
                if let Ok(v) = value.trim().parse() {
                    builder.server_alive_interval = Some(v);
                }
            }
            "serveralivecountmax" => {
                if let Ok(v) = value.trim().parse() {
                    builder.server_alive_count_max = Some(v);
                }
            }
            _ => {}
        }
    }

    if let Some(builder) = current.take() {
        if let Ok(host) = builder.build() {
            hosts.push(host);
        }
    }

    Ok(())
}

fn expand_include_pattern(pattern: &str, base_dir: &Path) -> Vec<PathBuf> {
    let expanded = expand_tilde(pattern);
    let path = if Path::new(&expanded).is_absolute() {
        PathBuf::from(&expanded)
    } else {
        base_dir.join(&expanded)
    };

    let path_str = path.to_string_lossy();
    if !(path_str.contains('*') || path_str.contains('?')) {
        return if path.is_file() {
            vec![path]
        } else {
            Vec::new()
        };
    }

    let Some(parent) = path.parent() else {
        return Vec::new();
    };
    let Some(file_pat) = path.file_name().and_then(|s| s.to_str()) else {
        return Vec::new();
    };
    let Ok(glob) = globset::Glob::new(file_pat) else {
        return Vec::new();
    };
    let matcher = glob.compile_matcher();
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() {
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                if matcher.is_match(name) {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = Directory::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    } else if path == "~" {
        if let Some(home) = Directory::home_dir() {
            return home.to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

fn dedupe_and_sort_hosts(hosts: &mut Vec<SshHost>) {
    let mut seen = HashSet::new();
    hosts.retain(|h| {
        let key = h.alias.clone().unwrap_or_else(|| {
            format!("{}|{}", h.user_host(), h.port.unwrap_or(22))
        });
        seen.insert(key)
    });
    hosts.sort_by(|a, b| {
        a.display_name()
            .to_ascii_lowercase()
            .cmp(&b.display_name().to_ascii_lowercase())
    });
}

/// Strict parse used when validating a pasted/new Host block.
pub fn parse_ssh_config_hosts(text: &str) -> Result<Vec<SshHost>> {
    let hosts = parse_ssh_config_hosts_lenient(text)?;
    if hosts.is_empty() {
        anyhow::bail!("No Host entries found");
    }
    Ok(hosts)
}

fn split_key_value(line: &str) -> (&str, &str) {
    if let Some((k, v)) = line.split_once('=') {
        (k.trim(), v.trim())
    } else if let Some((k, v)) = line.split_once(char::is_whitespace) {
        (k.trim(), v.trim())
    } else {
        (line, "")
    }
}

fn parse_yes_no(value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "1" => Ok(true),
        "no" | "false" | "0" => Ok(false),
        other => anyhow::bail!("Expected yes/no, got `{other}`"),
    }
}

struct HostBuilder {
    alias: String,
    hostname: Option<String>,
    user: Option<String>,
    port: Option<usize>,
    identity_file: Option<String>,
    identities_only: Option<bool>,
    server_alive_interval: Option<u64>,
    server_alive_count_max: Option<u64>,
}

impl HostBuilder {
    fn build(self) -> Result<SshHost> {
        let host = self.hostname.clone().unwrap_or_else(|| self.alias.clone());
        if host.is_empty() {
            anyhow::bail!("Host `{}` needs a HostName", self.alias);
        }
        Ok(SshHost {
            user: self.user,
            host,
            port: self.port,
            alias: Some(self.alias),
            identity_file: self.identity_file,
            identities_only: self.identities_only,
            server_alive_interval: self.server_alive_interval,
            server_alive_count_max: self.server_alive_count_max,
        })
    }
}

pub fn ssh_host_template() -> &'static str {
    "Host my-server\n  HostName 192.168.1.10\n  User ubuntu\n  Port 22\n  # IdentityFile ~/pem/key.pem\n  # IdentitiesOnly yes\n  # ServerAliveInterval 60\n  # ServerAliveCountMax 3\n"
}

pub fn ssh_config_display_path(path: &Path) -> String {
    if let Some(home) = Directory::home_dir() {
        if let Ok(rel) = path.strip_prefix(&home) {
            return format!("~/{}", rel.display());
        }
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiple_hosts() {
        let text = r#"
Host t23-home
  HostName 192.168.5.10
  User ubuntu
  Port 22

Host PQchatAzurx
  HostName 20.6.48.10
  User azureuser
  IdentityFile ~/pem/-prod.pem
  IdentitiesOnly yes
  ServerAliveInterval 60
  ServerAliveCountMax 3
"#;
        let hosts = parse_ssh_config_hosts(text).unwrap();
        assert_eq!(hosts.len(), 2);
        // Sorted alphabetically by display name
        assert_eq!(hosts[0].alias.as_deref(), Some("PQchatAzurx"));
        assert_eq!(hosts[1].alias.as_deref(), Some("t23-home"));
        assert_eq!(hosts[0].identity_file.as_deref(), Some("~/pem/-prod.pem"));
    }

    #[test]
    fn skips_wildcards() {
        let text = r#"
Host *
  ServerAliveInterval 30

Host grapheneos
  HostName 172.16.0.5
  User ubuntu-pc
"#;
        let hosts = parse_ssh_config_hosts_lenient(text).unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias.as_deref(), Some("grapheneos"));
    }

    #[test]
    fn parses_real_style_config() {
        let text = r#"
Host *
  Compression yes

Host freeBSD
  HostName 192.168.88.252
  User root
  Port 22

Host openclaw
  HostName 192.168.88.253
  User vta
  Port 22
"#;
        let hosts = parse_ssh_config_hosts_lenient(text).unwrap();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].alias.as_deref(), Some("freeBSD"));
        assert_eq!(hosts[1].alias.as_deref(), Some("openclaw"));
    }
}
