use std::{collections::HashMap, fmt::Display, path::PathBuf};

use devforge_core::directory::Directory;
use serde::{Deserialize, Serialize};

use crate::{debug::LapceBreakpoint, main_split::SplitInfo, panel::data::PanelInfo};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct SshHost {
    pub user: Option<String>,
    /// Connection target (HostName). Falls back to alias when HostName is omitted.
    pub host: String,
    pub port: Option<usize>,
    /// SSH config `Host` alias shown in the UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identities_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_alive_interval: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_alive_count_max: Option<u64>,
}

impl SshHost {
    pub fn from_string(s: &str) -> Self {
        let mut whole_splits = s.split(':');
        let splits = whole_splits
            .next()
            .unwrap()
            .split('@')
            .collect::<Vec<&str>>();
        let mut splits = splits.iter().rev();
        let host = splits.next().unwrap().to_string();
        let user = splits.next().map(|s| s.to_string());
        let port = whole_splits.next().and_then(|s| s.parse::<usize>().ok());
        Self {
            user,
            host,
            port,
            alias: None,
            identity_file: None,
            identities_only: None,
            server_alive_interval: None,
            server_alive_count_max: None,
        }
    }

    pub fn user_host(&self) -> String {
        if let Some(user) = self.user.as_ref() {
            format!("{user}@{}", self.host)
        } else {
            self.host.clone()
        }
    }

    pub fn display_name(&self) -> String {
        self.alias.clone().unwrap_or_else(|| self.to_string())
    }

    /// Target passed to the `ssh` CLI.
    /// Prefer the Host alias so OpenSSH applies `~/.ssh/config` fully.
    pub fn ssh_cli_target(&self) -> String {
        self.alias.clone().unwrap_or_else(|| self.user_host())
    }

    /// Expand `~` in IdentityFile for passing to OpenSSH.
    pub fn expanded_identity_file(&self) -> Option<PathBuf> {
        let path = self.identity_file.as_ref()?;
        let path = if let Some(rest) = path.strip_prefix("~/") {
            Directory::home_dir()?.join(rest)
        } else if path == "~" {
            Directory::home_dir()?
        } else {
            PathBuf::from(path)
        };
        Some(path)
    }
}

impl Display for SshHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(alias) = self.alias.as_ref() {
            if self.alias.as_deref() != Some(self.host.as_str()) {
                write!(f, "{alias} (")?;
                if let Some(user) = self.user.as_ref() {
                    write!(f, "{user}@")?;
                }
                write!(f, "{}", self.host)?;
                if let Some(port) = self.port {
                    write!(f, ":{port}")?;
                }
                write!(f, ")")?;
                return Ok(());
            }
        }
        if let Some(user) = self.user.as_ref() {
            write!(f, "{user}@")?;
        }
        write!(f, "{}", self.host)?;
        if let Some(port) = self.port {
            write!(f, ":{port}")?;
        }
        Ok(())
    }
}

#[cfg(windows)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct WslHost {
    pub host: String,
}

#[cfg(windows)]
impl Display for WslHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.host)?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LapceWorkspaceType {
    Local,
    RemoteSSH(SshHost),
    #[cfg(windows)]
    RemoteWSL(WslHost),
}

impl LapceWorkspaceType {
    pub fn is_local(&self) -> bool {
        matches!(self, LapceWorkspaceType::Local)
    }

    pub fn is_remote(&self) -> bool {
        use LapceWorkspaceType::*;

        #[cfg(not(windows))]
        return matches!(self, RemoteSSH(_));

        #[cfg(windows)]
        return matches!(self, RemoteSSH(_) | RemoteWSL(_));
    }
}

impl std::fmt::Display for LapceWorkspaceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LapceWorkspaceType::Local => f.write_str("Local"),
            LapceWorkspaceType::RemoteSSH(remote) => {
                write!(f, "ssh://{remote}")
            }
            #[cfg(windows)]
            LapceWorkspaceType::RemoteWSL(remote) => {
                write!(f, "{remote} (WSL)")
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LapceWorkspace {
    pub kind: LapceWorkspaceType,
    pub path: Option<PathBuf>,
    pub last_open: u64,
}

impl LapceWorkspace {
    pub fn display(&self) -> Option<String> {
        let path = self.path.as_ref()?;
        let path = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .to_string();
        let remote = match &self.kind {
            LapceWorkspaceType::Local => String::new(),
            LapceWorkspaceType::RemoteSSH(remote) => {
                format!(" [SSH: {}]", remote.host)
            }
            #[cfg(windows)]
            LapceWorkspaceType::RemoteWSL(remote) => {
                format!(" [WSL: {}]", remote.host)
            }
        };
        Some(format!("{path}{remote}"))
    }
}

impl Default for LapceWorkspace {
    fn default() -> Self {
        Self {
            kind: LapceWorkspaceType::Local,
            path: None,
            last_open: 0,
        }
    }
}

impl std::fmt::Display for LapceWorkspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}",
            self.kind,
            self.path.as_ref().and_then(|p| p.to_str()).unwrap_or("")
        )
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub split: SplitInfo,
    pub panel: PanelInfo,
    pub breakpoints: HashMap<PathBuf, Vec<LapceBreakpoint>>,
}
