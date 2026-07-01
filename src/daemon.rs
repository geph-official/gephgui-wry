//! Talks to the privileged `geph5 daemon` (the geph5-app supervisor) over
//! its loopback control protocol, instead of spawning geph5-client ourselves.
//!
//! The daemon owns the engine lifecycle: it always keeps a child geph5-client
//! running (a dry-run instance while disconnected, a real tunnel while
//! connected), so engine/broker queries forwarded through `daemon_rpc` work
//! whether or not we're connected. We only translate the GUI's lifecycle calls
//! (`start_daemon` / `stop_daemon` / `restart_daemon`) into the daemon's
//! `GephCtl` methods, and forward everything else through `daemon_rpc`.

use std::time::Duration;

use anyhow::Context;
use isocountry::CountryCode;
use nanorpc::{JrpcId, JrpcRequest, JrpcResponse};
use serde_json::{Value, json};
use smol_timeout2::TimeoutExt;

use crate::rpc::DaemonArgs;

/// The `geph daemon` control endpoint (GephCtlProtocol). A unix domain socket on
/// unix (matching the daemon's `daemon_control_path()` = `runtime_dir()`/`control.sock`);
/// a Windows named pipe on Windows (matching the daemon's `DAEMON_CONTROL_PIPE`).
///
/// `runtime_dir()` is `/run/geph` on Linux, but `/var/run/geph` on macOS (which
/// has no `/run`), so the socket path must be split per-OS to match the daemon
/// exactly — otherwise the connect attempt fails with ENOENT ("no such file or
/// directory").
#[cfg(target_os = "macos")]
const GEPH_CTL_SOCK: &str = "/var/run/geph/control.sock";
#[cfg(all(unix, not(target_os = "macos")))]
const GEPH_CTL_SOCK: &str = "/run/geph/control.sock";
#[cfg(windows)]
const GEPH_CTL_PIPE: &str = r"\\.\pipe\geph-daemon-control";

/// Low-level: send one `GephCtl` JSON-RPC request to the daemon and read the reply.
async fn ctl_call(req: JrpcRequest) -> anyhow::Result<JrpcResponse> {
    let line = format!("{}\n", serde_json::to_string(&req)?);
    let resp = transact(line).await?;
    Ok(serde_json::from_str(&resp)?)
}

/// Send one newline-terminated request line to the daemon's control endpoint and
/// return its one-line reply. The transport is runtime-native: gephgui runs on
/// smolscale (no tokio reactor), so unix uses a smol unix socket directly, and
/// Windows does a blocking Win32 named-pipe exchange off-thread via
/// `smol::unblock` (sillad's pipe is tokio-based and would need a tokio runtime).
#[cfg(unix)]
async fn transact(req_line: String) -> anyhow::Result<String> {
    use futures_util::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, io::BufReader};
    let conn = smol::net::unix::UnixStream::connect(GEPH_CTL_SOCK)
        .timeout(Duration::from_millis(500))
        .await
        .context("timed out connecting to geph daemon")??;
    let (read, mut write) = conn.split();
    write.write_all(req_line.as_bytes()).await?;
    let mut read = BufReader::new(read);
    let mut buf = String::new();
    read.read_line(&mut buf).await?;
    Ok(buf)
}

#[cfg(windows)]
async fn transact(req_line: String) -> anyhow::Result<String> {
    smol::unblock(move || windows_pipe::exchange(GEPH_CTL_PIPE, &req_line)).await
}

/// Blocking Win32 named-pipe client. The control protocol is a single
/// newline-terminated request and reply, and the pipe path opens as an ordinary
/// duplex `std::fs::File`, so a plain blocking open/write/read is enough — run
/// off the executor via `smol::unblock`.
#[cfg(windows)]
mod windows_pipe {
    use std::{
        fs::{File, OpenOptions},
        io::{BufRead, BufReader, Write},
        thread,
        time::{Duration, Instant},
    };

    /// `ERROR_PIPE_BUSY`: every pipe instance is momentarily in use; retry briefly.
    const ERROR_PIPE_BUSY: i32 = 231;

    pub fn exchange(pipe: &str, req_line: &str) -> anyhow::Result<String> {
        let file = open(pipe)?;
        // `&File` implements both `Write` and `Read`, so one duplex handle carries
        // the request and the reply.
        (&file).write_all(req_line.as_bytes())?;
        (&file).flush()?;
        let mut reader = BufReader::new(&file);
        let mut buf = String::new();
        reader.read_line(&mut buf)?;
        Ok(buf)
    }

    fn open(pipe: &str) -> anyhow::Result<File> {
        let deadline = Instant::now() + Duration::from_millis(2000);
        loop {
            match OpenOptions::new().read(true).write(true).open(pipe) {
                Ok(f) => return Ok(f),
                Err(e)
                    if e.raw_os_error() == Some(ERROR_PIPE_BUSY)
                        && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    return Err(anyhow::Error::new(e).context("connecting to geph daemon pipe"));
                }
            }
        }
    }
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
/// full-tunnel VPN mode, auto-proxy, allow-direct) without yet connecting.
/// Shared by start/restart.
async fn push_settings(args: &DaemonArgs) -> anyhow::Result<()> {
    // login persists + validates the secret; harmless if unchanged.
    geph_ctl("login", vec![json!(args.secret)]).await?;
    geph_ctl(
        "set_exit_constraint",
        vec![exit_constraint_value(&args.exit)?],
    )
    .await?;
    // Full-tunnel VPN mode (vs. local proxy). Must be pushed before connect so
    // the daemon brings the tunnel up in the right mode.
    geph_ctl("set_vpn_mode", vec![json!(args.global_vpn)]).await?;
    geph_ctl("set_auto_proxy", vec![json!(args.proxy_autoconf), session()]).await?;
    geph_ctl("set_allow_direct", vec![json!(args.allow_direct)]).await?;
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

/// Reconnect using the daemon's already-persisted secret + exit constraint, with
/// no `DaemonArgs` from the JS UI. This is what the tray "Connect" action uses:
/// the daemon keeps the last-used settings, so a bare `connect` brings the tunnel
/// back up exactly as the user last had it.
pub async fn reconnect_daemon() -> anyhow::Result<()> {
    geph_ctl("connect", vec![session()]).await?;
    Ok(())
}

/// Switch the exit constraint. The daemon persists it and, if currently
/// connected, reconnects to the new exit WITHOUT a leak window (the kill switch
/// stays up; only the engine child is restarted).
pub async fn set_exit_constraint(exit: &crate::rpc::ExitConstraint) -> anyhow::Result<()> {
    geph_ctl("set_exit_constraint", vec![exit_constraint_value(exit)?]).await?;
    Ok(())
}

/// Whether the daemon's control endpoint is up and answering at all (regardless of
/// connection state). Used by the startup bootstrap to decide whether the host
/// daemon needs to be installed/started.
#[cfg(unix)]
pub async fn daemon_reachable() -> bool {
    geph_ctl("get_settings", vec![]).await.is_ok()
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
