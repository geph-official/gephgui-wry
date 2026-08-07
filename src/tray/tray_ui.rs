//! Native tray UI boundary.
//!
//! All native menu/icon operations remain on the event-loop thread. This
//! module provides the UI-facing entry points while the concrete native-handle
//! implementation stays in the coordinator during this low-risk migration.

use super::{ConnInfo, ServerSelection, Tray, TrayIconType, TrayRefresh, TrayServerList};
use super::tray_menu_l10n;
use geph5_broker_protocol::ExitConstraint;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tao::window::Window;
use tray_icon::menu::{MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::Icon;

/// Copies the latest cached connection information into the UI state cache.
pub(super) fn refresh_conn_state(tray: &Tray) {
    let cache = tray.rpc_cache.lock().unwrap();
    if let Some(info) = &cache.conn_info {
        let state = match info {
            ConnInfo::Disconnected => super::ConnState::Disconnected,
            ConnInfo::Connecting => super::ConnState::Connecting,
            ConnInfo::Connected { .. } => super::ConnState::Connected,
        };
        *tray.conn_state_cache.lock().unwrap() = state;
    }
}

/// Bring the main window forward while avoiding redundant native calls.
/// Restores, shows, and focuses the main window without redundant native calls.
pub(super) fn show_window(window: &Window) {
    if window.is_minimized() {
        window.set_minimized(false);
    }
    if !window.is_visible() {
        window.set_visible(true);
    }
    if !window.is_focused() {
        window.set_focus();
    }
}

/// Synchronizes cached tunnel settings into the tray settings view model.
pub(super) fn refresh_tray_settings(tray: &Tray) {
    let cache = tray.rpc_cache.lock().unwrap();
    if let Some(settings) = &cache.tunnel_settings {
        tray.tray_settings
            .lock()
            .unwrap()
            .sync_from_tunnel_settings(settings);
    }
}

/// Transfers a completed server-list result into the UI-owned server cache.
pub(super) fn refresh_server_list(tray: &Tray) {
    let mut cache = tray.rpc_cache.lock().unwrap();
    if let Some(list) = cache.server_list.take() {
        *tray.server_list.lock().unwrap() = Some(list);
    }
}

/// Consumes UI refresh flags and updates state, menu labels, icon, or blink phase.
pub(super) fn process_tray_refreshes(tray: &Tray, refreshes: HashSet<TrayRefresh>) {
    super::tray_diag(format!(
        "process_tray_refreshes: {} refresh(es)",
        refreshes.len()
    ));
    for refresh in refreshes {
        match refresh {
            TrayRefresh::RefreshUI => {
                refresh_conn_state(tray);
                refresh_server_selector(tray);
                refresh_server_list(tray);
                refresh_tray_settings(tray);
            }
            TrayRefresh::RefreshIcon => {
                let state = *tray.conn_state_cache.lock().unwrap();
                update_icon(tray, state);
            }
            TrayRefresh::RefreshBlink => update_icon_tick(tray),
        }
    }
}

/// Rebuilds the dynamic server submenu after a new server list is available.
pub(super) fn process_server_list_update(tray: &Tray) {
    if let Some(list) = tray.server_list.lock().unwrap().take() {
        clear_server_selector_cache(tray);
        update_server_selector_subitems(
            &tray.server_selector,
            &tray.server_selector_subitems,
            &tray.server_selector_separators,
            &tray.server_selector_map,
            &list,
        );
    }
}

/// Creates server selector items and records their native IDs for click dispatch.
pub(super) fn update_server_selector_subitems(
    server_menu: &Submenu,
    server_subitems: &RefCell<Vec<MenuItem>>,
    server_separators: &RefCell<Vec<PredefinedMenuItem>>,
    server_selector_map: &RefCell<HashMap<MenuId, ServerSelection>>,
    server_list: &TrayServerList,
) {
    let labels = tray_menu_l10n::current_labels();
    if server_list.is_empty() {
        let item = MenuItem::new(labels.no_available_servers, false, None);
        server_menu.append(&item).unwrap();
        server_subitems.borrow_mut().push(item);
        return;
    }

    let auto_id = MenuId::new("server-auto");
    let auto_item = MenuItem::with_id(auto_id.clone(), labels.auto, true, None);
    server_menu.append(&auto_item).unwrap();
    server_subitems.borrow_mut().push(auto_item);
    server_selector_map
        .borrow_mut()
        .insert(auto_id, ServerSelection::Auto);

    if !server_list.core.is_empty() {
        let separator = PredefinedMenuItem::separator();
        server_menu.append(&separator).unwrap();
        server_separators.borrow_mut().push(separator);
        let title = MenuItem::new(labels.core, false, None);
        server_menu.append(&title).unwrap();
        server_subitems.borrow_mut().push(title);
        append_server_items(
            server_menu,
            server_subitems,
            server_selector_map,
            &server_list.core,
        );
    }
    if !server_list.streaming.is_empty() {
        let separator = PredefinedMenuItem::separator();
        server_menu.append(&separator).unwrap();
        server_separators.borrow_mut().push(separator);
        let title = MenuItem::new(labels.streaming, false, None);
        server_menu.append(&title).unwrap();
        server_subitems.borrow_mut().push(title);
        append_server_items(
            server_menu,
            server_subitems,
            server_selector_map,
            &server_list.streaming,
        );
    }
}

/// Appends one server group while preserving the selection map and item cache.
fn append_server_items(
    server_menu: &Submenu,
    server_subitems: &RefCell<Vec<MenuItem>>,
    server_selector_map: &RefCell<HashMap<MenuId, ServerSelection>>,
    servers: &[super::TrayServerEntry],
) {
    for server in servers {
        let id = MenuId::new(server.hostname.clone());
        let load = (server.load * 100.0).floor() as u32;
        let item = MenuItem::with_id(
            id.clone(),
            format!("{} / {} - [{}%]", server.country, server.city, load),
            true,
            None,
        );
        if let Err(err) = server_menu.append(&item) {
            eprintln!("[tray][ui] append server menu item failed: {err}");
        }
        server_subitems.borrow_mut().push(item);
        server_selector_map
            .borrow_mut()
            .insert(id, ServerSelection::Server(server.clone()));
    }
}

/// Updates the server selector title from the latest connection/settings snapshot.
pub(super) fn refresh_server_selector(tray: &Tray) {
    let cache = tray.rpc_cache.lock().unwrap();
    let title =
        resolve_server_selector_title(cache.conn_info.as_ref(), cache.tunnel_settings.as_ref());
    *tray.server_selector_title_cache.lock().unwrap() = Some(title);
    if let Some(title) = tray.server_selector_title_cache.lock().unwrap().take() {
        tray.server_selector.set_text(title);
    }
}

/// Formats a user-facing server selector title from connection or settings state.
pub(super) fn resolve_server_selector_title(
    conn_info: Option<&super::ConnInfo>,
    settings: Option<&super::TunnelSettings>,
) -> String {
    let labels = tray_menu_l10n::current_labels();
    match conn_info {
        Some(ConnInfo::Connected { sessions }) => {
            if let Some(session) = sessions.first() {
                let exit = &session.exit;
                format!(
                    "{}:    {} / {} - [{}%]",
                    labels.server,
                    exit.country.alpha2(),
                    exit.city,
                    (exit.load * 100.0).floor() as u32
                )
            } else {
                format!("{}:    {}", labels.server, labels.auto)
            }
        }
        _ => match settings {
            Some(settings) => match &settings.exit_constraint {
                ExitConstraint::Auto => format!("{}:    {}", labels.server, labels.auto),
                ExitConstraint::CountryCity(country, city) => {
                    format!("{}:    {:?} / {}", labels.server, country, city)
                }
                ExitConstraint::Hostname(hostname) => format!("{}:    {}", labels.server, hostname),
                ExitConstraint::Country(country) => format!("{}:    {:?}", labels.server, country),
                ExitConstraint::Direct(target) => format!("{}:    {}", labels.server, target),
            },
            None => format!("{}:    {}", labels.server, labels.auto),
        },
    }
}

/// Removes previously generated native server items before rebuilding the submenu.
pub(super) fn clear_server_selector_cache(tray: &Tray) {
    let mut separators = tray.server_selector_separators.borrow_mut();
    for separator in separators.iter() {
        if let Err(e) = tray.server_selector.remove(separator) {
            eprintln!("failed to remove server submenu separator: {e:?}");
        }
    }
    separators.clear();

    let mut items = tray.server_selector_subitems.borrow_mut();
    eprintln!("remove {} submenu items", items.len());
    for item in items.iter() {
        if let Err(e) = tray.server_selector.remove(item) {
            eprintln!("failed to remove server menu item: {e:?}");
        }
    }
    items.clear();
    tray.server_selector_map.borrow_mut().clear();
}

/// Applies the complete menu and icon projection for the current connection state.
pub(super) fn update_tray_ui(tray: &Tray, state: super::ConnState) {
    let conn_changed = tray.last_conn_state.get() != state;
    if conn_changed {
        tray.last_conn_state.set(state);
        update_status_item(tray, state);
        update_toggle_item(tray, state);
        update_icon(tray, state);
    }
    update_setting_items(tray);
}

/// Updates the non-clickable connection status menu item.
pub(super) fn update_status_item(tray: &Tray, state: super::ConnState) {
    let desired = match state {
        super::ConnState::Disconnected => tray.disconnected_label,
        super::ConnState::Connecting => tray.connecting_label,
        super::ConnState::Connected => tray.connected_label,
    };
    tray.status.set_text(desired);
}

/// Updates the Connect, Cancel, or Disconnect menu action label.
pub(super) fn update_toggle_item(tray: &Tray, state: super::ConnState) {
    let desired = match state {
        super::ConnState::Disconnected => tray.connect_label,
        super::ConnState::Connecting => tray.cancel_label,
        super::ConnState::Connected => tray.disconnect_label,
    };
    tray.toggle.set_text(desired);
}

/// Projects cached tunnel settings into the tray check items.
pub(super) fn update_setting_items(tray: &Tray) {
    let settings = tray.tray_settings.lock().unwrap();
    tray.global_vpn.set_checked(settings.global_vpn);
    tray.split_tunnel.set_checked(settings.split_tunnel);
}

/// Loads the color or gray tray bitmap and converts it to a native icon.
pub(super) fn load_icon(kind: TrayIconType) -> anyhow::Result<Icon> {
    let png: &[u8] = match kind {
        TrayIconType::Color => include_bytes!("../logo-naked-32px.png").as_slice(),
        TrayIconType::Gray => include_bytes!("../logo-naked-gray-32px.png").as_slice(),
    };
    let mut reader = png::Decoder::new(png.as_ref()).read_info()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    reader.next_frame(&mut buf)?;
    Ok(Icon::from_rgba(
        buf,
        reader.info().width,
        reader.info().height,
    )?)
}

/// Selects and applies the icon matching the current connection state.
pub(super) fn update_icon(tray: &Tray, state: super::ConnState) {
    match state {
        super::ConnState::Disconnected => {
            stop_icon_animation(tray);
            if let Err(e) = tray._tray.set_icon(Some(tray.gray_icon.clone())) {
                eprintln!("tray: failed to set disconnected icon: {e:?}");
            }
        }
        super::ConnState::Connecting => start_icon_animation(tray),
        super::ConnState::Connected => {
            stop_icon_animation(tray);
            if let Err(e) = tray._tray.set_icon(Some(tray.color_icon.clone())) {
                eprintln!("tray: failed to set connected icon: {e:?}");
            }
        }
    }
}

/// Starts the generation-guarded blink timer for the Connecting state.
pub(super) fn start_icon_animation(tray: &Tray) {
    if tray.blink_enabled.get() {
        return;
    }
    tray.blink_enabled.set(true);
    tray.blink_phase.set(false);
    let running = tray.blink_timer_running.clone();
    let generation = tray.blink_generation.fetch_add(1, Ordering::AcqRel) + 1;
    let blink_generation = tray.blink_generation.clone();
    let blink_tick = tray.blink_tick.clone();
    let blink_wake_pending = tray.blink_wake_pending.clone();
    running.store(true, Ordering::Release);
    std::thread::spawn(move || {
        while running.load(Ordering::Acquire)
            && blink_generation.load(Ordering::Acquire) == generation
        {
            std::thread::sleep(Duration::from_millis(150));
            if !running.load(Ordering::Acquire)
                || blink_generation.load(Ordering::Acquire) != generation
            {
                break;
            }
            blink_tick.store(true, Ordering::Release);
            if !blink_wake_pending.swap(true, Ordering::AcqRel) {
                super::tray_rpc::wake_event_loop();
            }
        }
    });
}

/// Stops blinking and resets all coalescing flags for the next connection cycle.
pub(super) fn stop_icon_animation(tray: &Tray) {
    tray.blink_enabled.set(false);
    tray.blink_generation.fetch_add(1, Ordering::AcqRel);
    tray.blink_timer_running.store(false, Ordering::Release);
    tray.blink_tick.store(false, Ordering::Release);
    tray.blink_wake_pending.store(false, Ordering::Release);
}

/// Consumes one blink tick and applies the next color/gray icon phase.
pub(super) fn update_icon_tick(tray: &Tray) {
    if !tray.blink_enabled.get() {
        return;
    }
    let phase = !tray.blink_phase.get();
    tray.blink_phase.set(phase);
    let icon = if phase {
        tray.color_icon.clone()
    } else {
        tray.gray_icon.clone()
    };
    if let Err(e) = tray._tray.set_icon(Some(icon)) {
        eprintln!("tray: failed to update blinking icon: {e:?}");
    }
}
