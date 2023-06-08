use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::{
    daemon::{debugpack_path, DaemonConfig, DAEMON_VERSION, GEPH_RPC_KEY},
    mtbus::mt_enqueue,
    pac::{configure_proxy, deconfigure_proxy},
    WINDOW_HEIGHT, WINDOW_WIDTH,
};
use anyhow::Context;

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::Deserialize;

use tide::convert::{DeserializeOwned, Serialize};
use wry::application::dpi::LogicalSize;
use wry::{
    application::window::Window,
    webview::{RpcRequest, RpcResponse},
};

/// JSON-RPC interface that talks to JavaScript.
pub fn global_rpc_handler(_window: &Window, req: RpcRequest) -> Option<RpcResponse> {
    tracing::trace!(req = format!("{:?}", req).as_str(), "received RPC request");
    std::thread::spawn(move || {
        let start = Instant::now();
        let method = req.method.clone();
        let result = match req.method.as_str() {
            "echo" => handle_rpc(req, handle_echo),
            "binder_rpc" => handle_rpc(req, handle_binder_rpc),
            "daemon_rpc" => handle_rpc(req, handle_daemon_rpc),
            "sync" => handle_rpc(req, handle_sync),
            "start_daemon" => handle_rpc(req, handle_start_daemon),
            "stop_daemon" => handle_rpc(req, handle_stop_daemon),
            "set_conversion_factor" => handle_rpc(req, handle_set_conversion_factor),
            "get_url" => handle_rpc(req, handle_get_url),

            "open_browser" => handle_rpc(req, handle_open_browser),
            "export_logs" => handle_rpc(req, handle_export_logs),
            "version" => handle_rpc(req, |_: Vec<serde_json::Value>| Ok(DAEMON_VERSION.clone())),
            other => {
                panic!("unrecognized RPC verb {}", other);
            }
        };
        tracing::debug!("{method} took {:?}", start.elapsed());
        mt_enqueue(move |wv| wv.evaluate_script(&result).unwrap());
    });
    None
}

fn handle_echo(params: (String,)) -> anyhow::Result<String> {
    Ok(params.0)
}

#[derive(Deserialize, Debug)]
struct DaemonConfigPlus {
    #[serde(flatten)]
    daemon_conf: DaemonConfig,
    proxy_autoconf: bool,
}

pub type DeathBox = Mutex<Option<std::process::Child>>;

pub static RUNNING_DAEMON: Lazy<DeathBox> = Lazy::new(Default::default);

fn handle_sync(params: (String, String, bool)) -> anyhow::Result<String> {
    println!("handle_sync {:?}", params);
    let (username, password, force) = params;
    let mut cmd = Command::new("geph4-client");
    cmd.arg("sync")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if force {
        cmd.arg("--force");
    }
    cmd.arg("auth-password")
        .arg("--username")
        .arg(username)
        .arg("--password")
        .arg(password);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    let mut child = cmd.spawn()?;
    let mut e = String::new();
    child.stderr.take().unwrap().read_to_string(&mut e)?;
    let mut s = String::new();
    child.stdout.take().unwrap().read_to_string(&mut s)?;
    child.wait()?;
    if !s.contains('{') {
        anyhow::bail!(e
            .lines()
            .last()
            .map(|e| e.to_string())
            .context("cannot read anything from sync")?)
    }

    Ok(s)
}

fn handle_daemon_rpc(params: (String,)) -> anyhow::Result<String> {
    Ok(
        ureq::post(&format!("http://127.0.0.1:9809/{}", GEPH_RPC_KEY.clone()))
            .send_string(&params.0)?
            .into_string()?,
    )
}

fn handle_binder_rpc(params: (String,)) -> anyhow::Result<String> {
    let params = params.0;
    // TODO cache this child process
    let mut cmd = Command::new("geph4-client");
    cmd.arg("binder-proxy")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    let mut child = cmd.spawn()?;
    eprintln!("params: {params}");
    let mut stdin = child.stdin.take().unwrap();
    std::thread::spawn(move || {
        let _ = stdin.write_all(params.as_bytes());
        let _ = stdin.write_all(b"\n");
    });
    let mut s = String::new();
    child.stdout.take().unwrap().read_to_string(&mut s)?;
    eprintln!("{}", s);
    child.wait()?;
    Ok(s)
}

static PROXY_CONFIGURED: AtomicBool = AtomicBool::new(false);

/// Handles a request to start the daemon
fn handle_start_daemon(params: (DaemonConfigPlus,)) -> anyhow::Result<String> {
    let params = params.0;
    if params.proxy_autoconf && !params.daemon_conf.vpn_mode {
        // configure_proxy().context("cannot configure proxy")?;
        PROXY_CONFIGURED.store(true, Ordering::SeqCst);
    }
    let mut rd = RUNNING_DAEMON.lock();
    if rd.is_none() {
        let daemon = params.daemon_conf.start().context("cannot start daemon")?;
        *rd = Some(daemon);
    }
    std::thread::spawn(move || loop {
        {
            let mut daemon = RUNNING_DAEMON.lock();
            if let Some(d) = daemon.as_mut() {
                if let Ok(Some(_)) = d.try_wait() {
                    std::thread::spawn(move || handle_stop_daemon(vec![]));
                }
            } else {
                return;
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    });
    Ok("".into())
}

/// Handles a request to stop the daemon
fn handle_stop_daemon(_: Vec<serde_json::Value>) -> anyhow::Result<String> {
    let mut rd = RUNNING_DAEMON.lock();
    if let Some(mut rd) = rd.take() {
        eprintln!("***** STOPPING DAEMON *****");
        rd.kill()?;
        rd.wait()?;
    }
    if PROXY_CONFIGURED.swap(false, Ordering::SeqCst) {
        deconfigure_proxy()?;
    }
    Ok("".into())
}

/// Handles a request to change DPI on, say, GTK platforms with pseudo-hidpi through font size changes.
#[tracing::instrument]
fn handle_set_conversion_factor(params: (f64,)) -> anyhow::Result<String> {
    let factor = params.0;
    tracing::debug!(factor);
    mt_enqueue(move |webview| {
        webview.window().set_resizable(true);
        webview.window().set_inner_size(LogicalSize {
            width: WINDOW_WIDTH as f64 * factor,
            height: WINDOW_HEIGHT as f64 * factor,
        });
        webview.window().set_resizable(false);
    });
    Ok("".into())
}

/// Handles a request to poll a particular URL
fn handle_get_url(params: (String,)) -> anyhow::Result<String> {
    Ok(ureq::get(&params.0).call()?.into_string()?)
}

/// Handles a request to open the browser
fn handle_open_browser(params: (String,)) -> anyhow::Result<String> {
    let _ = webbrowser::open(&params.0);
    Ok("".into())
}

fn handle_rpc<I: DeserializeOwned, O: Serialize, F: FnOnce(I) -> anyhow::Result<O>>(
    req: RpcRequest,
    f: F,
) -> String {
    let input: Result<I, _> = serde_json::from_value(req.params.unwrap());
    match input {
        Err(err) => {
            let err = format!("{:?}", err);
            tracing::error!(
                method = req.method.as_str(),
                err = err.as_str(),
                "invalid input to RPC call"
            );
            RpcResponse::get_error_script(req.id.unwrap(), serde_json::to_value(err).unwrap())
                .unwrap()
        }
        Ok(res) => match f(res) {
            Err(err) => {
                let err = format!("{:?}", err);
                tracing::trace!(
                    method = req.method.as_str(),
                    err = err.as_str(),
                    "RPC call returned error"
                );
                RpcResponse::get_error_script(req.id.unwrap(), serde_json::to_value(err).unwrap())
                    .unwrap()
            }
            Ok(res) => {
                RpcResponse::get_result_script(req.id.unwrap(), serde_json::to_value(res).unwrap())
                    .unwrap()
            }
        },
    }
}

fn handle_export_logs(_: Vec<serde_json::Value>) -> anyhow::Result<String> {
    let save_to = rfd::AsyncFileDialog::new()
        .set_file_name(&format!(
            "geph4-logs-export-{}.db",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
        ))
        .save_file();
    smolscale::spawn(async move {
        if let Some(save_to) = save_to.await {
            let mut cmd = Command::new("geph4-client");
            cmd.arg("debugpack")
                .arg("--debugpack-path")
                .arg(debugpack_path())
                .arg("--export-to")
                .arg(save_to.path())
                .stdin(Stdio::piped())
                .stderr(Stdio::piped());

            let mut child = cmd.spawn()?;
            let mut e = String::new();
            child.stderr.take().unwrap().read_to_string(&mut e)?;
            child.wait()?;
            if !e.is_empty() {
                anyhow::bail!(e
                    .lines()
                    .last()
                    .map(|e| e.to_string())
                    .context("export logs FAIL")?)
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .detach();
    Ok("".into())
}
