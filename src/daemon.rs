use std::{
    io::Write,
    net::{Ipv4Addr, SocketAddr},
    process::Command,
    sync::LazyLock,
    time::Duration,
};

use futures_util::{io::BufReader, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use geph5_client::{BridgeMode, BrokerKeys, BrokerSource};
use isocountry::CountryCode;
use nanorpc::{JrpcId, JrpcRequest, JrpcResponse, RpcTransport};
use smol::net::TcpStream;
use smol_timeout2::TimeoutExt;
use tempfile::NamedTempFile;

use crate::{
    pac::{configure_proxy, deconfigure_proxy},
    rpc::DaemonArgs,
};

const CONTROL_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12222);

pub const PAC_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12223);

const SOCKS5_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 18964);

pub const HTTP_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 18965);

pub async fn start_daemon(args: DaemonArgs) -> anyhow::Result<()> {
    if args.proxy_autoconf {
        configure_proxy()?;
    }
    let cfg = running_cfg(args);

    let mut tfile = NamedTempFile::with_suffix(".yaml")?;
    tfile.write_all(serde_yaml::to_string(&serde_json::to_value(&cfg)?)?.as_bytes())?;
    tfile.flush()?;
    let (_, path) = tfile.keep()?;

    if cfg.vpn {
        #[cfg(target_os = "linux")]
        {
            let mut cmd = std::process::Command::new("pkexec");
            cmd.arg("geph5-client").arg("--config").arg(path);
            cmd.spawn()?;
            Ok(())
        }
        #[cfg(target_os = "windows")]
        {
            let mut cmd = runas::Command::new("pkexec");
            cmd.arg("geph5-client").arg("--config").arg(path);
            std::thread::spawn(move || cmd.status().unwrap());
            Ok(())
        }
    } else {
        let mut cmd = Command::new("geph5-client");
        cmd.arg("--config").arg(path);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000);
        cmd.spawn()?;
        Ok(())
    }
}

pub async fn stop_daemon() -> anyhow::Result<()> {
    let jrpc = JrpcRequest {
        jsonrpc: "2.0".into(),
        method: "stop".into(),
        params: vec![],
        id: JrpcId::Number(1),
    };
    let _ = deconfigure_proxy();
    daemon_rpc(jrpc).await?;
    Ok(())
}

pub async fn daemon_running() -> bool {
    TcpStream::connect(CONTROL_ADDR).await.is_ok()
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
    let conn = TcpStream::connect(CONTROL_ADDR).await?;
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
        broker: Some(BrokerSource::Race(vec![
            BrokerSource::Fronted {
                front: "https://www.cdn77.com/".into(),
                host: "1826209743.rsc.cdn77.org".into(),
            },
            BrokerSource::Fronted {
                front: "https://vuejs.org/".into(),
                host: "svitania-naidallszei-2.netlify.app".into(),
            },
            BrokerSource::AwsLambda {
                function_name: "geph-lambda-bouncer".into(),
                region: "us-east-1".into(),
                access_key_id: String::from_utf8_lossy(
                    &base32::decode(
                        base32::Alphabet::Crockford,
                        "855MJGAMB58MCPJBB97K4P2C6NC36DT8",
                    )
                    .unwrap(),
                )
                .to_string(),
                secret_access_key: String::from_utf8_lossy(
                    &base32::decode(
                        base32::Alphabet::Crockford,
                        "8SQ7ECABES132WT4B9GQEN356XQ6GRT36NS64GBK9HP42EAGD8W6JRA39DTKAP2J",
                    )
                    .unwrap(),
                )
                .to_string(),
            },
        ])),
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

    cfg.exit_constraint = match args.exit {
        crate::rpc::ExitConstraint::Auto => geph5_client::ExitConstraint::Auto,
        crate::rpc::ExitConstraint::Manual { city, country } => {
            geph5_client::ExitConstraint::CountryCity(
                CountryCode::for_alpha2(&country).unwrap(),
                city,
            )
        }
    };

    cfg
}
