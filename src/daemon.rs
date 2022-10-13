use serde::Deserialize;
use tap::Tap;

use crate::rpc_handler::DeathBoxInner;
use anyhow::Context;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

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
                v.push(self.exit_hostname.clone());
            })
            .tap_mut(|v| {
                if self.prc_whitelist {
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

        if self.vpn_mode {
            todo!()
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
