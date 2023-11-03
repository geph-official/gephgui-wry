use once_cell::sync::Lazy;
use rand::Rng;
use serde::Deserialize;
use tap::Tap;

use anyhow::Context;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::PathBuf;

use crate::mtbus::mt_enqueue;

/// The daemon RPC key
pub static GEPH_RPC_KEY: Lazy<String> = Lazy::new(|| {
    smolscale::block_on(async {
        async fn fallible() -> anyhow::Result<String> {
            // try reading from file
            let mut rpc_key_path = dirs::config_dir().context("could not find config directory")?;
            rpc_key_path.push("geph4-credentials/");
            std::fs::create_dir_all(&rpc_key_path).context("could not create cache directory")?;
            rpc_key_path.push("rpc_key");
            // if key exists, return it
            if let Ok(key_bytes) = std::fs::read(&rpc_key_path) {
                let maybe_key: Result<String, _> = bincode::deserialize(&key_bytes);
                if let Ok(key) = maybe_key {
                    return Ok(key);
                }
            }
            // else, make a new key and store it in the right location
            let key = format!("geph-rpc-key-{}", rand::thread_rng().gen::<u128>());
            std::fs::write(
                rpc_key_path,
                bincode::serialize(&key).context("could not serialize RPC key")?,
            )
            .context("could not write RPC key to file")?;
            Ok(key)
        }

        match fallible().await {
            Ok(key) => key,
            Err(err) => {
                show_fatal_error(err.to_string()).await;
                std::process::exit(1);
            }
        }
    })
});

async fn show_fatal_error(err: String) {
    #[cfg(target_os = "macos")]
    let _ = {
        use rfd::{MessageButtons, MessageLevel};
        rfd::AsyncMessageDialog::new()
            .set_buttons(MessageButtons::Ok)
            .set_level(MessageLevel::Info)
            .set_title("System Error / 系统错误")
            .set_description(&format!("A fatal error has occurred\n系统错误:\n{}", err))
            .show()
            .await
    };
    #[cfg(not(target_os = "macos"))]
    let _ = {
        let (send, recv) = smol::channel::bounded(1);
        mt_enqueue(move |_wv| {
            let res = native_dialog::MessageDialog::new()
                .set_title("System Error / 系统错误")
                .set_text(&format!("A fatal error has occurred\n系统错误:\n{}", err))
                .show_alert();
            let _ = {
                res.unwrap_or_default();
                send.try_send(())
            };
        });
        recv.recv().await
    };
}

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
    pub fn start(self) -> anyhow::Result<()> {
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
                let _child = cmd.spawn().context("cannot spawn non-VPN child")?;
                Ok(())
            }
            #[cfg(target_os = "windows")]
            {
                let mut cmd = runas::Command::new(DAEMON_PATH);
                cmd.arg("connect");
                cmd.arg("--vpn-mode").arg("windivert");
                cmd.args(&common_args);
                cmd.show(false);
                std::thread::spawn(move || cmd.status().unwrap());
                Ok(())
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
            cmd.spawn().context("cannot spawn non-VPN child")?;
            Ok(())
        }
    }
}
