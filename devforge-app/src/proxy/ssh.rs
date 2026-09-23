//! Proxy remote command builder for SSH.
//!
//! When a Host alias is present, prefer connecting via that alias so OpenSSH
//! reads `~/.ssh/config` (HostName, IdentityFile, etc.) — same as Cursor.

use std::{path::Path, process::Command};

use anyhow::Result;
use tracing::debug;

use super::remote::Remote;
use crate::{proxy::new_command, workspace::SshHost};

pub struct SshRemote {
    pub ssh: SshHost,
}

impl SshRemote {
    #[cfg(windows)]
    const SSH_ARGS: &'static [&'static str] = &[];

    #[cfg(unix)]
    const SSH_ARGS: &'static [&'static str] = &[
        "-o",
        "ControlMaster=auto",
        "-o",
        "ControlPath=~/.ssh/cm_%C",
        "-o",
        "ControlPersist=30m",
        "-o",
        "ConnectTimeout=15",
    ];

    fn use_config_alias(ssh: &SshHost) -> bool {
        ssh.alias.is_some()
    }

    fn apply_explicit_options(cmd: &mut Command, ssh: &SshHost) {
        // Only force flags when not relying on ~/.ssh/config Host alias.
        if Self::use_config_alias(ssh) {
            return;
        }

        if let Some(identity) = ssh.expanded_identity_file() {
            cmd.arg("-i").arg(identity);
        }

        if ssh.identities_only == Some(true) {
            cmd.arg("-o").arg("IdentitiesOnly=yes");
        }

        if let Some(interval) = ssh.server_alive_interval {
            cmd.arg("-o").arg(format!("ServerAliveInterval={interval}"));
        }

        if let Some(count) = ssh.server_alive_count_max {
            cmd.arg("-o").arg(format!("ServerAliveCountMax={count}"));
        }
    }
}

impl Remote for SshRemote {
    fn upload_file(&self, local: impl AsRef<Path>, remote: &str) -> Result<()> {
        let local = local.as_ref();
        if !local.exists() {
            anyhow::bail!(
                "local proxy file missing: {} — build `devforge-proxy` for the remote OS/arch or place it under the app proxy directory",
                local.display()
            );
        }

        let mut cmd = new_command("scp");

        cmd.args(Self::SSH_ARGS);
        Self::apply_explicit_options(&mut cmd, &self.ssh);

        if !Self::use_config_alias(&self.ssh) {
            if let Some(port) = self.ssh.port {
                cmd.arg("-P").arg(port.to_string());
            }
        }

        let dest = if Self::use_config_alias(&self.ssh) {
            format!("{}:{remote}", self.ssh.ssh_cli_target())
        } else {
            format!("{}:{remote}", self.ssh.user_host())
        };

        let output = cmd.arg(local).arg(&dest).output()?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        debug!("scp dest={dest} status={}", output.status);
        debug!("{stderr}");
        debug!("{stdout}");

        if !output.status.success() {
            anyhow::bail!(
                "scp to `{dest}` failed ({}): {}",
                output.status,
                stderr.trim()
            );
        }

        Ok(())
    }

    fn command_builder(&self) -> Command {
        let mut cmd = new_command("ssh");
        cmd.args(Self::SSH_ARGS);
        Self::apply_explicit_options(&mut cmd, &self.ssh);

        if !Self::use_config_alias(&self.ssh) {
            if let Some(port) = self.ssh.port {
                cmd.arg("-p").arg(port.to_string());
            }
        }

        cmd.arg(self.ssh.ssh_cli_target());

        if !std::env::var("LAPCE_DEBUG").unwrap_or_default().is_empty()
            || !std::env::var("DEVFORGE_DEBUG")
                .unwrap_or_default()
                .is_empty()
        {
            cmd.arg("-v");
        }

        cmd
    }
}
