use std::{
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    process::Command,
    sync::LazyLock,
    time::Duration,
};

use anyhow::Context;
use futures_util::{io::BufReader, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use geph5_client::{BridgeMode, BrokerKeys, BrokerSource};
use isocountry::CountryCode;
use nanorpc::{JrpcId, JrpcRequest, JrpcResponse, RpcTransport};
use smol::net::TcpStream;
use smol_timeout2::TimeoutExt;
use tap::Tap;
use tempfile::NamedTempFile;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::{
    pac::{configure_proxy, deconfigure_proxy},
    rpc::DaemonArgs,
};

const CONTROL_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12222);

pub const PAC_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12223);

const SOCKS5_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 9909);

const HTTP_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 9910);

pub async fn restart_daemon(args: DaemonArgs) -> anyhow::Result<()> {
    if args.global_vpn {
        anyhow::bail!("cannot restart in VPN mode")
    }
    stop_daemon_inner().await?;
    start_daemon_inner(args).await?;
    Ok(())
}

pub async fn start_daemon(args: DaemonArgs) -> anyhow::Result<()> {
    if args.proxy_autoconf {
        configure_proxy()?;
    }
    start_daemon_inner(args).await?;
    wait_daemon_start()
        .timeout(Duration::from_secs(30))
        .await
        .context("daemon did not start in 30")?;
    smol::Timer::after(Duration::from_millis(500)).await;
    Ok(())
}

async fn start_daemon_inner(args: DaemonArgs) -> anyhow::Result<()> {
    let cfg = running_cfg(args);

    let mut tfile = NamedTempFile::with_suffix(".yaml")?;
    let val = serde_json::to_value(&cfg)?;

    tfile.write_all(serde_yaml::to_string(&val)?.as_bytes())?;
    tfile.flush()?;
    let (_, path) = tfile.keep()?;

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
            cmd.spawn()?;
        }
        #[cfg(target_os = "windows")]
        {
            let mut cmd = runas::Command::new(std::env::current_exe().unwrap());
            cmd.arg("--config").arg(path);
            cmd.show(false);
            std::thread::spawn(move || cmd.status().unwrap());
        }
    } else {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.arg("--config").arg(path);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000);
        cmd.spawn()?;
    }

    Ok(())
}

async fn wait_daemon_start() {
    while let Err(err) = check_daemon().await {
        tracing::warn!(err = debug(err), "daemon check result");
        smol::Timer::after(Duration::from_millis(50)).await;
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
            tracing::warn!(
                method = debug(&inner.method),
                err = debug(err),
                "error calling TCP, falling back to direct"
            );
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
    geph5_client::Config {
        // These fields are the base defaults:
        socks5_listen: Some(SOCKS5_ADDR),
        http_proxy_listen: Some(HTTP_ADDR),
        control_listen: Some(CONTROL_ADDR),
        exit_constraint: geph5_client::ExitConstraint::Auto,
        bridge_mode: BridgeMode::Auto,
        cache: None,
        broker: Some(BrokerSource::PriorityRace(
            vec![
                (
                    0,
                    BrokerSource::Fronted {
                        front: "https://www.cdn77.com/".into(),
                        host: "1826209743.rsc.cdn77.org".into(),
                    },
                ),
                (
                    1000,
                    BrokerSource::Fronted {
                        front: "https://www.vuejs.org/".into(),
                        host: "svitania-naidallszei-2.netlify.app".into(),
                    },
                ),
            ]
            .into_iter()
            .collect(),
        )),
        broker_keys: Some(BrokerKeys {
            master: "88c1d2d4197bed815b01a22cadfc6c35aa246dddb553682037a118aebfaa3954".into(),
            mizaru_free: "0558216cbab7a9c46f298f4c26e171add9af87d0694988b8a8fe52ee932aa754".into(),
            mizaru_plus: "cf6f58868c6d9459b3a63bc2bd86165631b3e916bad7f62b578cd9614e0bcb3b".into(),
        }),
        // Values that can be overridden by `args`:
        vpn: false,
        spoof_dns: false,
        passthrough_china: false,
        dry_run: false,
        credentials: geph5_broker_protocol::Credential::Secret(String::new()),
        sess_metadata: Default::default(),
        task_limit: None,
        vpn_fd: None,
        pac_listen: Some(PAC_ADDR),
    }
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
