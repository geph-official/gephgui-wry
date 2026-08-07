//! Tray RPC boundary.
//!
//! This module exposes the RPC scheduling entry points used by the tray
//! coordinator. The daemon client implementations remain in `manager.rs`;
//! this module must not own native menu or icon handles.

use super::{
    manager, process_tray_actions as process_tray_actions_impl, ConnState, RpcRefresh,
    RpcRefreshScope, RpcResult, ServerSelection, Tray, TrayAction, TraySettings,
};
use crate::mtbus::mt_enqueue;
use geph5_broker_protocol::ExitConstraint;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tao::window::Window;

/// Starts one asynchronous daemon refresh for the requested cache scope.
/// ConnInfo is fetched first because it drives the latency-sensitive tray icon.
pub(super) fn refresh_rpc_cache(
    scope: RpcRefreshScope,
    conn_state: ConnState,
    generation: u64,
    results: Arc<Mutex<Vec<RpcResult>>>,
    in_flight: Arc<AtomicBool>,
) {
    geph5_rt::spawn(async move {
        let conn_info = if scope.conn_info {
            match manager::tray_get_conn_info().await {
                Ok(v) => Some(v),
                Err(e) => {
                    eprintln!("tray: tray_get_conn_info failed: {e:?}");
                    None
                }
            }
        } else {
            None
        };
        let tunnel_settings = if scope.tunnel_settings {
            match manager::tray_get_tunnel_settings().await {
                Ok(v) => Some(v),
                Err(e) => {
                    eprintln!("tray: tray_get_tunnel_settings failed: {e:?}");
                    None
                }
            }
        } else {
            None
        };
        let should_refresh_server_list =
            scope.server_list && !matches!(conn_state, ConnState::Connecting);
        let server_list = if should_refresh_server_list {
            match manager::tray_get_net_status().await {
                Ok(status) => Some(super::tray_server_list::TrayServerList::from_net_status(
                    status,
                )),
                Err(e) => {
                    eprintln!("tray_get_net_status failed: {e:?}");
                    None
                }
            }
        } else {
            None
        };
        let server_list_valid = should_refresh_server_list.then_some(server_list.is_some());
        results.lock().unwrap().push(RpcResult {
            generation,
            conn_info,
            tunnel_settings,
            server_list,
            server_list_valid,
        });
        in_flight.store(false, Ordering::Release);
        wake_event_loop();
    })
    .detach();
}

/// Wakes the native event loop so queued RPC results are applied promptly.
pub(super) fn wake_event_loop() {
    mt_enqueue(|_, _| {});
}

/// Handles connection toggles and applies the immediate optimistic UI state.
/// Applies the optimistic connection transition and starts the daemon action.
pub(super) fn handle_toggle_connection(tray: &Tray, state: ConnState) {
    match state {
        ConnState::Disconnected => {
            *tray.conn_state_cache.lock().unwrap() = ConnState::Connecting;
            geph5_rt::spawn(async {
                if let Err(e) = manager::reconnect().await {
                    eprintln!("tray: reconnect failed: {e:?}");
                }
                wake_event_loop();
            })
            .detach();
        }
        ConnState::Connecting | ConnState::Connected => {
            *tray.conn_state_cache.lock().unwrap() = ConnState::Disconnected;
            geph5_rt::spawn(async {
                if let Err(e) = manager::stop_daemon().await {
                    eprintln!("tray: stop daemon failed: {e:?}");
                }
                wake_event_loop();
            })
            .detach();
        }
    }
}

/// Merges refresh requests, prevents concurrent polls, and starts the next generation.
pub(super) fn process_rpc_refreshes(tray: &Tray, refreshes: HashSet<RpcRefresh>) {
    super::tray_diag(format!(
        "process_rpc_refreshes: {} request(s)",
        refreshes.len()
    ));
    let mut pending = tray.pending_rpc_scope.lock().unwrap();
    for refresh in refreshes {
        match refresh {
            RpcRefresh::RefreshRpcCache { scope } => pending.merge(scope),
        }
    }
    if pending.is_empty() || tray.rpc_in_flight.swap(true, Ordering::AcqRel) {
        return;
    }
    let scope = std::mem::take(&mut *pending);
    let generation = tray.rpc_generation.fetch_add(1, Ordering::AcqRel) + 1;
    let conn_state = *tray.conn_state_cache.lock().unwrap();
    drop(pending);
    refresh_rpc_cache(
        scope,
        conn_state,
        generation,
        tray.rpc_results.clone(),
        tray.rpc_in_flight.clone(),
    );
}

/// Applies completed RPC results on the event-loop thread and rejects stale generations.
pub(super) fn apply_rpc_results(tray: &Tray) -> bool {
    let results = std::mem::take(&mut *tray.rpc_results.lock().unwrap());
    if results.is_empty() {
        return false;
    }

    let current_generation = tray.rpc_generation.load(Ordering::Acquire);
    let mut updated = false;
    let mut cache = tray.rpc_cache.lock().unwrap();

    for result in results {
        if result.generation < current_generation {
            eprintln!(
                "tray: ignoring stale RPC result generation={}",
                result.generation
            );
            continue;
        }
        if let Some(v) = result.conn_info {
            cache.conn_info = Some(v);
            updated = true;
        }
        if let Some(v) = result.tunnel_settings {
            cache.tunnel_settings = Some(v);
            updated = true;
        }
        if let Some(v) = result.server_list {
            cache.server_list = Some(v);
            tray.server_list_refresh_pending.set(false);
            updated = true;
        }
        if let Some(valid) = result.server_list_valid {
            cache.server_list_valid = valid;
            if !valid {
                // Keep the user-requested refresh pending so the next event
                // loop cycle retries after the daemon becomes ready.
                tray.server_list_refresh_pending.set(true);
            }
            updated = true;
        }
    }
    updated
}

/// Converts native menu actions into immediate UI work and deferred RPC refresh scopes.
pub(super) fn process_tray_actions(
    tray: &Tray,
    actions: Vec<TrayAction>,
    window: &Window,
) -> Option<RpcRefreshScope> {
    process_tray_actions_impl(tray, actions, window)
}

/// Applies a tray setting change asynchronously and logs failures for diagnostics.
pub(super) fn handle_toggle_settings(action: TrayAction, tray_settings: Arc<Mutex<TraySettings>>) {
    geph5_rt::spawn(async move {
        let mut settings = match manager::tray_get_tunnel_settings().await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("get tunnel settings failed: {e:?}");
                return;
            }
        };

        match action {
            TrayAction::ToggleGlobalVpn => settings.vpn = !settings.vpn,
            TrayAction::ToggleSplitTunnel => {
                settings.passthrough_china = !settings.passthrough_china
            }
            _ => {}
        }

        match manager::tray_apply_tunnel_settings(settings.clone()).await {
            Ok(_) => {
                tray_settings
                    .lock()
                    .unwrap()
                    .sync_from_tunnel_settings(&settings);
                wake_event_loop();
            }
            Err(e) => eprintln!("apply tunnel settings failed: {e:?}"),
        }
    })
    .detach();
}

pub(super) async fn handle_server_selection(selection: ServerSelection, conn_state: ConnState) {
    let mut settings = match manager::tray_get_tunnel_settings().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("get tunnel settings failed: {e:?}");
            return;
        }
    };

    match selection {
        ServerSelection::Auto => {
            settings.exit_constraint = ExitConstraint::Auto;
            eprintln!("Selected server: Auto");
        }
        ServerSelection::Server(server) => {
            settings.exit_constraint =
                ExitConstraint::CountryCity(server.country_code.clone(), server.city.clone());
            eprintln!("Selected server: {} / {}", server.country_code, server.city);
        }
    }

    if let Err(e) = manager::tray_apply_tunnel_settings(settings).await {
        eprintln!("apply tunnel settings failed: {e:?}");
    }

    if let ConnState::Disconnected = conn_state {
        eprintln!("Currently disconnected, reconnecting...");
        if let Err(e) = manager::reconnect().await {
            eprintln!("reconnect after server selection failed: {e:?}");
        }
    }
    wake_event_loop();
}
