use crate::{
    daemon::{start_binder_proxy, sync_status, DaemonConfig},
    pac::{configure_proxy, deconfigure_proxy},
};
use anyhow::Context;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::Deserialize;
use slab::Slab;
use tide::convert::{DeserializeOwned, Serialize};
use wry::application::dpi::LogicalSize;
use wry::{
    application::window::Window,
    webview::{RpcRequest, RpcResponse},
};

/// JSON-RPC interface that talks to JavaScript.
#[tracing::instrument]
pub fn global_rpc_handler(window: &Window, req: RpcRequest) -> Option<RpcResponse> {
    tracing::debug!(req = format!("{:?}", req).as_str(), "received RPC request");
    match req.method.as_str() {
        "echo" => handle_rpc(req, handle_echo),
        "start_daemon" => handle_rpc(req, handle_start_daemon),
        "stop_daemon" => handle_rpc(req, handle_stop_daemon),
        "start_binder_proxy" => handle_rpc(req, handle_start_binder_proxy),
        "stop_binder_proxy" => handle_rpc(req, handle_stop_binder_proxy),
        "start_sync_status" => handle_rpc(req, handle_start_sync_status),
        "check_sync_status" => handle_rpc(req, handle_check_sync_status),
        "set_conversion_factor" => {
            handle_rpc(req, |params| handle_set_conversion_factor(window, params))
        }
        "get_url" => handle_rpc(req, handle_get_url),
        other => {
            tracing::error!("unrecognized RPC verb {}", other);
            None
        }
    }
}

fn handle_echo(params: (String,)) -> anyhow::Result<String> {
    Ok(params.0)
}

#[derive(Deserialize, Debug)]
struct DaemonConfigPlus {
    #[serde(flatten)]
    daemon_conf: DaemonConfig,
    autoproxy: bool,
}

pub type DeathBox = Mutex<Option<DeathBoxInner>>;
pub type DeathBoxInner = Box<dyn FnOnce() -> anyhow::Result<()> + Send + Sync + 'static>;

static RUNNING_DAEMON: Lazy<DeathBox> = Lazy::new(Default::default);

/// Handles a request to start the daemon
#[tracing::instrument]
fn handle_start_daemon(params: (DaemonConfigPlus,)) -> anyhow::Result<String> {
    let params = params.0;
    let mut rd = RUNNING_DAEMON.lock();
    if rd.is_none() {
        let mut daemon = params.daemon_conf.start()?;
        if params.autoproxy {
            configure_proxy()?;
        }
        let autoproxy = params.autoproxy;
        *rd = Some(daemon);
    }
    Ok("".into())
}

/// Handles a request to stop the daemon
#[tracing::instrument]
fn handle_stop_daemon(_: [u8; 0]) -> anyhow::Result<String> {
    if let Some(killfun) = RUNNING_DAEMON.lock().take() {
        tracing::warn!("running the killfun");
        killfun()?;
    }
    if let Err(err) = deconfigure_proxy() {
        tracing::error!("cannot deconfigure proxy: {:?}", err);
    }
    Ok("".into())
}

static RUNNING_BINDPROX: Lazy<DeathBox> = Lazy::new(Default::default);

/// Handles a request to start the binder proxy.
#[tracing::instrument]
fn handle_start_binder_proxy(_: [u8; 0]) -> anyhow::Result<String> {
    let mut rd = RUNNING_BINDPROX.lock();
    if rd.is_none() {
        let mut proc = start_binder_proxy()?;
        *rd = Some(Box::new(move || {
            proc.kill()?;
            proc.wait()?;
            Ok(())
        }));
    }
    Ok("".into())
}

/// Handles a request to stop the binder proxy.
#[tracing::instrument]
fn handle_stop_binder_proxy(_: [u8; 0]) -> anyhow::Result<String> {
    if let Some(killfun) = RUNNING_BINDPROX.lock().take() {
        killfun()?;
    }
    Ok("".into())
}

enum SyncStatus {
    Pending,
    Error(String),
    Done(serde_json::Value),
}

static SYNC_STATUS_SLAB: Lazy<Mutex<Slab<SyncStatus>>> = Lazy::new(Default::default);

/// Handles a request to start syncing the status.
#[tracing::instrument]
fn handle_start_sync_status(args: (String, String, bool)) -> anyhow::Result<usize> {
    let (username, password, force) = args;
    let mut slab = SYNC_STATUS_SLAB.lock();
    let idx = slab.insert(SyncStatus::Pending);
    std::thread::spawn(move || match sync_status(username, password, force) {
        Ok(res) => SYNC_STATUS_SLAB.lock()[idx] = SyncStatus::Done(res),
        Err(err) => SYNC_STATUS_SLAB.lock()[idx] = SyncStatus::Error(err.to_string()),
    });
    Ok(idx)
}

/// Handles a request for the status of some sync.
#[tracing::instrument]
fn handle_check_sync_status(args: (usize,)) -> anyhow::Result<Option<serde_json::Value>> {
    let slab = SYNC_STATUS_SLAB.lock();
    match slab.get(args.0).context("no such id")? {
        SyncStatus::Done(val) => Ok(Some(val.clone())),
        SyncStatus::Error(err) => anyhow::bail!(err.clone()),
        SyncStatus::Pending => Ok(None),
    }
}

/// Handles a request to change DPI on, say, GTK platforms with pseudo-hidpi through font size changes.
#[tracing::instrument]
fn handle_set_conversion_factor(window: &Window, params: (f64,)) -> anyhow::Result<String> {
    let factor = params.0;
    tracing::debug!(factor);
    window.set_resizable(true);
    window.set_inner_size(LogicalSize {
        width: 400.0 * factor,
        height: 610.0 * factor,
    });
    window.set_resizable(false);
    Ok("".into())
}

/// Handles a request to poll a particular URL
#[tracing::instrument]
fn handle_get_url(params: (String,)) -> anyhow::Result<String> {
    smol::future::block_on(async move {
        let mut resp = surf::get(params.0).await.map_err(|e| e.into_inner())?;
        resp.body_string().await.map_err(|e| e.into_inner())
    })
}

fn handle_rpc<I: DeserializeOwned, O: Serialize, F: FnOnce(I) -> anyhow::Result<O>>(
    req: RpcRequest,
    f: F,
) -> Option<RpcResponse> {
    let input: Result<I, _> = serde_json::from_value(req.params?);
    match input {
        Err(err) => {
            let err = format!("{:?}", err);
            tracing::error!(
                method = req.method.as_str(),
                err = err.as_str(),
                "invalid input to RPC call"
            );
            Some(RpcResponse::new_error(
                req.id,
                Some(serde_json::to_value(err).unwrap()),
            ))
        }
        Ok(res) => match f(res) {
            Err(err) => {
                let err = format!("{:?}", err);
                tracing::error!(
                    method = req.method.as_str(),
                    err = err.as_str(),
                    "RPC call returned error"
                );
                Some(RpcResponse::new_error(
                    req.id,
                    Some(serde_json::to_value(err).unwrap()),
                ))
            }
            Ok(res) => Some(RpcResponse::new_result(
                req.id,
                Some(serde_json::to_value(res).unwrap()),
            )),
        },
    }
}
