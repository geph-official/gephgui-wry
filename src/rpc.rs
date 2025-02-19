use async_trait::async_trait;
use nanorpc::{nanorpc_derive, JrpcRequest, RpcService};
use serde::Deserialize;
use tide::log;

use crate::mtbus::mt_enqueue;

#[derive(Deserialize)]
struct IpcObject {
    callback_code: String,
    inner: JrpcRequest,
}

pub fn ipc_handle(ipc_string: String) -> anyhow::Result<()> {
    let ipc: IpcObject = serde_json::from_str(&ipc_string)?;
    log::debug!("ipc: {}", ipc_string);
    smolscale::spawn(async move {
        let rpc = IpcService(RpcProtocolImpl).respond_raw(ipc.inner).await;
        mt_enqueue(move |wv| {
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

#[nanorpc_derive]
#[async_trait]
trait IpcProtocol {
    async fn start_daemon(&self, val: serde_json::Value) {}

    async fn stop_daemon(&self) {}

    async fn echo(&self, i: f64) -> f64 {
        i
    }
}

struct RpcProtocolImpl;

#[async_trait]
impl IpcProtocol for RpcProtocolImpl {}
