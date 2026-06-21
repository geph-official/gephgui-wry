//! Talks to the privileged `geph daemon` (the geph5-client-cli supervisor) over
//! its loopback control protocol, instead of spawning geph5-client ourselves.
//!
//! The daemon owns the engine lifecycle: it always keeps a child geph5-client
//! running (a dry-run instance while disconnected, a real tunnel while
//! connected), so engine/broker queries forwarded through `daemon_rpc` work
//! whether or not we're connected. We only translate the GUI's lifecycle calls
//! (`start_daemon` / `stop_daemon` / `restart_daemon`) into the daemon's
//! `GephCtl` methods, and forward everything else through `daemon_rpc`.

use std::{
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};

use anyhow::Context;
use futures_util::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, io::BufReader};
use isocountry::CountryCode;
use nanorpc::{JrpcId, JrpcRequest, JrpcResponse};
use serde_json::{Value, json};
use smol::net::TcpStream;
use smol_timeout2::TimeoutExt;

use crate::rpc::DaemonArgs;

/// The `geph daemon` control endpoint (GephCtlProtocol).
const GEPH_CTL_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 28080);

/// Low-level: send one `GephCtl` JSON-RPC request to the daemon and read the reply.
async fn ctl_call(req: JrpcRequest) -> anyhow::Result<JrpcResponse> {
    let conn = TcpStream::connect(GEPH_CTL_ADDR)
        .timeout(Duration::from_millis(500))
        .await
        .context("timed out connecting to geph daemon")??;
    let (read, mut write) = conn.split();
    write
        .write_all(format!("{}\n", serde_json::to_string(&req)?).as_bytes())
        .await?;
    let mut read = BufReader::new(read);
    let mut buf = String::new();
    read.read_line(&mut buf).await?;
    Ok(serde_json::from_str(&buf)?)
}

/// Call a `GephCtl` method, unwrapping its `Result<_, String>` into the inner value.
async fn geph_ctl(method: &str, params: Vec<Value>) -> anyhow::Result<Value> {
    let req = JrpcRequest {
        jsonrpc: "2.0".into(),
        method: method.into(),
        params,
        id: JrpcId::Number(1),
    };
    let resp = ctl_call(req)
        .timeout(Duration::from_secs(60))
        .await
        .context("geph daemon call timed out")??;
    if let Some(err) = resp.error {
        anyhow::bail!("{}", err.message);
    }
    Ok(resp.result.unwrap_or(Value::Null))
}

/// The calling desktop session, so the (root) daemon configures *our* proxy.
/// This is just identity — uid plus a few env vars; the proxy logic is the
/// daemon's.
fn session() -> Value {
    #[cfg(unix)]
    {
        json!({
            "uid": unsafe { libc::geteuid() },
            "gid": unsafe { libc::getegid() },
            "home": std::env::var("HOME").ok(),
            "dbus_session_bus_address": std::env::var("DBUS_SESSION_BUS_ADDRESS").ok(),
            "xdg_runtime_dir": std::env::var("XDG_RUNTIME_DIR").ok(),
        })
    }
    #[cfg(not(unix))]
    {
        json!({ "uid": 0 })
    }
}

/// Translate the GUI's exit selection into a geph `ExitConstraint` JSON value.
fn exit_constraint_value(exit: &crate::rpc::ExitConstraint) -> anyhow::Result<Value> {
    let constraint = match exit {
        crate::rpc::ExitConstraint::Auto => geph5_broker_protocol::ExitConstraint::Auto,
        crate::rpc::ExitConstraint::Manual { city, country } => {
            geph5_broker_protocol::ExitConstraint::CountryCity(
                CountryCode::for_alpha2(country)
                    .map_err(|_| anyhow::anyhow!("bad country code {country}"))?,
                city.clone(),
            )
        }
    };
    Ok(serde_json::to_value(constraint)?)
}

/// Push the GUI's `DaemonArgs` into the daemon's settings (secret, exit,
/// auto-proxy) without yet connecting. Shared by start/restart.
async fn push_settings(args: &DaemonArgs) -> anyhow::Result<()> {
    // login persists + validates the secret; harmless if unchanged.
    geph_ctl("login", vec![json!(args.secret)]).await?;
    geph_ctl(
        "set_exit_constraint",
        vec![exit_constraint_value(&args.exit)?],
    )
    .await?;
    geph_ctl("set_auto_proxy", vec![json!(args.proxy_autoconf), session()]).await?;
    Ok(())
}

pub async fn start_daemon(args: DaemonArgs) -> anyhow::Result<()> {
    push_settings(&args).await?;
    geph_ctl("connect", vec![session()]).await?;
    Ok(())
}

pub async fn restart_daemon(args: DaemonArgs) -> anyhow::Result<()> {
    // The daemon is persistent; "restart" is just pushing new settings and
    // (re)connecting. connect() restarts the child with the new exit.
    push_settings(&args).await?;
    geph_ctl("connect", vec![session()]).await?;
    Ok(())
}

pub async fn stop_daemon() -> anyhow::Result<()> {
    geph_ctl("disconnect", vec![session()]).await?;
    Ok(())
}

/// Whether the user currently wants the tunnel up (mirrors the old "is the
/// daemon process running" semantics, which only existed while connected).
pub async fn daemon_running() -> bool {
    match geph_ctl("get_settings", vec![]).await {
        Ok(v) => v
            .get("connected")
            .and_then(|c| c.as_bool())
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// Forward a raw engine RPC (`conn_info`, `broker_rpc`, `net_status`,
/// `stat_history`, `recent_logs`, `start_registration`, …) to the daemon, which
/// relays it to its always-running child geph5-client. This is what makes
/// broker/engine calls work whether or not we're connected.
pub async fn daemon_rpc(inner: JrpcRequest) -> anyhow::Result<JrpcResponse> {
    let req = JrpcRequest {
        jsonrpc: "2.0".into(),
        method: "daemon_rpc".into(),
        params: vec![json!(inner.method), Value::Array(inner.params)],
        id: inner.id.clone(),
    };
    let mut resp = ctl_call(req)
        .timeout(Duration::from_secs(10))
        .await
        .context("daemon_rpc timed out")??;
    // The daemon's `daemon_rpc` result/error already reflects the inner call.
    resp.id = inner.id;
    Ok(resp)
}
