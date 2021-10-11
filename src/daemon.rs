use std::process::Stdio;

use serde::Deserialize;
use tap::{Pipe, Tap};

use crate::interface::DeathBoxInner;
use anyhow::Context;
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
        .tap_mut(|_c| {
            #[cfg(windows)]
            _c.creation_flags(0x08000000);
        })
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
    pub listen_all: bool,
}

const DAEMON_PATH: &str = "geph4-client";

const VPN_HELPER_PATH: &str = "geph4-vpn-helper";

impl DaemonConfig {
    /// Starts the daemon, returning a death handle.
    pub fn start(self) -> anyhow::Result<DeathBoxInner> {
        let common_args = Vec::new()
            .tap_mut(|v| {
                v.push("--username".to_string());
                v.push(self.username.clone());
                v.push("--password".into());
                v.push(self.password.clone());
                v.push("--exit-server".into());
                v.push(self.exit_name.clone());
            })
            .tap_mut(|v| {
                if self.use_tcp {
                    v.push("--use-tcp".into())
                }
            })
            .tap_mut(|v| {
                if self.exclude_prc {
                    v.push("--exclude-prc".into())
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

        if self.vpn {
            #[cfg(target_os = "linux")]
            {
                let mut cmd = std::process::Command::new("pkexec").tap_mut(|f| {
                    f.arg(which::which(VPN_HELPER_PATH).expect("vpn helper not in PATH"));
                });
                let mut child = cmd
                    .arg(which::which(DAEMON_PATH).expect("daemon not in PATH"))
                    .arg("connect")
                    .arg("--stdio-vpn")
                    .arg("--dns-listen")
                    .arg("127.0.0.1:15353")
                    .arg("--credential-cache")
                    .arg("/tmp/geph4-credentials")
                    .args(&common_args)
                    .spawn()?;
                Ok(Box::new(move || {
                    child.kill()?;
                    child.wait()?;
                    Ok(())
                }))
            }
            #[cfg(windows)]
            {
                std::thread::spawn(move || {
                    runas::Command::new(
                        which::which(VPN_HELPER_PATH).expect("vpn helper not in PATH"),
                    )
                    .arg(which::which(DAEMON_PATH).expect("daemon not in PATH"))
                    .arg("connect")
                    .arg("--stdio-vpn")
                    .arg("--dns-listen")
                    .arg("127.0.0.1:15353")
                    .args(&common_args)
                    .gui(true)
                    .show(false)
                    .status()
                    .expect("could not run");
                    tracing::warn!("daemon stopped ITSELF");
                });
                Ok(Box::new(move || {
                    tracing::warn!("IGNORING KILL ON WINDOZE VPN MODE");
                    Ok(())
                }))
            }
        } else {
            let mut cmd = std::process::Command::new(DAEMON_PATH);
            cmd.arg("connect");
            cmd.args(&common_args);
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
