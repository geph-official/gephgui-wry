use std::{
    io::{Read, Write},
    process::{Command, Stdio},
};

use crate::{daemon::DaemonConfig, mtbus::mt_enqueue, pac::configure_proxy};
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
    tracing::debug!(req = format!("{:?}", req).as_str(), "received RPC request");
    std::thread::spawn(move || {
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
            other => {
                panic!("unrecognized RPC verb {}", other);
            }
        };
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

pub type DeathBox = Mutex<Option<DeathBoxInner>>;
pub type DeathBoxInner = Box<dyn FnOnce() -> anyhow::Result<()> + Send + Sync + 'static>;

pub static RUNNING_DAEMON: Lazy<DeathBox> = Lazy::new(Default::default);

fn handle_sync(params: (String, String)) -> anyhow::Result<String> {
    let (username, password) = params;
    let mut child = Command::new("geph4-client")
        .arg("sync")
        .arg("--username")
        .arg(username)
        .arg("--password")
        .arg(password)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut e = String::new();
    child.stderr.take().unwrap().read_to_string(&mut e)?;
    let mut s = String::new();
    child.stdout.take().unwrap().read_to_string(&mut s)?;
    if !s.contains('{') {
        anyhow::bail!(e.lines().last().map(|e| e.to_string()).context("lell")?)
    }

    eprintln!("ess: {s}");
    Ok(s)
}

fn handle_daemon_rpc(params: (String,)) -> anyhow::Result<String> {
    Ok(ureq::post("http://127.0.0.1:9809")
        .send_string(&params.0)?
        .into_string()?)
}

fn handle_binder_rpc(params: (String,)) -> anyhow::Result<String> {
    let params = params.0;
    // TODO cache this child process
    let mut child = Command::new("geph4-client")
        .arg("binder-proxy")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    eprintln!("params: {params}");
    let mut stdin = child.stdin.take().unwrap();
    std::thread::spawn(move || {
        let _ = stdin.write_all(params.as_bytes());
        let _ = stdin.write_all(b"\n");
    });
    let mut s = String::new();
    child.stdout.take().unwrap().read_to_string(&mut s)?;
    eprintln!("{}", s);
    Ok(s)
}

/// Handles a request to start the daemon
fn handle_start_daemon(params: (DaemonConfigPlus,)) -> anyhow::Result<String> {
    let params = params.0;
    if params.proxy_autoconf {
        configure_proxy().context("cannot configure proxy")?;
    }
    let mut rd = RUNNING_DAEMON.lock();
    if rd.is_none() {
        let daemon = params.daemon_conf.start().context("cannot start daemon")?;
        *rd = Some(daemon);
    }
    Ok("".into())
}

/// Handles a request to stop the daemon
fn handle_stop_daemon(_: Vec<serde_json::Value>) -> anyhow::Result<String> {
    let mut rd = RUNNING_DAEMON.lock();
    if let Some(rd) = rd.take() {
        eprintln!("***** STOPPING DAEMON *****");
        rd()?;
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
            width: 400.0 * factor,
            height: 610.0 * factor,
        });
        webview.window().set_resizable(false);
    });
    Ok("".into())
}

/// Handles a request to poll a particular URL
#[tracing::instrument]
fn handle_get_url(params: (String,)) -> anyhow::Result<String> {
    Ok(ureq::get(&params.0).call()?.into_string()?)
}

/// Handles a request to open the browser
#[tracing::instrument]
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
                tracing::error!(
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
