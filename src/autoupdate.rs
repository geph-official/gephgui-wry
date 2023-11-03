use anyhow::Context;
use isahc::AsyncReadResponseExt;
use rand::Rng;

use serde::{Deserialize, Serialize};
use smol_timeout::TimeoutExt;
use std::{
    collections::HashMap,
    fs::{create_dir_all},
    io::{Read},
    time::Duration,
};
use tempfile::Builder;

use crate::{daemon::daemon_version, mtbus::mt_enqueue};

pub async fn autoupdate_loop() {
    eprintln!("enter autoupdate loop");
    loop {
        let downloaders = vec![
            AutoupdateDownloader::new("https://sos-ch-dk-2.exo.io/utopia/geph-releases"),
            AutoupdateDownloader::new("https://f001.backblazeb2.com/file/geph4-dl/geph-releases"),
        ];
        let picked = &downloaders[rand::thread_rng().gen_range(0..downloaders.len())];
        let fallible_part = async {
            let update_avail = picked.update_available().await?;
            if let Some(update) = update_avail {
                let version = update.version.clone();

                #[cfg(target_os = "linux")]
                {
                    let (send, recv) = smol::channel::bounded(1);

                    mt_enqueue(move |_wv| {
                        let res = native_dialog::MessageDialog::new().set_title("Update available / 可用更新").set_text(&format!("A new version ({version}) of Geph is available. Upgrade using the 'flatpak update' command.\n找到更新版本的迷雾通 ({version})。 使用'flatpak update'命令进行更新。\n找到更新版本的迷霧通 ({version})。 使用'flatpak update'命令進行更新。")).show_alert();
                        let _ = {
                            res.unwrap_or_default();
                            send.try_send(())
                        };
                    });
                    recv.recv().await?
                };

                #[cfg(not(target_os = "linux"))]
                {
                    let update_path = picked.download_update().await?;
                    #[cfg(target_os = "windows")]
                    let decision_made = {
                        let (send, recv) = smol::channel::bounded(1);

                        mt_enqueue(move |_wv| {
                            let res = native_dialog::MessageDialog::new().set_title("Update available / 可用更新").set_text(&format!("A new version ({version}) of Geph is available. Upgrade?\n发现更新版本的迷雾通（{version}）。是否更新？\n發現更新版本的迷霧通（{version}）。是否更新？")).show_confirm();
                            let _ = send.try_send(res.unwrap_or_default());
                        });
                        recv.recv().await?
                    };
                    #[cfg(target_os = "macos")]
                    let decision_made: bool = {
                        use rfd::{MessageButtons, MessageLevel};
                        rfd::AsyncMessageDialog::new()
                            .set_buttons(MessageButtons::YesNo)
                            .set_level(MessageLevel::Info)
                            .set_title("Update available / 可用更新")
                            .set_description(&format!("A new version ({version}) of Geph is available. Upgrade?\n发现更新版本的迷雾通（{version}）。是否更新？\n發現更新版本的迷霧通（{version}）。是否更新？"))
                            .show()
                            .await
                    };
                    if decision_made {
                        install_update(update_path)?;
                    }
                }
            }
            anyhow::Ok(())
        };
        if let Err(err) = fallible_part.await {
            tracing::error!("could not check for updates: {:?}", err);
        }
        smol::Timer::after(Duration::from_secs(3600)).await;
    }
}

fn install_update(path: String) -> anyhow::Result<()> {
    eprintln!("Initiating update installation");

    match open::that(&path) {
        Ok(()) => eprintln!("Successfully opened '{}'", &path),
        Err(e) => eprintln!("Error opening '{}': {}", &path, e),
    };

    Ok(())
}

#[derive(Serialize, Deserialize, Debug, Clone)]
/// Metadata for one particular "track".
pub struct UpdateMetadata {
    version: String,
    blake3: String,
    filename: String,
}

/// An autoupdate downloader.
pub struct AutoupdateDownloader {
    base_url: String,
}

#[cfg(target_os = "linux")]
const TRACK: &str = "linux-stable";

#[cfg(target_os = "windows")]
const TRACK: &str = "windows-stable";

#[cfg(target_os = "macos")]
const TRACK: &str = "macos-stable";

impl AutoupdateDownloader {
    /// Creates a new downloader.
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }

    /// Resolves a filename.
    pub fn resolve_url(&self, meta: &UpdateMetadata) -> String {
        format!(
            "{}/{}/{}/{}",
            self.base_url, TRACK, meta.version, meta.filename
        )
    }

    /// Checks whether or not an update is available.
    pub async fn update_available(&self) -> anyhow::Result<Option<UpdateMetadata>> {
        let metadata = self
            .metadata()
            .await?
            .get(TRACK)
            .context("no such track")?
            .clone();
        let remote_ver = semver::Version::parse(&metadata.version)?;
        let our_ver = semver::Version::parse(&daemon_version()?)?;
        if remote_ver > our_ver || std::env::var("GEPH_FORCE_UPDATE").is_ok() {
            Ok(Some(metadata))
        } else {
            Ok(None)
        }
    }

    /// Helper method that returns the update metadata
    async fn metadata(&self) -> anyhow::Result<HashMap<String, UpdateMetadata>> {
        let get_url = format!("{}/metadata.yaml", self.base_url);
        let response = isahc::get_async(&get_url)
            .timeout(Duration::from_secs(10))
            .await
            .context("timeout")??
            .bytes()
            .await?;
        Ok(serde_yaml::from_slice(&response)?)
    }

    async fn download_update(&self) -> anyhow::Result<String> {
        eprintln!("about to update...");
        let metadata = self
            .metadata()
            .await?
            .get(TRACK)
            .context("error getting track")?
            .clone();
        let url = self.resolve_url(&metadata);
        eprintln!("downloading update from {url}...");
        let mut res = isahc::get_async(&url).await?;

        let mut tmp_dir = Builder::new().tempdir()?.into_path();
        let filename = url
            .split('/')
            .last()
            .context("Unable to get update filename")?;
        tmp_dir.push(filename);
        let filepath_str = tmp_dir
            .as_os_str()
            .to_str()
            .context("Error converting tmp directory to string")?;

        create_dir_all(tmp_dir.parent().context("Error getting tmp file dir")?)?;
        res.copy_to(smol::fs::File::create(filepath_str).await?)
            .await?;

        eprintln!(
            "Update v{} was successfully downloaded to {}",
            metadata.version, &filepath_str
        );

        Ok(filepath_str.to_string())
    }
}
