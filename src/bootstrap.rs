//! Startup bootstrap of the privileged `geph manager` (Linux + Windows).
//!
//! The GUI is an unprivileged client of a persistent root manager (see manager.rs):
//! the manager owns the TUN device, routing, kill-switch, and the control endpoint
//! (`/run/geph/control.sock` on Linux, a named pipe on Windows). A sandboxed Flatpak
//! GUI cannot run that manager itself, and even a native GUI shouldn't assume it's
//! already installed. So, run early in `main()`, this module makes sure a host
//! manager is present, current, and answering before the webview tries to talk to it.
//!
//! Linux logic:
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
//! Windows logic: the installer already registered the "Geph Manager" scheduled task
//! (boot-triggered, SYSTEM), so normally the manager is up before any user logs in.
//! But AV / "PC cleaner" tools sometimes delete autostart scheduled tasks outright;
//! without a repair path that breaks Geph permanently until a reinstall. When the
//! manager doesn't answer (after a short grace period for boot races), we run the
//! same orchestration with UAC in place of pkexec: explain → `ShellExecuteExW`
//! ("runas") on the sibling `geph5.exe register-manager` → wait for the named pipe
//! to answer. Since the GUI autostarts at every logon ({commonstartup} shortcut in
//! setup.iss), this heals a deleted task at the next logon or app launch.
//!
//! The orchestration (detect → dialog → elevate → result) is generic; only the
//! command wrapping and the post-install relaunch differ between native and Flatpak.
//! On Flatpak, when a fresh sandbox is needed to pick up `/run/geph`, we hand off to
//! a new instance automatically via a host-side `flatpak run`; the "reopen Geph"
//! dialog remains only as a fallback when that handoff can't be arranged.

#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::process::Command;
use std::time::Duration;

use anyhow::Context;
use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

/// Ensure the host manager is installed, current, and answering. Returns `true` if
/// the GUI should continue starting up, or `false` if it should exit now.
pub fn ensure_manager() -> bool {
    #[cfg(target_os = "linux")]
    {
        ensure_manager_linux()
    }
    #[cfg(target_os = "windows")]
    {
        ensure_manager_windows()
    }
}

/// Windows: repair a missing/dead "Geph Manager" scheduled task by re-running
/// `geph5.exe register-manager` elevated. See the module docs for why this exists.
#[cfg(target_os = "windows")]
fn ensure_manager_windows() -> bool {
    if reachable() {
        return true;
    }
    // Not answering. This might just be a race with the boot-triggered manager task
    // still starting up, so poll for a few seconds before bothering the user.
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(250));
        if reachable() {
            return true;
        }
    }

    if !explain_dialog() {
        return false; // user chose Quit
    }

    // Elevate + repair, retrying on failure until it succeeds or the user quits.
    loop {
        match do_install_windows() {
            Ok(()) => break,
            Err(err) => {
                if !error_retry_dialog(&err.to_string()) {
                    return false;
                }
            }
        }
    }

    // Wait briefly for the (re)registered manager to bind its named pipe, then
    // continue either way; the GUI surfaces its own "can't reach manager" error.
    for _ in 0..40 {
        if reachable() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    true
}

/// Run the sibling `geph5.exe register-manager` elevated via the UAC "runas" verb
/// (std `Command` cannot request elevation) and wait for it to finish. Deliberately
/// no cmd/powershell intermediary: a GUI app spawning a hidden shell is a classic
/// malware heuristic, and this binary already lives on Defender's naughty step.
#[cfg(target_os = "windows")]
fn do_install_windows() -> anyhow::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, INFINITE, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };

    // geph5.exe is installed as a sibling of the GUI executable ({app} in setup.iss).
    let geph5 = std::env::current_exe()
        .context("cannot locate our own executable")?
        .with_file_name("geph5.exe");
    anyhow::ensure!(
        geph5.exists(),
        "{} is missing; please reinstall Geph",
        geph5.display()
    );

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }
    let verb = wide("runas".as_ref());
    let file = wide(geph5.as_os_str());
    let params = wide("register-manager".as_ref());

    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    // NOCLOSEPROCESS: hand us the process handle so we can wait for the exit code.
    // NOASYNC: resolve the launch synchronously (we're not on a COM/STA UI thread).
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = 0; // SW_HIDE: geph5.exe is a console binary; don't flash a window

    let ok = unsafe { ShellExecuteExW(&mut info) };
    if ok == 0 || info.hProcess.is_null() {
        // Most commonly ERROR_CANCELLED: the user declined the UAC prompt.
        anyhow::bail!("elevation was declined or failed");
    }
    let exit_code = unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code: u32 = 1;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        code
    };
    anyhow::ensure!(exit_code == 0, "register-manager exited with code {exit_code}");
    Ok(())
}

/// Bundled (in-sandbox) locations of the host manager binaries and packaging assets.
#[cfg(target_os = "linux")]
const APP_GEPH5: &str = "/app/bin/geph5";
#[cfg(target_os = "linux")]
const APP_GEPH5_CLIENT: &str = "/app/bin/geph5-client";
#[cfg(target_os = "linux")]
const APP_LIBEXEC: &str = "/app/libexec/geph";

/// Where the manager binaries live on the host after installation.
#[cfg(target_os = "linux")]
const HOST_GEPH5: &str = "/usr/local/bin/geph5";
#[cfg(target_os = "linux")]
const HOST_GEPH5_CLIENT: &str = "/usr/local/bin/geph5-client";

/// Files staged out of the Flatpak into a host-visible dir for the privileged
/// installer. The first two are the static binaries; the rest are packaging assets.
#[cfg(target_os = "linux")]
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

/// Linux: ensure the host manager is installed, current, and answering. Returns
/// `true` if the GUI should continue starting up, or `false` if it should exit now
/// (the user quit, or a Flatpak first-run/relaunch is required so the `/run/geph`
/// bind-mount picks up the freshly-created control socket).
#[cfg(target_os = "linux")]
fn ensure_manager_linux() -> bool {
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
        if !auto_relaunch() {
            relaunch_dialog();
        }
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
        if !auto_relaunch() {
            relaunch_dialog();
        }
        return false;
    }
    // Native: continue anyway; the GUI surfaces its own "can't reach manager" error.
    true
}

/// Can we reach the manager's control socket right now?
fn reachable() -> bool {
    geph5_rt::block_on(crate::manager::manager_reachable())
}

/// On Flatpak, is the host-installed manager a different build than the one we bundle?
/// Compares content hashes (no version RPC needed). If we can't read our own bundled
/// binaries we conservatively report "not stale"; if the host copy is missing or
/// unreadable we report "stale" so it gets (re)installed.
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
fn file_sha256(path: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(hex::encode(hmac_sha256::Hash::hash(&bytes)))
}

/// SHA-256 (hex) of host files, via `flatpak-spawn --host sha256sum`. The host
/// binaries are world-readable, so the unprivileged host spawn can hash them.
/// Returns one hash per input path, in order, or `None` if any couldn't be hashed.
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
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
    // Linux elevation (pkexec) asks for a password; Windows elevation (UAC) asks
    // for confirmation. Say the right thing for each.
    let (title, body, setup, quit) = if is_chinese() {
        (
            "设置迷雾通后台服务",
            if cfg!(windows) {
                "迷雾通需要安装并启动一个以管理员权限运行的后台系统服务来管理你的连接。系统将弹出权限确认窗口。"
            } else {
                "迷雾通需要安装并启动一个以管理员权限运行的后台系统服务来管理你的连接。系统将提示你输入密码。"
            },
            "设置",
            "退出",
        )
    } else {
        (
            "Set up the Geph background service",
            if cfg!(windows) {
                "Geph needs to install and start a background system service that \
                 runs with administrator privileges to manage your connection. Your \
                 system will ask you for permission."
            } else {
                "Geph needs to install and start a background system service that \
                 runs with administrator privileges to manage your connection. Your \
                 system will prompt you for your password."
            },
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

/// Hand this session off to a fresh Flatpak instance, whose sandbox will bind-mount
/// the now-existing `/run/geph`. Returns whether the handoff was scheduled; the
/// caller must exit promptly either way, since the helper launches the new instance
/// only once this one is gone.
#[cfg(target_os = "linux")]
fn auto_relaunch() -> bool {
    let Some(app_id) = std::env::var_os("FLATPAK_ID") else {
        return false;
    };
    // The relaunch has to be driven from the host: only a brand-new `flatpak run`
    // gets a sandbox whose `/run/geph` bind-mount exists. Two timing constraints
    // shape the shell helper:
    //   * The subshell is backgrounded so the outer `sh` — and with it our
    //     flatpak-spawn child — returns immediately, instead of tethering this
    //     dying sandbox to the new instance's whole lifetime.
    //   * The new instance can only win the single-instance port (main.rs) after
    //     this process exits, and it *quits* if it loses that race. So the helper
    //     polls `flatpak ps` until our instance is gone (bounded at ~10s) before
    //     launching the replacement.
    const HANDOFF: &str = r#"(
        for _ in $(seq 40); do
            flatpak ps --columns=application 2>/dev/null | grep -qF "$1" || break
            sleep 0.25
        done
        exec flatpak run "$1" >/dev/null 2>&1
    ) &"#;
    Command::new("flatpak-spawn")
        .args(["--host", "sh", "-c", HANDOFF, "sh"])
        .arg(&app_id)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Tell the user setup is done and they should reopen Geph — fallback for when
/// `auto_relaunch` couldn't schedule the handoff (Flatpak only).
#[cfg(target_os = "linux")]
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
