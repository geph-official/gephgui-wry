use tide::convert::{DeserializeOwned, Serialize};
use wry::{
    application::window::Window,
    webview::{RpcRequest, RpcResponse},
};

/// JSON-RPC interface that talks to JavaScript.
#[tracing::instrument]
pub fn global_rpc_handler(window: &Window, req: RpcRequest) -> Option<RpcResponse> {
    match req.method.as_str() {
        "echo" => handle_rpc(req, handle_echo),
        other => {
            tracing::error!("unrecognized RPC verb {}", other);
            None
        }
    }
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

fn handle_echo(params: (String,)) -> anyhow::Result<String> {
    Ok(params.0)
}
