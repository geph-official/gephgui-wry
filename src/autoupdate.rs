#![cfg_attr(target_os = "linux", allow(dead_code))]

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::exit,
    time::Duration,
};

use anyhow::Context;
use async_trait::async_trait;
use rand::Rng;

use geph5_misc_rpc::client_control::ControlClient;
use nanorpc::{JrpcRequest, JrpcResponse, RpcTransport};
use rfd::MessageDialog;
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::daemon::{daemon_rpc, stop_daemon};

const UPDATE_MEAN_INTERVAL_HOURS: f64 = 72.0;
const RETRY_DELAY_SECONDS: u64 = 600;
const CACHE_FOLDER: &str = "geph5-dl";
const METADATA_FILE: &str = "update-metadata.json";

/// Background loop that periodically downloads updates (if any) and records
/// metadata so we can prompt on the next startup.
pub async fn download_update_loop() {
    loop {
        let delay =
            sample_poisson_delay(Duration::from_secs_f64(UPDATE_MEAN_INTERVAL_HOURS * 3600.0));
        tracing::debug!(delay = debug(delay), "delay set for update checking");
        smol::Timer::after(delay).await;
        match ensure_update_cached().await {
            Ok(reason) => {
                tracing::debug!(
                    ?reason,
                    wait_seconds = delay.as_secs_f64(),
                    "next update check scheduled"
                );
            }
            Err(err) => {
                tracing::debug!(err = debug(err), "failed to cache update");
            }
        }
    }
}

/// On startup, prompt the user if we already downloaded an update previously.
pub async fn prompt_cached_update_if_available() -> anyhow::Result<()> {
    let Some(metadata) = load_metadata()? else {
        tracing::debug!("no cached update metadata; skipping prompt");
        return Ok(());
    };
    tracing::debug!(version = %metadata.version, path = %metadata.download_path.display(), "cached update metadata found");

    let current_version = current_version()?;
    let metadata_version = match Version::parse(&metadata.version) {
        Ok(ver) => ver,
        Err(err) => {
            tracing::debug!(err = debug(err), "invalid cached update metadata");
            let _ = clear_metadata();
            return Ok(());
        }
    };

    if metadata_version <= current_version {
        return Ok(());
    }

    if !metadata.download_path.exists() {
        return Ok(());
    }

    run_update(&metadata.version, &metadata.download_path).await
}

#[derive(Debug)]
enum CacheResult {
    AlreadyCurrent,
    CachedFresh,
    AlreadyCached,
}

async fn ensure_update_cached() -> anyhow::Result<CacheResult> {
    let (manifest, base_url) = ControlClient(DaemonRpcTransport)
        .get_update_manifest()
        .await?
        .map_err(|e| anyhow::anyhow!(e))?;
    let entry: ManifestEntry = serde_json::from_value(manifest[TRACK].clone())?;
    let url = format!("{base_url}/{TRACK}/{}/{}", entry.version, entry.filename);
    tracing::debug!(version = %entry.version, url, "update manifest fetched");

    let current_version = current_version()?;
    let manifest_version = Version::parse(&entry.version)?;

    if manifest_version <= current_version {
        tracing::debug!(
            %manifest_version,
            %current_version,
            "already running latest version"
        );
        let _ = clear_metadata();
        return Ok(CacheResult::AlreadyCurrent);
    }

    let cache_dir = cache_root()?;
    let hash_path = cache_dir.join(&entry.sha256);
    fs::create_dir_all(&hash_path)?;
    let download_path = hash_path.join(&entry.filename);

    let need_download =
        !download_path.exists() || read_file_sha256(download_path.clone()).await? != entry.sha256;

    if need_download {
        tracing::info!(
            "Downloading update from {} to {}",
            url,
            download_path.display()
        );

        let resp = reqwest::get(&url).await?;
        let bytes = resp.bytes().await?;
        fs::write(&download_path, &bytes)?;

        let file_hash = read_file_sha256(download_path.clone()).await?;
        if file_hash != entry.sha256 {
            anyhow::bail!("Downloaded file hash mismatch");
        }
    }

    write_metadata(&UpdateMetadata {
        version: entry.version,
        sha256: entry.sha256,
        filename: entry.filename,
        download_path,
    })?;
    tracing::debug!("cached update metadata written successfully");

    Ok(if need_download {
        CacheResult::CachedFresh
    } else {
        CacheResult::AlreadyCached
    })
}

async fn run_update(version: &str, path: &Path) -> anyhow::Result<()> {
    let version = version.to_string();
    let path = path.to_path_buf();

    // Use smol::unblock to perform blocking dialog operations
    let should_exit = smol::unblock(move || {
        // Check if system language is Chinese
        let is_chinese = sys_locale::get_locale().unwrap_or_default().contains("zh");

        // Prepare dialog text based on language
        let title = if is_chinese {
            "迷雾通更新可用"
        } else {
            "Geph Update Available"
        };

        let description = if is_chinese {
            format!("迷雾通新版本可用 ({version}). 安装此更新将停止当前迷雾通程序并运行安装程序。现在安装？")
        } else {
            format!("A new version of Geph is available ({version}). Installing this update will stop the current Geph program and run the installer. Install now?")
        };

        // Show a dialog to inform the user about the update
        let result = MessageDialog::new()
            .set_title(title)
            .set_description(&description)
            .set_buttons(rfd::MessageButtons::YesNo)
            .show();

        if result == rfd::MessageDialogResult::Yes {
            // User clicked Yes, run the installer

            // Run the installer
            #[cfg(target_os = "windows")]
            {
                // On Windows, just execute the installer
                std::process::Command::new(&path).arg("/SILENT").spawn()?;
            }

            #[cfg(target_os = "macos")]
            {
                // On macOS, open the .dmg or .pkg file
                std::process::Command::new("open").arg(&path).spawn()?;
            }

            #[cfg(target_os = "linux")]
            {
                let _ = &path;
            }

            // Return true to indicate we should exit
            anyhow::Ok(true)
        } else {
            // User clicked No, don't exit
            Ok(false)
        }
    })
    .await?;

    if should_exit {
        // Stop the daemon
        stop_daemon().await?;

        // Exit the application
        tracing::info!("Exiting for update installation");
        exit(0);
    }

    Ok(())
}

fn cache_root() -> anyhow::Result<PathBuf> {
    let path = dirs::cache_dir()
        .context("no cache dir in the system")?
        .join(CACHE_FOLDER);
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn metadata_path() -> anyhow::Result<PathBuf> {
    Ok(cache_root()?.join(METADATA_FILE))
}

fn write_metadata(metadata: &UpdateMetadata) -> anyhow::Result<()> {
    let path = metadata_path()?;
    let bytes = serde_json::to_vec(metadata)?;
    fs::write(path, bytes)?;
    Ok(())
}

fn load_metadata() -> anyhow::Result<Option<UpdateMetadata>> {
    let path = metadata_path()?;
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn clear_metadata() -> anyhow::Result<()> {
    let path = metadata_path()?;
    match fs::remove_file(path) {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn sample_poisson_delay(mean: Duration) -> Duration {
    let mut rng = rand::thread_rng();
    let u: f64 = rng.gen_range(f64::MIN_POSITIVE..1.0);
    let secs = -mean.as_secs_f64() * u.ln();
    Duration::from_secs_f64(secs)
}

fn current_version() -> anyhow::Result<Version> {
    Version::parse(
        option_env!("VERSION")
            .unwrap_or("0.0.0")
            .trim_start_matches('v'),
    )
    .map_err(|e| e.into())
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

#[derive(Serialize, Deserialize, Debug)]
struct UpdateMetadata {
    version: String,
    sha256: String,
    filename: String,
    download_path: PathBuf,
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
