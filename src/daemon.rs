use serde::Deserialize;
use tap::Tap;

use crate::rpc_handler::DeathBoxInner;
use anyhow::Context;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

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
}

const DAEMON_PATH: &str = "geph4-client";

/// Returns the directory where all the log files are found.
pub fn logfile_directory() -> PathBuf {
    let mut base = dirs::data_local_dir().expect("no local dir");
    base.push("geph4-logs");
    let _ = std::fs::create_dir_all(&base);
    base
}

impl DaemonConfig {
    /// Starts the daemon, returning a death handle.
    pub fn start(self) -> anyhow::Result<DeathBoxInner> {
        let logfile_name = logfile_directory().tap_mut(|p| {
            p.push(format!(
                "geph4-logs-{}.txt",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
            ))
        });
        let log_file = std::fs::File::create(&logfile_name).context("cannot create log file")?;
        let common_args = Vec::new()
            .tap_mut(|v| {
                v.push("--username".to_string());
                v.push(self.username.clone());
                v.push("--password".into());
                v.push(self.password.clone());
                v.push("--exit-server".into());
                v.push(self.exit_hostname.clone());
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
            });

        if self.vpn_mode {
            #[cfg(target_os = "linux")]
            {
                let mut cmd = std::process::Command::new("pkexec");
                cmd.arg(DAEMON_PATH);
                cmd.arg("connect");
                cmd.args(&common_args);
                cmd.stderr(log_file);
                cmd.arg("--vpn-mode").arg("tun-route");
                let mut child = cmd.spawn().context("cannot spawn non-VPN child")?;
                Ok(Box::new(move || {
                    child.kill()?;
                    child.wait()?;
                    Ok(())
                }))
            }
            #[cfg(target_os = "windows")]
            {
                if !is_elevated::is_elevated() {
                    anyhow::bail!("VPN mode requires admin privileges on Windows!!!")
                }
                let mut cmd = std::process::Command::new(DAEMON_PATH);
                cmd.arg("connect");
                cmd.args(&common_args);
                cmd.arg("--vpn-mode").arg("windivert");
                cmd.stderr(log_file);
                #[cfg(windows)]
                cmd.creation_flags(0x08000000);
                let mut child = cmd.spawn().context("cannot spawn non-VPN child")?;
                Ok(Box::new(move || {
                    child.kill()?;
                    child.wait()?;
                    Ok(())
                }))
            }
            #[cfg(target_os = "macos")]
            {
                anyhow::bail!("VPN mode not supported on macOS")
            }
        } else {
            let mut cmd = std::process::Command::new(DAEMON_PATH);
            cmd.arg("connect");
            cmd.args(&common_args);
            cmd.stderr(log_file);
            #[cfg(windows)]
            cmd.creation_flags(0x08000000);
            let mut child = cmd.spawn().context("cannot spawn non-VPN child")?;
            Ok(Box::new(move || {
                child.kill()?;
                child.wait()?;
                Ok(())
            }))
        }
    }
}
