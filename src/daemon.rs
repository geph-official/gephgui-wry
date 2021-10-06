use std::process::Stdio;

use serde::Deserialize;
use tap::Pipe;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Synchronizes the stuff
pub fn sync_status(
    username: String,
    password: String,
    force: bool,
) -> anyhow::Result<serde_json::Value> {
    let cmd = std::process::Command::new(DAEMON_PATH)
        .arg("sync")
        .arg("--username")
        .arg(&username)
        .arg("--password")
        .arg(&password)
        .pipe(|c| if force { c.arg("--force") } else { c })
        .stdout(Stdio::piped())
        .spawn()?;
    let output = cmd.wait_with_output()?;
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// Starts the binder proxy, returning a process
pub fn start_binder_proxy() -> anyhow::Result<std::process::Child> {
    Ok(std::process::Command::new(DAEMON_PATH)
        .arg("binder-proxy")
        .arg("--listen")
        .arg("127.0.0.1:23456")
        .spawn()?)
}

/// Configuration for starting the daemon
#[derive(Deserialize, Debug)]
pub struct DaemonConfig {
    pub username: String,
    pub password: String,
    pub exit_name: String,
    pub use_tcp: bool,
    pub force_bridges: bool,
    pub vpn: bool,
    pub exclude_prc: bool,
}

const DAEMON_PATH: &str = "geph4-client";

const VPN_HELPER_PATH: &str = "geph4-vpn-helper";

impl DaemonConfig {
    /// Starts the daemon, returning a process.
    pub fn start(self) -> anyhow::Result<std::process::Child> {
        let mut command = if self.vpn {
            std::mem::replace(
                std::process::Command::new(VPN_HELPER_PATH)
                    .arg(DAEMON_PATH)
                    .arg("connect")
                    .arg("--stdio-vpn")
                    .arg("--dns-listen")
                    .arg("127.0.0.1:15353"),
                std::process::Command::new(VPN_HELPER_PATH),
            )
        } else {
            let mut cmd = std::process::Command::new(DAEMON_PATH);
            cmd.arg("connect");
            cmd
        };
        command
            .arg("--username")
            .arg(self.username.as_str())
            .arg("--password")
            .arg(self.password.as_str())
            .arg("--exit-server")
            .arg(self.exit_name.as_str())
            .pipe(|c| if self.use_tcp { c.arg("--use-tcp") } else { c })
            .pipe(|c| if self.use_tcp { c.arg("--use-tcp") } else { c })
            .pipe(|c| {
                if self.exclude_prc {
                    c.arg("--exclude-prc")
                } else {
                    c
                }
            });
        #[cfg(windows)]
            command.creation_flags(0x08000000);
        let child = command
            .spawn()?;
        Ok(child)
    }
}
