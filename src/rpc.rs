use async_trait::async_trait;
use nanorpc::{nanorpc_derive, JrpcId, JrpcRequest, RpcService};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tao::dpi::LogicalSize;
use webbrowser::open_browser;

use crate::{
    daemon::{daemon_rpc, daemon_running, restart_daemon, start_daemon, stop_daemon},
    mtbus::mt_enqueue,
    WINDOW_HEIGHT, WINDOW_WIDTH,
};

#[derive(Deserialize)]
struct IpcObject {
    callback_code: String,
    inner: JrpcRequest,
}

pub fn ipc_handle(ipc_string: String) -> anyhow::Result<()> {
    let ipc: IpcObject = serde_json::from_str(&ipc_string)?;
    tracing::debug!("ipc: {}", ipc_string);
    smolscale::spawn(async move {
        let rpc = IpcService(RpcProtocolImpl).respond_raw(ipc.inner).await;

        mt_enqueue(move |wv, _| {
            wv.evaluate_script(&format!(
                "({})({})",
                ipc.callback_code,
                serde_json::to_string(&rpc).unwrap()
            ))
            .unwrap();
        });
    })
    .detach();
    Ok(())
}

/// The derived RPC trait. Add in all the methods your JS side expects.
#[nanorpc_derive]
#[async_trait]
trait IpcProtocol {
    /// Handles a request to change DPI on, say, GTK platforms with pseudo-hidpi through font size changes.
    async fn set_conversion_factor(&self, factor: f64) {
        mt_enqueue(move |_, window| {
            window.set_inner_size(LogicalSize {
                width: WINDOW_WIDTH as f64 * factor,
                height: WINDOW_HEIGHT as f64 * factor,
            });
        });
    }

    /// Start the daemon with the given arguments.
    async fn start_daemon(&self, args: DaemonArgs) -> Result<(), String> {
        start_daemon(args).await.map_err(|s| format!("{:?}", s))
    }

    /// Stop the daemon.
    async fn stop_daemon(&self) {
        let _ = stop_daemon().await;
    }

    /// Restart the daemon with the given arguments.
    async fn restart_daemon(&self, args: DaemonArgs) -> Result<(), String> {
        restart_daemon(args).await.map_err(|s| format!("{:?}", s))
    }

    /// Returns whether the daemon is running.
    async fn is_running(&self) -> bool {
        daemon_running().await
    }

    /// Generic "daemon_rpc" call.
    async fn daemon_rpc(
        &self,
        method: String,
        args: Vec<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let jrpc = JrpcRequest {
            jsonrpc: "2.0".into(),
            method,
            params: args,
            id: JrpcId::Number(1),
        };
        let resp = daemon_rpc(jrpc).await.map_err(|e| format!("{:?}", e))?;
        if let Some(err) = resp.error {
            tracing::warn!("error: {:?}", err);
            return Err(err.message);
        }
        Ok(resp.result.unwrap_or_default())
    }

    /// Returns a list of price points.
    async fn price_points(&self) -> Result<Vec<(u32, f64)>, String> {
        let v = self.daemon_rpc("price_points".to_string(), vec![]).await?;
        Ok(serde_json::from_value(v).map_err(|s| format!("{:?}", s))?)
    }

    /// Create an invoice using a number of days, returning an `InvoiceInfo`.
    async fn create_invoice(&self, secret: String, days: u32) -> InvoiceInfo {
        InvoiceInfo {
            id: serde_json::to_string(&(secret, days)).unwrap(),
            methods: vec!["credit-card".to_string(), "wechat".to_string()],
        }
    }

    /// Pay an invoice with a given method.
    async fn pay_invoice(&self, id: String, method: String) -> Result<(), String> {
        tracing::warn!("GONNA PAY INVOICE {id} {method}");
        let (secret, days): (String, u32) = serde_json::from_str(&id).map_err(|e| e.to_string())?;
        let url = self
            .daemon_rpc(
                "create_payment".to_string(),
                vec![json!(secret), json!(days), json!(method)],
            )
            .await?;
        tracing::warn!("URL URL URL {url}");
        let url: String = serde_json::from_value(url).map_err(|e| e.to_string())?;
        open_browser(webbrowser::Browser::Default, &url).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Export a debug pack with the provided email.
    async fn export_debug_pack(&self, email: String) -> Result<(), String> {
        self.daemon_rpc(
            "export_debug_pack".to_string(),
            vec![json!(email), json!(self.get_debug_pack().await)],
        )
        .await?;
        Ok(())
    }

    /// Obtain the actual contents of the debug pack.
    async fn get_debug_pack(&self) -> String {
        let gui_logs = geph5_client::logging::get_json_logs();
        let daemon_logs = self
            .daemon_rpc("recent_logs".to_string(), vec![])
            .await
            .unwrap_or(serde_json::Value::Null);
        let daemon_logs: Vec<String> = serde_json::from_value(daemon_logs).unwrap_or_default();
        let daemon_logs = daemon_logs.join("\n");

        format!("===== GUI =====\n\n{gui_logs}\n\n===== DAEMON =====\n\n {daemon_logs}")
    }

    /// Get the icon of an app, returning it as a URL string.
    async fn get_app_icon_url(&self, _id: String) -> String {
        // Replace with real implementation
        todo!()
    }

    /// Whether this platform supports listening on all interfaces.
    async fn supports_listen_all(&self) -> bool {
        // Replace with real implementation
        todo!()
    }

    /// Whether this platform supports an app whitelist.
    async fn supports_app_whitelist(&self) -> bool {
        // Replace with real implementation
        todo!()
    }

    /// Whether this platform supports the PRC whitelist.
    async fn supports_prc_whitelist(&self) -> bool {
        // Replace with real implementation
        todo!()
    }

    async fn supports_proxy_conf(&self) -> bool {
        // Replace with real implementation
        todo!()
    }

    /// Whether this platform supports VPN configuration.
    async fn supports_vpn_conf(&self) -> bool {
        // Replace with real implementation
        todo!()
    }

    /// Whether this platform supports auto-updates.
    async fn supports_autoupdate(&self) -> bool {
        // Replace with real implementation
        todo!()
    }

    /// Obtain native info for debugging.
    async fn get_native_info(&self) -> NativeInfo {
        // Replace with real implementation
        todo!()
    }

    /// Sample echo method left from your original snippet.
    async fn echo(&self, i: f64) -> f64 {
        i
    }

    async fn open_browser(&self, url: String) {
        let _ = open_browser(webbrowser::Browser::Default, &url);
    }
}

struct RpcProtocolImpl;

#[async_trait]
impl IpcProtocol for RpcProtocolImpl {}

#[derive(Debug, Serialize, Deserialize)]
pub struct InvoiceInfo {
    pub id: String,
    pub methods: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonArgs {
    pub secret: String,
    pub metadata: serde_json::Value,
    pub prc_whitelist: bool,
    pub exit: ExitConstraint,
    pub global_vpn: bool,
    pub listen_all: bool,
    pub proxy_autoconf: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExitConstraint {
    /// The string "auto"
    Auto,
    #[serde(untagged)]
    Manual { city: String, country: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NativeInfo {
    pub platform_type: String,
    pub platform_details: String,
    pub version: String,
}
