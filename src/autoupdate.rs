use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context;
use async_trait::async_trait;
use geph5_client::ControlClient;
use nanorpc::{JrpcRequest, JrpcResponse, RpcTransport};
use rfd::MessageDialog;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::fs;

use crate::daemon::daemon_rpc;

pub async fn check_update_loop() {
    loop {
        if let Err(err) = check_update_inner().await {
            tracing::debug!(err = debug(err), "checking update failed!");
            smol::Timer::after(Duration::from_secs(10)).await;
        } else {
            smol::Timer::after(Duration::from_secs(3600)).await;
        }
    }
}

async fn check_update_inner() -> anyhow::Result<()> {
    let (manifest, base_url) = ControlClient(DaemonRpcTransport)
        .get_update_manifest()
        .await?
        .map_err(|e| anyhow::anyhow!(e))?;
    let entry: ManifestEntry = serde_json::from_value(manifest[TRACK].clone())?;
    let url = format!("{base_url}/{TRACK}/{}/{}", entry.version, entry.filename);

    // Check if a version upgrade is available by comparing semver
    let current_version = Version::parse(
        option_env!("VERSION")
            .unwrap_or("0.0.0")
            .trim_start_matches('v'),
    )?;
    let manifest_version = Version::parse(&entry.version)?;

    if manifest_version <= current_version {
        // No update needed
        return Ok(());
    }

    // okay now we download if we need to
    let hash_path = dirs::cache_dir()
        .context("no cache dir in the system")?
        .join("geph5-dl")
        .join(&entry.sha256);
    std::fs::create_dir_all(&hash_path)?;

    // Define the download path
    let download_path = hash_path.join(&entry.filename);

    // Check if we need to download the file or if we already have it
    let download_path_str = download_path.to_string_lossy().to_string();
    let need_download =
        !download_path.exists() || read_file_sha256(download_path.clone()).await? != entry.sha256;

    // Download if needed
    if need_download {
        // File doesn't exist or hash doesn't match, need to download
        tracing::info!(
            "Downloading update from {} to {}",
            url,
            download_path.display()
        );

        // Download the file using reqwest
        let resp = reqwest::get(&url).await?;
        let bytes = resp.bytes().await?;

        // Write the file
        fs::write(&download_path, &bytes)?;

        // Verify the hash
        let file_hash = read_file_sha256(download_path.clone()).await?;
        if file_hash != entry.sha256 {
            anyhow::bail!("Downloaded file hash mismatch");
        }
    }

    // Now that we have the file (either downloaded or already had it), show the update dialog
    alert_update(entry.version, download_path_str).await?;

    Ok(())
}

async fn alert_update(version: String, path: String) -> anyhow::Result<()> {
    // Use smol::unblock to perform blocking dialog operations
    smol::unblock(move || {
        // Check if system language is Chinese
        let is_chinese = sys_locale::get_locale().unwrap_or_default().contains("zh");

        // Prepare dialog text based on language
        let title = if is_chinese {
            "迷雾通更新可用"
        } else {
            "Geph Update Available"
        };

        let description = if is_chinese {
            format!("迷雾通新版本可用 ({version})。保存安装程序？")
        } else {
            format!("A new version of Geph is available ({version}). Save installer?")
        };

        let save_title = if is_chinese {
            "保存迷雾通安装程序"
        } else {
            "Save Geph Installer"
        };

        // Show a dialog to inform the user about the update
        let result = MessageDialog::new()
            .set_title(title)
            .set_description(description)
            .set_buttons(rfd::MessageButtons::YesNo)
            .show();

        if result == rfd::MessageDialogResult::Yes {
            // User clicked OK, show a file save dialog
            if let Some(save_path) = rfd::FileDialog::new()
                .set_title(save_title)
                .set_file_name(
                    Path::new(&path)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy(),
                )
                .save_file()
            {
                // Copy the installer to the selected location
                fs::copy(path, &save_path)?;

                // Optional: Open the installer location in file explorer
                #[cfg(target_os = "windows")]
                {
                    let parent = save_path.parent().unwrap_or_else(|| Path::new(""));
                    std::process::Command::new("explorer").arg(parent).spawn()?;
                }

                #[cfg(target_os = "macos")]
                {
                    let parent = save_path.parent().unwrap_or_else(|| Path::new(""));
                    std::process::Command::new("open").arg(parent).spawn()?;
                }

                #[cfg(target_os = "linux")]
                {
                    let parent = save_path.parent().unwrap_or_else(|| Path::new(""));
                    std::process::Command::new("xdg-open").arg(parent).spawn()?;
                }
            }
        }

        Ok(())
    })
    .await
}

async fn read_file_sha256(fname: PathBuf) -> anyhow::Result<String> {
    smol::unblock(move || {
        let bts = std::fs::read(&fname)?;
        anyhow::Ok(hex::encode(hmac_sha256::Hash::hash(&bts)))
    })
    .await
}

#[derive(Serialize, Deserialize, Debug)]
struct ManifestEntry {
    version: String,
    sha256: String,
    filename: String,
}

#[cfg(target_os = "linux")]
const TRACK: &str = "linux-stable";

#[cfg(target_os = "windows")]
const TRACK: &str = "windows-stable";

#[cfg(target_os = "macos")]
const TRACK: &str = "macos-stable";

struct DaemonRpcTransport;

#[async_trait]
impl RpcTransport for DaemonRpcTransport {
    type Error = anyhow::Error;
    async fn call_raw(&self, req: JrpcRequest) -> Result<JrpcResponse, Self::Error> {
        daemon_rpc(req).await
    }
}
