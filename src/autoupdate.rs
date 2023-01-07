use anyhow::Context;
use isahc::AsyncReadResponseExt;
use rand::Rng;
use serde::{Deserialize, Serialize};
use smol_timeout::TimeoutExt;
use std::{collections::HashMap, time::Duration};

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
                let (send, recv) = smol::channel::bounded(1);

                mt_enqueue(move |wv| {
                    let res = native_dialog::MessageDialog::new().set_title("Update available / 可用更新").set_text(&format!("A new version ({version}) of Geph is available. Upgrade?\n发现更新版本的迷雾通（{version}）。是否更新？\n發現更新版本的迷霧通（{version}）。是否更新？")).set_owner(wv.window()).show_confirm();
                    let _ = send.try_send(res.unwrap_or_default());
                });
                let decision_made = recv.recv().await?;
                if decision_made {
                    // TODO do something more intelligent
                    let url = picked.resolve_url(&update);
                    mt_enqueue(move |_| {
                        let _ = webbrowser::open(&url);
                    })
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
}
