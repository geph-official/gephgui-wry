use std::{
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    process::Command,
    sync::LazyLock,
    time::Duration,
};

use anyhow::Context;
use futures_util::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, io::BufReader};
use isocountry::CountryCode;
use nanorpc::{JrpcId, JrpcRequest, JrpcResponse, RpcTransport};
use oneshot::Receiver as OneshotReceiver;
use oneshot::channel as oneshot_channel;
use smol::future::FutureExt as SmolFutureExt;
use smol::net::TcpStream;
use smol_timeout2::TimeoutExt;
use std::process::Stdio;
use tap::Tap;
use tempfile::NamedTempFile;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::{
    pac::{configure_proxy, deconfigure_proxy},
    rpc::DaemonArgs,
};

const DEFAULT_CONFIG_YAML: &str = include_str!("default-config.yaml");

const CONTROL_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12222);

pub const PAC_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12223);

const SOCKS5_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 9909);

pub const HTTP_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 9910);

pub async fn restart_daemon(args: DaemonArgs) -> anyhow::Result<()> {
    if args.global_vpn {
        anyhow::bail!("cannot restart in VPN mode")
    }
    stop_daemon_inner().await?;
    let _ = start_daemon_inner(args)?;
    Ok(())
}

pub async fn start_daemon(args: DaemonArgs) -> anyhow::Result<()> {
    if args.proxy_autoconf {
        configure_proxy()?;
    }
    let crash_rx = start_daemon_inner(args)?;
    let start_fut = async {
        wait_daemon_start()
            .timeout(Duration::from_secs(30))
            .await
            .context("daemon did not start in 30")?;
        Ok::<(), anyhow::Error>(())
    };
    let crash_fut = async {
        match crash_rx.await {
            Ok(stderr) => {
                anyhow::bail!("daemon exited before becoming reachable:\n{}", stderr)
            }
            Err(_) => {
                anyhow::bail!("daemon exited before becoming reachable")
            }
        }
    };
    start_fut.race(crash_fut).await?;
    smol::Timer::after(Duration::from_millis(500)).await;
    Ok(())
}

fn start_daemon_inner(args: DaemonArgs) -> anyhow::Result<OneshotReceiver<String>> {
    let cfg = running_cfg(args);

    let mut tfile = NamedTempFile::with_suffix(".yaml")?;
    let val = serde_json::to_value(&cfg)?;

    tfile.write_all(serde_yaml::to_string(&val)?.as_bytes())?;
    tfile.flush()?;
    let (_, path) = tfile.keep()?;

    let (sender, receiver) = oneshot_channel::<String>();

    if cfg.vpn {
        #[cfg(target_os = "linux")]
        {
            let exec_path = std::env::var("APPIMAGE").unwrap_or_else(|_| {
                std::env::current_exe()
                    .expect("could not get current_exe")
                    .display()
                    .to_string()
            });

            let mut cmd = std::process::Command::new("pkexec");
            cmd.arg(exec_path).arg("--config").arg(path);
            std::thread::spawn(move || {
                let _ = cmd.status();
                let _ = sender.send(String::new());
            });
        }
        #[cfg(target_os = "windows")]
        {
            let mut cmd = runas::Command::new(std::env::current_exe().unwrap());
            cmd.arg("--config").arg(path);
            cmd.show(false);
            std::thread::spawn(move || {
                let _ = cmd.status();
                let _ = sender.send(String::new());
            });
        }
    } else {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.arg("--config").arg(path);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000);
        cmd.stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        std::thread::spawn(move || {
            let mut buf = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                stderr.read_to_string(&mut buf).ok();
            }
            let _ = child.wait();
            let _ = sender.send(buf);
        });
    }

    Ok(receiver)
}

async fn wait_daemon_start() {
    smol::Timer::after(Duration::from_millis(150)).await;
    while let Err(err) = check_daemon().await {
        tracing::warn!(err = debug(err), "daemon check result");
        smol::Timer::after(Duration::from_millis(250)).await;
    }
}

async fn check_daemon() -> anyhow::Result<()> {
    TcpStream::connect(CONTROL_ADDR)
        .timeout(Duration::from_millis(50))
        .await
        .context("timeout")??;
    Ok(())
}

pub async fn stop_daemon() -> anyhow::Result<()> {
    let _ = deconfigure_proxy();
    stop_daemon_inner().await
}

async fn stop_daemon_inner() -> anyhow::Result<()> {
    let jrpc = JrpcRequest {
        jsonrpc: "2.0".into(),
        method: "stop".into(),
        params: vec![],
        id: JrpcId::Number(1),
    };
    daemon_rpc(jrpc).await?;
    smol::Timer::after(Duration::from_millis(1000)).await;
    Ok(())
}

pub async fn daemon_running() -> bool {
    check_daemon().await.is_ok()
}

/// Either dispatches to a running daemon, or virtually starts a dryrun daemon and runs with it
pub async fn daemon_rpc(inner: JrpcRequest) -> anyhow::Result<JrpcResponse> {
    match daemon_rpc_tcp(inner.clone())
        .timeout(Duration::from_secs(3))
        .await
    {
        Some(Ok(resp)) => Ok(resp),
        Some(Err(err)) => {
            // tracing::warn!(
            //     method = debug(&inner.method),
            //     err = debug(err),
            //     "error calling TCP, falling back to direct"
            // );
            daemon_rpc_direct(inner).await
        }
        None => {
            anyhow::bail!("timed out")
        }
    }
}

async fn daemon_rpc_tcp(inner: JrpcRequest) -> anyhow::Result<JrpcResponse> {
    let conn = TcpStream::connect(CONTROL_ADDR)
        .timeout(Duration::from_millis(50))
        .await
        .context("timeout")
        .and_then(|s| Ok(s?))?;
    let (read, mut write) = conn.split();
    write
        .write_all(format!("{}\n", serde_json::to_string(&inner)?).as_bytes())
        .await?;
    let mut read = BufReader::new(read);
    let mut buf = String::new();
    read.read_line(&mut buf).await?;
    Ok(serde_json::from_str(&buf)?)
}

async fn daemon_rpc_direct(inner: JrpcRequest) -> anyhow::Result<JrpcResponse> {
    if inner.method == "stop" {
        anyhow::bail!("cannot stop now lol");
    }
    static DAEMON: LazyLock<geph5_client::Client> =
        LazyLock::new(|| geph5_client::Client::start(default_config().inert()));
    DAEMON.control_client().0.call_raw(inner).await
}

fn default_config() -> geph5_client::Config {
    static DEFAULT_CONFIG: LazyLock<geph5_client::Config> = LazyLock::new(|| {
        let value: serde_json::Value = serde_yaml::from_str(DEFAULT_CONFIG_YAML)
            .expect("default-config.yaml must deserialize into serde_json::Value");
        serde_json::from_value(value)
            .expect("default-config.yaml must deserialize into geph5_client::Config")
    });

    DEFAULT_CONFIG.clone()
}

fn running_cfg(args: DaemonArgs) -> geph5_client::Config {
    // Start with the template config:
    let mut cfg = default_config();

    // Override fields that depend on `args`:
    cfg.vpn = args.global_vpn;
    cfg.passthrough_china = args.prc_whitelist;
    cfg.credentials = geph5_broker_protocol::Credential::Secret(args.secret);
    if args.listen_all {
        cfg.socks5_listen =
            Some(SOCKS5_ADDR.tap_mut(|sa| sa.set_ip(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)))));
        cfg.http_proxy_listen =
            Some(HTTP_ADDR.tap_mut(|sa| sa.set_ip(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)))));
    }

    cfg.exit_constraint = match args.exit {
        crate::rpc::ExitConstraint::Auto => geph5_client::ExitConstraint::Auto,
        crate::rpc::ExitConstraint::Manual { city, country } => {
            geph5_client::ExitConstraint::CountryCity(
                CountryCode::for_alpha2(&country).unwrap(),
                city,
            )
        }
    };

    cfg.sess_metadata = args.metadata;
    cfg.allow_direct = args.allow_direct;

    cfg
}

#[cfg(test)]
mod tests {
    use crate::daemon::default_config;

    #[test]
    fn test_dump_default_config() {
        // Get the default configuration
        let config = default_config();

        // Convert to JSON and pretty print
        let json_config =
            serde_json::to_string_pretty(&config).expect("Failed to serialize config to JSON");

        // Print the JSON representation for inspection
        println!("Default config JSON representation:");
        println!("{}", json_config);

        // Assert that the config can be serialized (this should never fail if the previous step succeeded)
        assert!(serde_json::to_string(&config).is_ok());
    }
}
