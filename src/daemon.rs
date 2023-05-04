use once_cell::sync::Lazy;
use rand::Rng;
use serde::Deserialize;
use tap::Tap;

use anyhow::Context;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::PathBuf;

/// The daemon RPC key
pub static GEPH_RPC_KEY: Lazy<String> =
    Lazy::new(|| format!("geph-rpc-key-{}", rand::thread_rng().gen::<u128>()));

/// Configuration for starting the daemon
#[derive(Deserialize, Debug)]
pub struct DaemonConfig {
    pub username: String,
    pub password: String,
    pub exit_hostname: String,
    pub force_bridges: bool,
    pub vpn_mode: bool,
    pub prc_whitelist: bool,
    pub listen_all: bool,
    pub force_protocol: Option<String>,
}

const DAEMON_PATH: &str = "geph4-client";

pub static DAEMON_VERSION: Lazy<String> = Lazy::new(|| {
    let mut cmd = std::process::Command::new(DAEMON_PATH);
    cmd.arg("--version");

    #[cfg(windows)]
    cmd.creation_flags(0x08000000);

    String::from_utf8_lossy(&cmd.output().unwrap().stdout)
        .replace("geph4-client", "")
        .trim()
        .to_string()
});

/// Returns the daemon version.
pub fn daemon_version() -> anyhow::Result<String> {
    Ok(DAEMON_VERSION.clone())
}

/// Returns the directory where all the log files are found.
pub fn debugpack_path() -> PathBuf {
    let mut base = dirs::data_local_dir().expect("no local dir");
    base.push("geph4-logs.db");
    base
}

impl DaemonConfig {
    /// Starts the daemon, returning a death handle.
    pub fn start(self) -> anyhow::Result<std::process::Child> {
        std::env::set_var("GEPH_RPC_KEY", GEPH_RPC_KEY.clone());
        let common_args = Vec::new()
            .tap_mut(|v| {
                v.push("--exit-server".into());
                v.push(self.exit_hostname.clone());
                if let Some(force) = self.force_protocol.clone() {
                    v.push("--force-protocol".into());
                    v.push(force);
                }
                v.push("--debugpack-path".into());
                v.push(debugpack_path().to_string_lossy().to_string());
            })
            .tap_mut(|v| {
                if self.prc_whitelist {
                    v.push("--exclude-prc".into())
                }
            })
            .tap_mut(|v| {
                if self.force_bridges {
                    v.push("--use-bridges".into())
                }
            })
            .tap_mut(|v| {
                if self.listen_all {
                    v.push("--socks5-listen".into());
                    v.push("0.0.0.0:9909".into());
                    v.push("--http-listen".into());
                    v.push("0.0.0.0:9910".into());
                }
            })
            .tap_mut(|v| {
                v.push("auth-password".to_string());
                v.push("--username".to_string());
                v.push(self.username.clone());
                v.push("--password".into());
                v.push(self.password.clone());
            });

        if self.vpn_mode {
            #[cfg(target_os = "linux")]
            {
                let mut cmd = std::process::Command::new("pkexec");
                cmd.arg(DAEMON_PATH);
                cmd.arg("connect");
                cmd.arg("--vpn-mode").arg("tun-route");
                cmd.args(&common_args);
                let child = cmd.spawn().context("cannot spawn non-VPN child")?;
                Ok(child)
            }
            #[cfg(target_os = "windows")]
            {
                if !is_elevated::is_elevated() {
                    anyhow::bail!("VPN mode requires admin privileges on Windows!!!")
                }
                let mut cmd = std::process::Command::new(DAEMON_PATH);
                cmd.arg("connect");
                cmd.arg("--vpn-mode").arg("windivert");
                cmd.args(&common_args);
                #[cfg(windows)]
                cmd.creation_flags(0x08000000);
                let mut child = cmd.spawn().context("cannot spawn non-VPN child")?;
                Ok(child)
            }
            #[cfg(target_os = "macos")]
            {
                anyhow::bail!("VPN mode not supported on macOS")
            }
        } else {
            let mut cmd = std::process::Command::new(DAEMON_PATH);
            cmd.arg("connect");
            cmd.args(&common_args);
            #[cfg(windows)]
            cmd.creation_flags(0x08000000);
            let child = cmd.spawn().context("cannot spawn non-VPN child")?;
            eprintln!("*** CHILD ***");
            Ok(child)
        }
    }
}
