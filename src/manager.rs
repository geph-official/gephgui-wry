//! Talks to the privileged `geph5 manager` (the geph5-app supervisor) over
//! its control protocol, instead of spawning geph5-client ourselves.
//!
//! The manager owns the engine lifecycle: it always keeps a child geph5-client
//! running (a dry-run instance while disconnected, a real tunnel while
//! connected), so engine/broker queries forwarded through `daemon_rpc` work
//! whether or not we're connected. We only translate the GUI's lifecycle calls
//! (`start_daemon` / `stop_daemon` / `restart_daemon`) into the manager's
//! `GephCtl` methods, and forward everything else through `daemon_rpc`.
//!
//! Both the protocol (`GephCtlProtocol`) and the transport dialing the
//! manager's control endpoint (unix socket / Windows named pipe) come from
//! `geph5_misc_rpc::manager_control` — the same definitions the manager and the
//! `geph` CLI compile against, so the endpoint and the wire types cannot drift.

use std::{future::Future, sync::LazyLock, time::Duration};

use anyhow::Context;
use geph5_broker_protocol::ExitConstraint;
use geph5_misc_rpc::manager_control::{self, GephCtlClient, GephCtlError, SessionContext};
use geph5_rt::TimeoutExt;
use isocountry::CountryCode;
use nanorpc::{JrpcRequest, JrpcResponse, RpcTransport};
use serde_json::{Value, json};

use crate::rpc::DaemonArgs;

/// The shared typed client pointed at the running manager. Each call dials a
/// fresh connection (the transport has no pooling); this just avoids rebuilding
/// the wrapper.
fn client() -> &'static GephCtlClient {
    static CLIENT: LazyLock<GephCtlClient> = LazyLock::new(manager_control::manager_control_client);
    &CLIENT
}

/// Await a `GephCtl` call with a timeout, flattening the transport and
/// application error layers into one `anyhow` error.
async fn ctl<T>(
    fut: impl Future<Output = Result<Result<T, String>, GephCtlError<anyhow::Error>>>,
) -> anyhow::Result<T> {
    match fut
        .timeout(Duration::from_secs(60))
        .await
        .context("geph manager call timed out")?
    {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(msg)) => Err(anyhow::anyhow!(msg)),
        Err(e) => Err(anyhow::anyhow!("could not reach the geph manager: {e:?}")),
    }
}

/// The calling desktop session, so the (root) manager configures *our* proxy.
/// This is just identity — uid plus a few env vars; the proxy logic is the
/// manager's.
fn session() -> SessionContext {
    #[cfg(unix)]
    {
        SessionContext {
            uid: unsafe { libc::geteuid() },
            gid: Some(unsafe { libc::getegid() }),
            home: std::env::var("HOME").ok(),
            dbus_session_bus_address: std::env::var("DBUS_SESSION_BUS_ADDRESS").ok(),
            xdg_runtime_dir: std::env::var("XDG_RUNTIME_DIR").ok(),
        }
    }
    #[cfg(not(unix))]
    {
        SessionContext::default()
    }
}

/// Translate the GUI's exit selection into a geph `ExitConstraint`.
fn exit_constraint(exit: &crate::rpc::ExitConstraint) -> anyhow::Result<ExitConstraint> {
    Ok(match exit {
        crate::rpc::ExitConstraint::Auto => ExitConstraint::Auto,
        crate::rpc::ExitConstraint::Manual { city, country } => ExitConstraint::CountryCity(
            CountryCode::for_alpha2(country)
                .map_err(|_| anyhow::anyhow!("bad country code {country}"))?,
            city.clone(),
        ),
    })
}

/// Push the GUI's `DaemonArgs` into the manager's settings (secret, exit,
/// full-tunnel VPN mode, local-proxy config, LAN access, and allow-direct) without
/// yet connecting. Shared by start/restart.
async fn push_settings(args: &DaemonArgs) -> anyhow::Result<()> {
    let c = client();
    // login persists + validates the secret; harmless if unchanged.
    ctl(c.login(args.secret.clone())).await?;
    ctl(c.set_exit_constraint(exit_constraint(&args.exit)?)).await?;
    // Full-tunnel VPN mode (vs. local proxy). Must be pushed before connect so
    // the manager brings the tunnel up in the right mode.
    ctl(c.set_vpn_mode(args.global_vpn)).await?;
    // Local-proxy config; `None` means the manager listens on no ports at all.
    ctl(c.set_proxy_settings(args.proxy.clone(), session())).await?;
    ctl(c.set_allow_lan(args.allow_lan)).await?;
    ctl(c.set_allow_direct(args.allow_direct)).await?;
    Ok(())
}

pub async fn start_daemon(args: DaemonArgs) -> anyhow::Result<()> {
    push_settings(&args).await?;
    ctl(client().connect(session())).await?;
    Ok(())
}

pub async fn restart_daemon(args: DaemonArgs) -> anyhow::Result<()> {
    // The manager is persistent; "restart" is just pushing new settings and
    // (re)connecting. connect() restarts the child with the new exit.
    push_settings(&args).await?;
    ctl(client().connect(session())).await?;
    Ok(())
}

pub async fn stop_daemon() -> anyhow::Result<()> {
    ctl(client().disconnect(session())).await?;
    Ok(())
}

/// Reconnect using the manager's already-persisted secret + exit constraint, with
/// no `DaemonArgs` from the JS UI. This is what the tray "Connect" action uses:
/// the manager keeps the last-used settings, so a bare `connect` brings the tunnel
/// back up exactly as the user last had it.
pub async fn reconnect() -> anyhow::Result<()> {
    ctl(client().connect(session())).await?;
    Ok(())
}

/// Switch the exit constraint. The manager persists it and, if currently
/// connected, reconnects to the new exit WITHOUT a leak window (the kill switch
/// stays up; only the engine child is restarted).
pub async fn set_exit_constraint(exit: &crate::rpc::ExitConstraint) -> anyhow::Result<()> {
    ctl(client().set_exit_constraint(exit_constraint(exit)?)).await?;
    Ok(())
}

/// Whether the manager's control endpoint is up and answering at all (regardless
/// of connection state). Used by the startup bootstrap to decide whether the host
/// manager needs to be installed/started. Short timeout: this is polled.
#[cfg(unix)]
pub async fn manager_reachable() -> bool {
    (client().get_settings().timeout(Duration::from_secs(2)).await)
        .is_some_and(|r| matches!(r, Ok(Ok(_))))
}

/// Whether the user currently wants the tunnel up (mirrors the old "is the
/// manager process running" semantics, which only existed while connected).
/// Short timeout: the tray polls this once a second.
pub async fn manager_connected() -> bool {
    match client().get_settings().timeout(Duration::from_secs(2)).await {
        Some(Ok(Ok(settings))) => settings.connected,
        _ => false,
    }
}

/// Forward a raw engine RPC (`conn_info`, `broker_rpc`, `net_status`,
/// `stat_history`, `recent_logs`, `start_registration`, …) to the manager, which
/// relays it to its always-running child geph5-client. This is what makes
/// broker/engine calls work whether or not we're connected.
pub async fn daemon_rpc(inner: JrpcRequest) -> anyhow::Result<JrpcResponse> {
    let req = JrpcRequest {
        jsonrpc: "2.0".into(),
        method: "daemon_rpc".into(),
        params: vec![json!(inner.method), Value::Array(inner.params)],
        id: inner.id.clone(),
    };
    let mut resp = manager_control::manager_control_transport()
        .call_raw(req)
        .timeout(Duration::from_secs(10))
        .await
        .context("daemon_rpc timed out")??;
    // The manager's `daemon_rpc` result/error already reflects the inner call.
    resp.id = inner.id;
    Ok(resp)
}
