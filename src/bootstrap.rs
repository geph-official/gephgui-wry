//! Startup bootstrap of the privileged `geph manager` (Linux only).
//!
//! The GUI is an unprivileged client of a persistent root manager (see manager.rs):
//! the manager owns the TUN device, routing, kill-switch, and the control socket at
//! `/run/geph/control.sock`. A sandboxed Flatpak GUI cannot run that manager itself,
//! and even a native GUI shouldn't assume it's already installed. So, run early in
//! `main()`, this module makes sure a host manager is present, current, and answering
//! before the webview tries to talk to it.
//!
//! Logic (all Linux, no-op elsewhere):
//!   * If the control socket answers — and, on Flatpak, the installed manager binary
//!     matches the one we bundle — there's nothing to do.
//!   * Otherwise we explain via a native (non-HTML) dialog, elevate once with
//!     `pkexec`, and run the privileged installer:
//!       - Native: `pkexec geph5 register-manager` (the `.deb` put `geph5` on PATH).
//!       - Flatpak: stage the bundled static `geph5`/`geph5-client` plus the
//!         packaging-owned `install-host-manager.sh` to a host-visible dir and run it
//!         via `flatpak-spawn --host pkexec`. All install/cleanup *policy* lives in
//!         that bundled script (owned by gephgui-pkg), not here and not in geph5.
//!
//! The orchestration (detect → dialog → elevate → result) is generic; only the
//! command wrapping and the post-install relaunch differ between native and Flatpak.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::Context;
use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

/// Bundled (in-sandbox) locations of the host manager binaries and packaging assets.
const APP_GEPH5: &str = "/app/bin/geph5";
const APP_GEPH5_CLIENT: &str = "/app/bin/geph5-client";
const APP_LIBEXEC: &str = "/app/libexec/geph";

/// Where the manager binaries live on the host after installation.
const HOST_GEPH5: &str = "/usr/local/bin/geph5";
const HOST_GEPH5_CLIENT: &str = "/usr/local/bin/geph5-client";

/// Files staged out of the Flatpak into a host-visible dir for the privileged
/// installer. The first two are the static binaries; the rest are packaging assets.
const STAGED_FILES: &[(&str, &str)] = &[
    (APP_GEPH5, "geph5"),
    (APP_GEPH5_CLIENT, "geph5-client"),
    (
        "/app/libexec/geph/install-host-manager.sh",
        "install-host-manager.sh",
    ),
    (
        "/app/libexec/geph/geph-cleanup.service",
        "geph-cleanup.service",
    ),
    ("/app/libexec/geph/geph-cleanup.timer", "geph-cleanup.timer"),
    (
        "/app/libexec/geph/geph-flatpak-watchdog.sh",
        "geph-flatpak-watchdog.sh",
    ),
];

/// Ensure the host manager is installed, current, and answering. Returns `true` if
/// the GUI should continue starting up, or `false` if it should exit now (the user
/// quit, or a Flatpak first-run/relaunch is required so the `/run/geph` bind-mount
/// picks up the freshly-created control socket).
pub fn ensure_manager() -> bool {
    let is_flatpak = std::env::var_os("FLATPAK_ID").is_some();
    let was_reachable = reachable();

    let needs_install = if !was_reachable {
        true
    } else if is_flatpak {
        // Reachable, but did a Flatpak update ship a newer manager than the host copy?
        flatpak_manager_stale()
    } else {
        false
    };
    if !needs_install {
        return true;
    }

    if !explain_dialog() {
        return false; // user chose Quit
    }

    // Elevate + install, retrying on failure until it succeeds or the user quits.
    loop {
        match do_install(is_flatpak) {
            Ok(()) => break,
            Err(err) => {
                if !error_retry_dialog(&err.to_string()) {
                    return false;
                }
            }
        }
    }

    if is_flatpak && !was_reachable {
        // Fresh install: `/run/geph` did not exist when this sandbox started, so the
        // `--filesystem=/run/geph:ro` bind-mount missed it. A relaunch picks it up.
        relaunch_dialog();
        return false;
    }

    // Native install, or a Flatpak in-place upgrade (the dir was already mounted):
    // wait briefly for the (re)started manager to bind its socket, then continue.
    for _ in 0..40 {
        if reachable() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    if is_flatpak {
        relaunch_dialog();
        return false;
    }
    // Native: continue anyway; the GUI surfaces its own "can't reach manager" error.
    true
}

/// Can we reach the manager's control socket right now?
fn reachable() -> bool {
    smolscale::block_on(crate::manager::manager_reachable())
}

/// On Flatpak, is the host-installed manager a different build than the one we bundle?
/// Compares content hashes (no version RPC needed). If we can't read our own bundled
/// binaries we conservatively report "not stale"; if the host copy is missing or
/// unreadable we report "stale" so it gets (re)installed.
fn flatpak_manager_stale() -> bool {
    let bundled = match (file_sha256(APP_GEPH5), file_sha256(APP_GEPH5_CLIENT)) {
        (Some(a), Some(b)) => vec![a, b],
        _ => return false,
    };
    match host_sha256(&[HOST_GEPH5, HOST_GEPH5_CLIENT]) {
        Some(installed) => installed != bundled,
        None => true,
    }
}

/// SHA-256 (hex) of a file we can read directly (in-sandbox bundled binaries).
fn file_sha256(path: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(hex::encode(hmac_sha256::Hash::hash(&bytes)))
}

/// SHA-256 (hex) of host files, via `flatpak-spawn --host sha256sum`. The host
/// binaries are world-readable, so the unprivileged host spawn can hash them.
/// Returns one hash per input path, in order, or `None` if any couldn't be hashed.
fn host_sha256(paths: &[&str]) -> Option<Vec<String>> {
    let out = Command::new("flatpak-spawn")
        .arg("--host")
        .arg("sha256sum")
        .args(paths)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let hashes: Vec<String> = text
        .lines()
        .filter_map(|line| line.split_whitespace().next().map(str::to_string))
        .collect();
    (hashes.len() == paths.len()).then_some(hashes)
}

/// Perform the privileged install (single elevation).
fn do_install(is_flatpak: bool) -> anyhow::Result<()> {
    let status = if is_flatpak {
        let staging = stage_assets().context("staging the installer to a host-visible dir")?;
        // `flatpak uninstall` removes the per-app data dir; the sandbox's $HOME *is*
        // that dir at its real host path, so pass it as the self-cleanup "owner".
        let owner = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        let script = staging.join("install-host-manager.sh");
        Command::new("flatpak-spawn")
            .arg("--host")
            .arg("pkexec")
            .arg(&script)
            .arg(&staging)
            .arg(&owner)
            .status()
            .context("running install-host-manager.sh via flatpak-spawn/pkexec")?
    } else {
        // Native: the `.deb` already installed `geph5`; just register the service.
        Command::new("pkexec")
            .arg("geph5")
            .arg("register-manager")
            .status()
            .context("running pkexec geph5 register-manager")?
    };
    if !status.success() {
        anyhow::bail!("privileged installer exited with {status}");
    }
    Ok(())
}

/// Copy the bundled binaries and packaging assets into a host-visible staging dir
/// (under `$XDG_DATA_HOME`, which maps to the same absolute path on the host) and
/// return it. The privileged installer reads everything from there.
fn stage_assets() -> anyhow::Result<PathBuf> {
    let staging = dirs::data_dir()
        .context("no data dir")?
        .join("geph-host-install");
    std::fs::create_dir_all(&staging).with_context(|| format!("mkdir {}", staging.display()))?;
    for (src, name) in STAGED_FILES {
        let dst = staging.join(name);
        std::fs::copy(src, &dst).with_context(|| format!("copy {src} -> {}", dst.display()))?;
        // The host runs these via pkexec, so make sure they're executable.
        std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("chmod {}", dst.display()))?;
    }
    Ok(staging)
}

/// Whether the system locale is Chinese, for picking dialog copy.
fn is_chinese() -> bool {
    sys_locale::get_locale().unwrap_or_default().contains("zh")
}

/// Explain the privileged setup and ask permission. Returns `true` to proceed.
fn explain_dialog() -> bool {
    let (title, body, setup, quit) = if is_chinese() {
        (
            "设置迷雾通后台服务",
            "迷雾通需要安装并启动一个以管理员权限运行的后台系统服务来管理你的连接。系统将提示你输入密码。",
            "设置",
            "退出",
        )
    } else {
        (
            "Set up the Geph background service",
            "Geph needs to install and start a background system service that runs \
             with administrator privileges to manage your connection. Your system \
             will prompt you for your password.",
            "Set up",
            "Quit",
        )
    };
    let result = MessageDialog::new()
        .set_level(MessageLevel::Info)
        .set_title(title)
        .set_description(body)
        .set_buttons(MessageButtons::OkCancelCustom(setup.into(), quit.into()))
        .show();
    matches!(result, MessageDialogResult::Custom(label) if label == setup)
}

/// Report an install failure and offer to retry. Returns `true` to retry.
fn error_retry_dialog(err: &str) -> bool {
    let (title, retry, quit, prefix) = if is_chinese() {
        ("设置失败", "重试", "退出", "无法设置迷雾通后台服务：\n\n")
    } else {
        (
            "Setup failed",
            "Retry",
            "Quit",
            "Geph couldn't set up its background service:\n\n",
        )
    };
    let result = MessageDialog::new()
        .set_level(MessageLevel::Error)
        .set_title(title)
        .set_description(format!("{prefix}{err}"))
        .set_buttons(MessageButtons::OkCancelCustom(retry.into(), quit.into()))
        .show();
    matches!(result, MessageDialogResult::Custom(label) if label == retry)
}

/// Tell the user setup is done and they should reopen Geph (Flatpak first run).
fn relaunch_dialog() {
    let (title, body) = if is_chinese() {
        ("设置完成", "迷雾通后台服务已安装。请重新打开迷雾通以继续。")
    } else {
        (
            "Setup complete",
            "Geph's background service is installed. Please reopen Geph to continue.",
        )
    };
    MessageDialog::new()
        .set_level(MessageLevel::Info)
        .set_title(title)
        .set_description(body)
        .set_buttons(MessageButtons::Ok)
        .show();
}
