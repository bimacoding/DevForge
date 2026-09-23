use std::{
    io::{BufReader, Write},
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Result, anyhow};
use devforge_core::{
    directory::Directory,
    meta::{self, ReleaseType},
};
use devforge_rpc::{
    RpcMessage,
    core::CoreRpcHandler,
    proxy::{ProxyRpc, ProxyRpcHandler},
    stdio_transport,
};
use flate2::read::GzDecoder;
use thiserror::Error;
use tracing::{debug, error};

const UNIX_PROXY_SCRIPT: &[u8] = include_bytes!("../../../extra/proxy.sh");
const WINDOWS_PROXY_SCRIPT: &[u8] = include_bytes!("../../../extra/proxy.ps1");

#[derive(Clone, Copy, Error, Debug, PartialEq, Eq, strum_macros::Display)]
#[strum(ascii_case_insensitive)]
enum HostPlatform {
    UnknownOS,
    #[strum(serialize = "windows")]
    Windows,
    #[strum(serialize = "linux")]
    Linux,
    #[strum(serialize = "darwin")]
    Darwin,
    #[strum(serialize = "bsd")]
    Bsd,
}

/// serialise via strum to arch name that is used
/// in CI artefacts
#[derive(Clone, Copy, Error, Debug, PartialEq, Eq, strum_macros::Display)]
#[strum(ascii_case_insensitive)]
enum HostArchitecture {
    UnknownArch,
    #[strum(serialize = "x86_64")]
    AMD64,
    #[strum(serialize = "x86")]
    X86,
    #[strum(serialize = "aarch64")]
    ARM64,
    #[strum(serialize = "armv7")]
    ARM32v7,
    #[strum(serialize = "armhf")]
    ARM32v6,
}

pub trait Remote: Sized {
    #[allow(unused)]
    fn home_dir(&self) -> Result<String> {
        let cmd = self
            .command_builder()
            .arg("echo")
            .arg("-n")
            .arg("$HOME")
            .stdout(Stdio::piped())
            .output()?;

        Ok(String::from_utf8(cmd.stdout)?)
    }

    fn upload_file(&self, local: impl AsRef<Path>, remote: &str) -> Result<()>;

    fn command_builder(&self) -> Command;
}

pub fn start_remote(
    remote: impl Remote,
    core_rpc: CoreRpcHandler,
    proxy_rpc: ProxyRpcHandler,
) -> Result<()> {
    // Note about platforms:
    // Windows can use either cmd.exe, powershell.exe or pwsh.exe as
    // SSH shell, syntax logic varies significantly that's why we bet on
    // cmd.exe as it doesn't add unwanted newlines and use powershell only
    // for proxy install
    //
    // Unix-like systems due to POSIX, always have /bin/sh which should not
    // be necessary to use explicitly most of the time, as many wide-spread
    // shells retain similar syntax, although shells like Nushell might not
    // work (hopefully no one uses it as login shell)
    use HostPlatform::*;
    let (platform, architecture) = host_specification(&remote).unwrap();

    if platform == UnknownOS || architecture == HostArchitecture::UnknownArch {
        error!("detected remote host: {platform}/{architecture}");
        return Err(anyhow!("Unknown OS and/or architecture"));
    }

    // ! Below paths have to be synced with what is
    // ! returned by Config::proxy_directory()
    let remote_proxy_path = match platform {
        Windows => format!(
            "%HOMEDRIVE%%HOMEPATH%\\AppData\\Local\\devforge\\{}\\data\\proxy",
            meta::NAME
        ),
        Darwin => format!(
            "~/Library/Application\\ Support/dev.devforge.{}/proxy",
            meta::NAME
        ),
        _ => {
            format!("~/.local/share/{}/proxy", meta::NAME.to_lowercase())
        }
    };

    let remote_proxy_file = match platform {
        Windows => format!("{remote_proxy_path}\\devforge.exe"),
        _ => format!("{remote_proxy_path}/devforge"),
    };

    if !remote
        .command_builder()
        .args([&remote_proxy_file, "--version"])
        .output()
        .map(|output| {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Accept both legacy Lapce-proxy and DevForge proxy version banners.
            let ok_debug = meta::RELEASE == ReleaseType::Debug
                && (stdout.contains("Lapce-proxy")
                    || stdout.contains("DevForge")
                    || stdout.contains("devforge"));
            let expected = format!("Lapce-proxy {}", meta::VERSION);
            let ok_release = stdout.trim() == expected
                || stdout.trim() == format!("DevForge-proxy {}", meta::VERSION);
            output.status.success() && (ok_debug || ok_release)
        })
        .unwrap_or(false)
    {
        download_remote(
            &remote,
            &platform,
            &architecture,
            &remote_proxy_path,
            &remote_proxy_file,
        )?;
    };

    // Final sanity check — never spawn a missing remote binary.
    let version_ok = remote
        .command_builder()
        .args([&remote_proxy_file, "--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !version_ok {
        return Err(anyhow!(
            "Remote proxy missing at `{remote_proxy_file}`. \
             Debug builds cannot download GitHub `nightly` (404). \
             Place `devforge-proxy-{platform}-{architecture}` (or a .gz) in the local proxy folder, \
             set DEVFORGE_REMOTE_PROXY to that file, or publish a matching GitHub release."
        ));
    }

    debug!("remote proxy path: {remote_proxy_path}");

    let mut child = match platform {
        // Force cmd.exe usage to resolve %envvar% variables
        Windows => remote
            .command_builder()
            .args(["cmd", "/c"])
            .arg(&remote_proxy_file)
            .arg("--proxy")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?,
        _ => remote
            .command_builder()
            .arg(&remote_proxy_file)
            .arg("--proxy")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?,
    };
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("can't find stdin"))?;
    let stdout = BufReader::new(
        child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("can't find stdout"))?,
    );
    debug!("process id: {}", child.id());

    let (writer_tx, writer_rx) = crossbeam_channel::unbounded();
    let (reader_tx, reader_rx) = crossbeam_channel::unbounded();
    stdio_transport(stdin, writer_rx, stdout, reader_tx);

    let local_proxy_rpc = proxy_rpc.clone();
    let local_writer_tx = writer_tx.clone();
    std::thread::Builder::new()
        .name("ProxyRpcHandler".to_owned())
        .spawn(move || {
            for msg in local_proxy_rpc.rx() {
                match msg {
                    ProxyRpc::Request(id, rpc) => {
                        if let Err(err) =
                            local_writer_tx.send(RpcMessage::Request(id, rpc))
                        {
                            tracing::error!("{:?}", err);
                        }
                    }
                    ProxyRpc::Notification(rpc) => {
                        if let Err(err) =
                            local_writer_tx.send(RpcMessage::Notification(rpc))
                        {
                            tracing::error!("{:?}", err);
                        }
                    }
                    ProxyRpc::Shutdown => {
                        if let Err(err) = child.kill() {
                            tracing::error!("{:?}", err);
                        }
                        if let Err(err) = child.wait() {
                            tracing::error!("{:?}", err);
                        }
                        return;
                    }
                }
            }
        })
        .unwrap();

    for msg in reader_rx {
        match msg {
            RpcMessage::Request(id, req) => {
                let writer_tx = writer_tx.clone();
                let core_rpc = core_rpc.clone();
                std::thread::spawn(move || match core_rpc.request(req) {
                    Ok(resp) => {
                        if let Err(err) =
                            writer_tx.send(RpcMessage::Response(id, resp))
                        {
                            tracing::error!("{:?}", err);
                        }
                    }
                    Err(e) => {
                        if let Err(err) = writer_tx.send(RpcMessage::Error(id, e)) {
                            tracing::error!("{:?}", err);
                        }
                    }
                });
            }
            RpcMessage::Notification(n) => {
                core_rpc.notification(n);
            }
            RpcMessage::Response(id, resp) => {
                proxy_rpc.handle_response(id, Ok(resp));
            }
            RpcMessage::Error(id, err) => {
                proxy_rpc.handle_response(id, Err(err));
            }
        }
    }

    Ok(())
}

fn download_remote(
    remote: &impl Remote,
    platform: &HostPlatform,
    architecture: &HostArchitecture,
    remote_proxy_path: &str,
    remote_proxy_file: &str,
) -> Result<()> {
    use base64::{Engine as _, engine::general_purpose};

    let script_install = match platform {
        HostPlatform::Windows => {
            let local_proxy_script =
                Directory::proxy_directory().unwrap().join("proxy.ps1");

            let mut proxy_script = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&local_proxy_script)?;
            proxy_script.write_all(WINDOWS_PROXY_SCRIPT)?;

            let remote_proxy_script = "${env:TEMP}\\devforge-proxy.ps1";
            remote.upload_file(local_proxy_script, remote_proxy_script)?;

            let cmd = remote
                .command_builder()
                .args([
                    "powershell",
                    "-c",
                    remote_proxy_script,
                    "-version",
                    meta::VERSION,
                    "-directory",
                    remote_proxy_path,
                ])
                .output()?;
            debug!("{}", String::from_utf8_lossy(&cmd.stderr));
            debug!("{}", String::from_utf8_lossy(&cmd.stdout));

            cmd.status
        }
        _ => {
            let proxy_script = general_purpose::STANDARD.encode(UNIX_PROXY_SCRIPT);

            let version = match meta::RELEASE {
                ReleaseType::Debug => "nightly".to_string(),
                ReleaseType::Nightly => "nightly".to_string(),
                ReleaseType::Stable => format!("v{}", meta::VERSION),
            };
            let cmd = remote
                .command_builder()
                .args([
                    "echo",
                    &proxy_script,
                    "|",
                    "base64",
                    "-d",
                    "|",
                    "sh",
                    "/dev/stdin",
                    &version,
                    remote_proxy_path,
                ])
                .output()?;

            debug!("{}", String::from_utf8_lossy(&cmd.stderr));
            debug!("{}", String::from_utf8_lossy(&cmd.stdout));

            cmd.status
        }
    };

    if !script_install.success() {
        let cmd = match platform {
            HostPlatform::Windows => remote
                .command_builder()
                .args(["dir", remote_proxy_file])
                .status()?,
            _ => remote
                .command_builder()
                .arg("test")
                .arg("-e")
                .arg(remote_proxy_file)
                .status()?,
        };
        if !cmd.success() {
            let proxy_filename = format!("devforge-proxy-{platform}-{architecture}");
            let proxy_dir = Directory::proxy_directory()
                .ok_or_else(|| anyhow!("can't find proxy directory"))?;
            let download_path = proxy_dir.join(&proxy_filename);

            // Try GitHub release artifact (stable/nightly). Debug builds usually 404
            // on DevForge — fall back to Lapce nightly (compatible enough for debug).
            let proxy_version = match meta::RELEASE {
                meta::ReleaseType::Stable => meta::VERSION,
                _ => "nightly",
            };
            let urls = [
                format!(
                    "https://github.com/bimacoding/DevForge/releases/download/{proxy_version}/{proxy_filename}.gz"
                ),
                format!(
                    "https://github.com/lapce/lapce/releases/download/nightly/lapce-proxy-{platform}-{architecture}.gz"
                ),
            ];
            let mut downloaded = false;
            for url in &urls {
                debug!("proxy download URI: {url}");
                match try_download_gzip_proxy(url, &download_path) {
                    Ok(()) => {
                        downloaded = true;
                        break;
                    }
                    Err(e) => {
                        error!("proxy download failed from {url}: {e}");
                    }
                }
            }
            if !downloaded {
                error!("all proxy download URLs failed; will try local binary next");
            }

            let local_proxy_file = resolve_local_proxy_binary(platform, architecture)
                .ok_or_else(|| {
                    anyhow!(
                        "Remote proxy install failed and no local `{proxy_filename}` was found.\n\
                         • Publish GitHub release `{proxy_version}/{proxy_filename}.gz`, or\n\
                         • Copy the linux/darwin proxy binary to:\n  {}\n\
                         • Or set env DEVFORGE_REMOTE_PROXY=/path/to/proxy",
                        proxy_dir.display()
                    )
                })?;

            match platform {
                HostPlatform::Windows => remote
                    .command_builder()
                    .arg("mkdir")
                    .arg(remote_proxy_path)
                    .status()?,
                _ => remote
                    .command_builder()
                    .arg("mkdir")
                    .arg("-p")
                    .arg(remote_proxy_path)
                    .status()?,
            };

            remote.upload_file(&local_proxy_file, remote_proxy_file)?;
            if platform != &HostPlatform::Windows {
                let chmod = remote
                    .command_builder()
                    .arg("chmod")
                    .arg("+x")
                    .arg(remote_proxy_file)
                    .status()?;
                if !chmod.success() {
                    anyhow::bail!("chmod +x `{remote_proxy_file}` failed");
                }
            }
        }
    }

    Ok(())
}

fn try_download_gzip_proxy(url: &str, dest: &Path) -> Result<()> {
    let mut resp = devforge_proxy::get_url(url, None)?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {}", resp.status());
    }
    let mut out = std::fs::File::create(dest)
        .map_err(|e| anyhow!("failed to create {}: {e}", dest.display()))?;
    let mut gz = GzDecoder::new(&mut resp);
    std::io::copy(&mut gz, &mut out)
        .map_err(|e| anyhow!("failed to decompress proxy download: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(dest)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(dest, perms)?;
    }
    Ok(())
}

/// Locate a proxy binary that can be uploaded to the remote host.
fn resolve_local_proxy_binary(
    platform: &HostPlatform,
    architecture: &HostArchitecture,
) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    if let Ok(explicit) = std::env::var("DEVFORGE_REMOTE_PROXY") {
        let p = PathBuf::from(explicit);
        if p.is_file() {
            return Some(p);
        }
    }

    let proxy_dir = Directory::proxy_directory()?;
    let name = format!("devforge-proxy-{platform}-{architecture}");
    let candidates = [proxy_dir.join(&name), proxy_dir.join(format!("{name}.exe"))];
    for c in &candidates {
        if c.is_file() {
            return Some(c.clone());
        }
    }

    // Same-OS/arch: reuse a locally built `devforge-proxy` next to this executable
    // or under common cargo target folders.
    if local_platform_matches(platform, architecture) {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                for name in ["devforge-proxy", "devforge-proxy.exe", "lapce-proxy"] {
                    let p = dir.join(name);
                    if p.is_file() {
                        return Some(p);
                    }
                }
                // target/fastdev/devforge → look for sibling /fastdev/devforge-proxy
                for name in ["devforge-proxy", "devforge-proxy.exe"] {
                    let p = dir.join(name);
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }
    }

    // Last resort: previously downloaded (possibly incomplete) artifact path —
    // only if the file actually exists after a successful download attempt.
    let downloaded =
        proxy_dir.join(format!("devforge-proxy-{platform}-{architecture}"));
    if downloaded.is_file() {
        return Some(downloaded);
    }

    None
}

fn local_platform_matches(
    platform: &HostPlatform,
    architecture: &HostArchitecture,
) -> bool {
    let local_os = if cfg!(target_os = "windows") {
        HostPlatform::Windows
    } else if cfg!(target_os = "macos") {
        HostPlatform::Darwin
    } else if cfg!(target_os = "linux") {
        HostPlatform::Linux
    } else {
        HostPlatform::UnknownOS
    };

    let local_arch = if cfg!(target_arch = "x86_64") {
        HostArchitecture::AMD64
    } else if cfg!(target_arch = "aarch64") {
        HostArchitecture::ARM64
    } else if cfg!(target_arch = "x86") {
        HostArchitecture::X86
    } else {
        HostArchitecture::UnknownArch
    };

    local_os == *platform && local_arch == *architecture
}

fn host_specification(
    remote: &impl Remote,
) -> Result<(HostPlatform, HostArchitecture)> {
    use HostArchitecture::*;
    use HostPlatform::*;

    let cmd = remote.command_builder().args(["uname", "-sm"]).output();

    let spec = match cmd {
        Ok(cmd) => {
            let stdout = String::from_utf8_lossy(&cmd.stdout).to_lowercase();
            let stdout = stdout.trim();
            debug!("{}", &stdout);
            match stdout {
                // If empty, then we probably deal with Windows and not Unix
                // or something went wrong with command output
                "" => {
                    // Try cmd explicitly
                    let cmd = remote
                        .command_builder()
                        .args(["cmd", "/c", "echo %OS% %PROCESSOR_ARCHITECTURE%"])
                        .output();
                    match cmd {
                        Ok(cmd) => {
                            let stdout =
                                String::from_utf8_lossy(&cmd.stdout).to_lowercase();
                            let stdout = stdout.trim();
                            debug!("{}", &stdout);
                            match stdout.split_once(' ') {
                                Some((os, arch)) => (parse_os(os), parse_arch(arch)),
                                None => {
                                    // PowerShell fallback
                                    let cmd = remote
                                            .command_builder()
                                            .args(["echo", "\"${env:OS} ${env:PROCESSOR_ARCHITECTURE}\""])
                                            .output();
                                    match cmd {
                                        Ok(cmd) => {
                                            let stdout =
                                                String::from_utf8_lossy(&cmd.stdout)
                                                    .to_lowercase();
                                            let stdout = stdout.trim();
                                            debug!("{}", &stdout);
                                            match stdout.split_once(' ') {
                                                Some((os, arch)) => {
                                                    (parse_os(os), parse_arch(arch))
                                                }
                                                None => (UnknownOS, UnknownArch),
                                            }
                                        }
                                        Err(e) => {
                                            error!("{e}");
                                            (UnknownOS, UnknownArch)
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            error!("{e}");
                            (UnknownOS, UnknownArch)
                        }
                    }
                }
                v => {
                    if let Some((os, arch)) = v.split_once(' ') {
                        (parse_os(os), parse_arch(arch))
                    } else {
                        (UnknownOS, UnknownArch)
                    }
                }
            }
        }
        Err(e) => {
            error!("{e}");
            (UnknownOS, UnknownArch)
        }
    };
    Ok(spec)
}

fn parse_arch(arch: &str) -> HostArchitecture {
    use HostArchitecture::*;
    // processor architectures be like that
    match arch {
        "amd64" | "x64" | "x86_64" => AMD64,
        "x86" | "i386" | "i586" | "i686" => X86,
        "arm" | "armhf" | "armv6" => ARM32v6,
        "armv7" | "armv7l" => ARM32v7,
        "arm64" | "armv8" | "aarch64" => ARM64,
        _ => UnknownArch,
    }
}

fn parse_os(os: &str) -> HostPlatform {
    use HostPlatform::*;
    match os {
        "linux" => Linux,
        "darwin" => Darwin,
        "windows_nt" => Windows,
        v if v.ends_with("bsd") => Bsd,
        _ => UnknownOS,
    }
}
