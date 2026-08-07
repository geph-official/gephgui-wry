//! System-tray icon + exit guard (Windows, macOS, Linux).
//!
//! The privileged `geph5 manager` owns the tunnel and runs independently of this
//! GUI. To avoid the "tunnel is up but there is no visible UI" situation,
//! we keep a tray icon alive for the whole process lifetime and only let the
//! process exit while the manager is disconnected:
//!
//!   * closing the window while the manager is active hides to tray (see the
//!     `CloseRequested` handler in main.rs),
//!   * the tray "Quit" disconnects first, then exits,
//!   * the auto-update path already disconnects before exiting.
//!
//! `manager_connected()` (the persisted `connected` flag) is `true` exactly while
//! the manager is connecting or connected, so it is our "active" signal. We mirror
//! it into an atomic once a second because the close handler is synchronous and
//! must not block on an RPC.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use tao::window::Window;
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu},
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

use std::collections::HashMap;
use std::sync::Mutex;

use crate::manager;

// use geph5_broker_protocol::ExitConstraint;
use geph5_misc_rpc::client_control::ConnInfo;
use geph5_misc_rpc::manager_control::TunnelSettings;

mod tray_rpc;
mod tray_ui;
mod tray_server_list;
mod tray_menu_l10n;

// use crate::tray_menu_l10n;
// use self::tray_server_list;
use self::tray_server_list::TrayServerEntry;
use self::tray_server_list::TrayServerList;

/// Optional low-noise diagnostics for tray timing and state transitions.
/// Enable with `GEPH_TRAY_DIAGNOSTICS=1` during local troubleshooting.
pub(super) fn tray_diag(message: impl AsRef<str>) {
    if std::env::var_os("GEPH_TRAY_DIAGNOSTICS").is_some() {
        eprintln!("[tray] {}", message.as_ref());
    }
}

// Synchronous snapshot used by the window close handler. The tray ViewModel
// remains responsible for UI state; this flag is only for the close/exit guard.
static TUNNEL_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn tunnel_active() -> bool {
    TUNNEL_ACTIVE.load(Ordering::Acquire)
}

/// Mirror manager connectivity outside the tray event loop without blocking
/// the synchronous window close handler.
pub fn spawn_state_poll() {
    geph5_rt::spawn(async {
        loop {
            let active = manager::manager_connected().await;
            TUNNEL_ACTIVE.store(active, Ordering::Release);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .detach();
}

// tray ui connection status enum
/// Current VPN state shown by tray UI.
/// NOTE:
/// This is connection state only.
/// Do NOT store business logic here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Disconnected,
    Connecting,
    Connected,
}

/// User actions collected from the native tray menu.
enum TrayAction {
    ShowWindow,
    ToggleConnection,
    SelectServer(ServerSelection),
    ToggleGlobalVpn,
    ToggleSplitTunnel,
    Quit,
}

/// Latest daemon values consumed by the UI refresh pipeline.
#[derive(Default)]
struct TrayRpcCache {
    pub tunnel_settings: Option<TunnelSettings>,
    pub conn_info: Option<ConnInfo>,
    pub server_list: Option<TrayServerList>,
    pub server_list_valid: bool,
}

#[derive(Default, Clone, Copy, Hash, Eq, PartialEq)]
struct RpcRefreshScope {
    pub tunnel_settings: bool,
    pub conn_info: bool,
    pub server_list: bool,
}

impl RpcRefreshScope {
    fn is_empty(self) -> bool {
        !self.tunnel_settings && !self.conn_info && !self.server_list
    }

    fn merge(&mut self, other: Self) {
        self.tunnel_settings |= other.tunnel_settings;
        self.conn_info |= other.conn_info;
        self.server_list |= other.server_list;
    }
}

/// Immutable result returned by one generation of asynchronous RPC work.
struct RpcResult {
    generation: u64,
    conn_info: Option<ConnInfo>,
    tunnel_settings: Option<TunnelSettings>,
    server_list: Option<TrayServerList>,
    server_list_valid: Option<bool>,
}

/// UI refresh work requested by actions, polling, or RPC completion.
#[derive(Hash, Eq, PartialEq)]
enum RpcRefresh {
    RefreshRpcCache { scope: RpcRefreshScope },
}

#[derive(Eq, Hash, PartialEq)]
enum TrayRefresh {
    RefreshUI,
    RefreshIcon,
    RefreshBlink,
}

/// Selects the icon bitmap used for the current connection state.
enum TrayIconType {
    Color,
    Gray,
}

/// Server choice carried from a dynamic menu item back to the RPC action path.
#[derive(Clone)]
enum ServerSelection {
    Auto,
    Server(TrayServerEntry),
}

/// Owns the live tray icon (dropping it removes the icon, so it must outlive the
/// event loop) plus the menu items we toggle/identify on click.
pub struct Tray {
    _tray: TrayIcon,

    /// Last state observed by the tray event loop and used for optimistic UI decisions.
    last_conn_state: Cell<ConnState>,
    /// Deadline source for the one-second state/settings poll.
    last_tray_state_poll: Cell<Instant>,
    /// Deadline source for the ten-second server-list poll.
    last_server_list_poll: Cell<Instant>,
    conn_state_cache: Arc<Mutex<ConnState>>,

    // color icon and gray icon for connection status
    color_icon: Icon,
    gray_icon: Icon,

    /// Whether the Connecting animation is currently active.
    blink_enabled: Cell<bool>,
    /// Current color/gray phase of the animation.
    blink_phase: Cell<bool>,
    /// Shared stop flag observed by the timer task.
    blink_timer_running: Arc<AtomicBool>,
    /// Coalesced timer tick consumed by `pump_tray_events`.
    blink_tick: Arc<AtomicBool>,
    // Coalesces wakeups while the main thread is busy handling native events.
    blink_wake_pending: Arc<AtomicBool>,
    // Invalidates old timer threads when blinking is stopped/restarted.
    blink_generation: Arc<AtomicU64>,

    show: MenuItem,

    status: MenuItem, // Add a non-clickable menu item to show connection status - e.g. disconnected/connecting/connected/error

    // Server list Submenu
    server_selector: Submenu,
    server_list: Arc<Mutex<Option<TrayServerList>>>,
    /// Keeps an icon-click server-list request alive until it succeeds.
    server_list_refresh_pending: Cell<bool>,
    // Cache the dynamical menu items in the server list Submenu
    server_selector_subitems: RefCell<Vec<MenuItem>>,
    /// Cached separators, which must be removed together with menu items.
    server_selector_separators: RefCell<Vec<PredefinedMenuItem>>,
    server_selector_map: RefCell<HashMap<MenuId, ServerSelection>>,
    server_selector_title_cache: Arc<Mutex<Option<String>>>,

    /// Latest RPC snapshot; written by the event-loop result applier.
    pub rpc_cache: Arc<Mutex<TrayRpcCache>>,
    /// Refresh scope accumulated while an RPC request is in flight.
    pending_rpc_scope: Arc<Mutex<RpcRefreshScope>>,
    /// Prevents duplicate concurrent daemon refreshes.
    rpc_in_flight: Arc<AtomicBool>,
    /// Monotonically increasing generation used to reject stale results.
    rpc_generation: Arc<AtomicU64>,
    /// Cross-task result queue drained by the tray event loop.
    rpc_results: Arc<Mutex<Vec<RpcResult>>>,

    /// A single Connect/Disconnect item whose label tracks the manager state, so
    /// the menu shows only the relevant action instead of both with one greyed out.
    toggle: MenuItem,

    // Adding tray settings CheckMenuItem
    global_vpn: CheckMenuItem,
    split_tunnel: CheckMenuItem,
    // A cache for the tunnel settings
    tray_settings: Arc<Mutex<TraySettings>>,

    quit: MenuItem,

    /// Localized labels for the three `toggle` states. (adding the "cancel" button during connecting to consist with WebView UI)
    connect_label: &'static str,
    disconnect_label: &'static str,
    cancel_label: &'static str,

    // Localized labels for the connection states
    disconnected_label: &'static str,
    connecting_label: &'static str,
    connected_label: &'static str,
}

// Struct for Tray Settings cache
#[derive(Default)]
struct TraySettings {
    global_vpn: bool,
    split_tunnel: bool,
}

// Impl: Sync TraySettings from TunnelSettings
impl TraySettings {
    pub fn sync_from_tunnel_settings(&mut self, settings: &TunnelSettings) {
        self.global_vpn = settings.vpn;
        self.split_tunnel = settings.passthrough_china;
    }
}

/// Build the tray icon and its context menu. Must be called on the main thread
/// (the event-loop thread): `TrayIcon` is `!Send`, and every backend (the Windows
/// message hook, the macOS `NSStatusItem`, the Linux `gtk`/AppIndicator widget)
/// must be created and serviced on the thread that runs the event loop.
pub fn build_tray() -> anyhow::Result<Tray> {
    let labels = tray_menu_l10n::current_labels();
    let show = MenuItem::new(labels.show, true, None);
    // One Connect/Disconnect toggle; `pump_tray_events` keeps its label in sync
    // with the manager state. Starts as "Connect" (disconnected) and is corrected
    // on the first poll.
    let toggle = MenuItem::new(labels.connect, true, None);
    let status = MenuItem::new(labels.disconnected, true, None);
    let conn_state_cache = Arc::new(Mutex::new(ConnState::Disconnected));
    let server_selector = Submenu::new(labels.select_server, true);
    let server_list = Arc::new(Mutex::new(None));
    let server_selector_subitems = RefCell::new(Vec::new());
    let server_selector_map = RefCell::new(HashMap::new());
    let server_selector_title_cache = Arc::new(Mutex::new(None));

    let rpc_cache = Arc::new(Mutex::new(TrayRpcCache::default()));
    let pending_rpc_scope = Arc::new(Mutex::new(RpcRefreshScope::default()));
    let rpc_in_flight = Arc::new(AtomicBool::new(false));
    let rpc_generation = Arc::new(AtomicU64::new(0));
    let rpc_results = Arc::new(Mutex::new(Vec::new()));

    let quit = MenuItem::new(labels.quit, true, None);

    // global_vpn checkMenuItem in Tray
    let global_vpn = CheckMenuItem::new(labels.global_vpn, true, false, None);
    // split tunnel checkMenuItem in Tray
    let split_tunnel = CheckMenuItem::new(labels.split_tunnel, true, false, None);
    let tray_settings = Arc::new(Mutex::new(TraySettings {
        global_vpn: false,
        split_tunnel: false,
    }));

    let menu = Menu::new();
    menu.append(&show)?;
    menu.append(&PredefinedMenuItem::separator())?;

    // Add a menu item to show connection status - e.g. disconnected/connecting/connected/error
    menu.append(&status)?;
    status.set_enabled(false); //set status to non-clickable

    // Server select submenu item
    menu.append(&server_selector)?;

    menu.append(&toggle)?;
    menu.append(&PredefinedMenuItem::separator())?;

    menu.append(&global_vpn)?;
    menu.append(&split_tunnel)?;
    menu.append(&PredefinedMenuItem::separator())?;

    menu.append(&quit)?;

    #[allow(unused_mut)]
    let color_icon = tray_ui::load_icon(TrayIconType::Color)?;
    let gray_icon = tray_ui::load_icon(TrayIconType::Gray)?;

    let builder = TrayIconBuilder::new()
        .with_tooltip("Geph")
        .with_icon(gray_icon.clone())
        .with_menu(Box::new(menu));

    // Under Flatpak, the appindicator icon is passed to the host's tray daemon
    // as a file path, so it must live somewhere the host can read. The
    // sandbox-private /tmp default is invisible outside;
    // $XDG_RUNTIME_DIR/app/$FLATPAK_ID is shared with the host at the same path.
    #[cfg(target_os = "linux")]
    if let (Ok(app_id), Ok(runtime_dir)) = (
        std::env::var("FLATPAK_ID"),
        std::env::var("XDG_RUNTIME_DIR"),
    ) {
        let dir = std::path::Path::new(&runtime_dir).join("app").join(app_id);
        let _ = std::fs::create_dir_all(&dir);
        builder = builder.with_temp_dir_path(dir);
    }

    let tray = builder.build()?;

    Ok(Tray {
        _tray: tray,
        last_conn_state: Cell::new(ConnState::Disconnected), // disconnected as default state at App startup
        last_tray_state_poll: Cell::new(Instant::now()),
        last_server_list_poll: Cell::new(Instant::now()),
        conn_state_cache,
        color_icon,
        gray_icon,
        blink_enabled: Cell::new(false),
        blink_phase: Cell::new(false),
        blink_timer_running: Arc::new(AtomicBool::new(false)),
        blink_tick: Arc::new(AtomicBool::new(false)),
        blink_wake_pending: Arc::new(AtomicBool::new(false)),
        blink_generation: Arc::new(AtomicU64::new(0)),
        show,
        status,
        toggle,
        global_vpn,
        split_tunnel,
        tray_settings,
        server_selector,
        server_list,
        server_list_refresh_pending: Cell::new(false),
        server_selector_subitems,
        server_selector_separators: RefCell::new(Vec::new()),
        server_selector_map,
        server_selector_title_cache,
        rpc_cache,
        pending_rpc_scope,
        rpc_in_flight,
        rpc_generation,
        rpc_results,
        quit,
        connect_label: labels.connect,
        disconnect_label: labels.disconnect,
        cancel_label: labels.cancel,
        disconnected_label: labels.disconnected,
        connecting_label: labels.connecting,
        connected_label: labels.connected,
    })
}

/// Drain pending tray/menu events and refresh menu enablement. Called from the
/// `MainEventsCleared` arm: tray-icon posts its window messages to this same
/// thread's queue, so every click wakes the loop and lands here.
pub fn pump_tray_events(tray: &Tray, window: &Window) {
    // Coalesce every "show the window" request in this drain into a single
    // `show_window` at the end. A fast double-click on the tray delivers two
    // `Click{Up}` events (plus a `DoubleClick`) in one drain; calling
    // `set_visible`/`set_focus` twice back-to-back here re-enters tao's Windows
    // event-loop runner (the second show/focus fires while the first is still
    // pumping WM_ACTIVATE/WM_SETFOCUS/... messages) and panics with
    // `already borrowed: BorrowMutError`. With `panic = "abort"` that panic takes
    // the whole process down — window and tray vanish together. One show per drain
    // makes a double-click behave like the already-safe single-click.

    let mut click_triggered = false;

    // an unified tray action flag
    let mut actions: Vec<TrayAction> = Vec::new();
    // an unified rpc refresh flag
    let mut rpc_refreshes: HashSet<RpcRefresh> = HashSet::new();
    // an unified tray refresh flag
    let mut tray_refreshes: HashSet<TrayRefresh> = HashSet::new();

    // rewrite clicking event process architecture for better readability and expansion capability
    while let Ok(event) = MenuEvent::receiver().try_recv() {
        click_triggered = true;

        match event.id {
            // Process click of the Show Geph MenuItem into action flag
            id if id == *tray.show.id() => {
                actions.push(TrayAction::ShowWindow);
            }

            // Process click of the Connection toggle MenuItem into action flag
            id if id == *tray.toggle.id() => {
                actions.push(TrayAction::ToggleConnection);
            }

            // Process click of the Quit MenuItem into action flag
            id if id == *tray.quit.id() => {
                actions.push(TrayAction::Quit);
            }

            // Process click of the global_vpn MenuItem into action flag
            id if id == *tray.global_vpn.id() => {
                actions.push(TrayAction::ToggleGlobalVpn);
            }

            // Process click of the split_tunnel toggle into action flag
            id if id == *tray.split_tunnel.id() => {
                actions.push(TrayAction::ToggleSplitTunnel);
            }

            // Process click of the Server Selection in ServerList Submenu into action flag
            id => {
                if let Some(selection) = tray.server_selector_map.borrow().get(&id) {
                    actions.push(TrayAction::SelectServer(selection.clone()));
                }
            }
        }
    }

    // Collect refresh flags and trigger Tray cache refresh if icon gets clicked
    while let Ok(event) = TrayIconEvent::receiver().try_recv() {
        // Debug Log
        // eprintln!("TrayIconEvent {:?}", event);

        if let TrayIconEvent::Click {
            button: MouseButton::Left | MouseButton::Right,
            button_state: MouseButtonState::Down,
            ..
        } = event
        {
            click_triggered = true;
            // An icon click is a user-visible refresh request. It must survive
            // an in-flight RPC or a daemon-not-ready failure.
            tray.server_list_refresh_pending.set(true);

            // Push refreshes if mouse clicked on tray icon
            rpc_refreshes.insert(RpcRefresh::RefreshRpcCache {
                scope: RpcRefreshScope {
                    tunnel_settings: true,
                    conn_info: true,
                    server_list: true,
                },
            });
        }
    }

    // Blink ticks are converted into the same UI refresh path as all other
    // tray updates; the timer thread never touches the native tray object.
    let blink_tick_triggered = tray.blink_tick.swap(false, Ordering::Relaxed);
    if blink_tick_triggered {
        // Allow the timer thread to schedule the next wakeup only after this
        // tick has been consumed by the main event-loop thread.
        tray.blink_wake_pending.store(false, Ordering::Release);
        tray_refreshes.insert(TrayRefresh::RefreshBlink);
    }

    // Trigger tray refresh and apply actions by tray event or the cycled 1s/10s polling
    let need_tray_state_poll = should_poll_state(tray);
    let need_server_list_poll = should_poll_server_list(tray);
    // Debug Log
    // eprintln!("need_tray_state_poll={} elapsed={:?}", need_tray_state_poll, tray.last_tray_state_poll.get().elapsed());

    let rpc_result_ready = !tray.rpc_results.lock().unwrap().is_empty();
    if click_triggered
        || need_tray_state_poll
        || need_server_list_poll
        || blink_tick_triggered
        || rpc_result_ready
    {
        let mut scope = RpcRefreshScope {
            tunnel_settings: false,
            conn_info: false,
            server_list: false,
        };

        // Refresh tray state by 1s polling
        if need_tray_state_poll {
            scope.tunnel_settings = true;
            scope.conn_info = true;
        }

        // Refresh server list by 10s polling
        if need_server_list_poll || tray.server_list_refresh_pending.get() {
            scope.server_list = true;
        }

        if scope.tunnel_settings || scope.conn_info || scope.server_list {
            rpc_refreshes.insert(RpcRefresh::RefreshRpcCache { scope });
        }

        // Process actions first. Actions may apply an optimistic local state
        // and must be reflected in the icon before waiting for RPC results.
        let rpc_scope = tray_rpc::process_tray_actions(tray, actions, window);
        if let Some(scope) = rpc_scope {
            rpc_refreshes.insert(RpcRefresh::RefreshRpcCache { scope });
        }

        // Consume completed RPC data. The completion flag is retained for
        // compatibility with the existing cache, but UI refreshes are now
        // also generated explicitly for icon/blink changes.
        if tray_rpc::apply_rpc_results(tray) {
            tray_refreshes.insert(TrayRefresh::RefreshUI);
        }

        // Apply completed results before starting a newer generation. This
        // prevents a just-completed server-list result from being discarded
        // merely because the next polling request incremented the generation.
        tray_rpc::process_rpc_refreshes(tray, rpc_refreshes);

        // A request accumulated while another RPC was in flight can now be
        // started without waiting for the next polling interval.
        tray_rpc::process_rpc_refreshes(tray, HashSet::new());

        // Refresh Tray from rpc_cache
        tray_ui::process_tray_refreshes(tray, tray_refreshes);

        // Process server list update in the server_selector Submenu
        tray_ui::process_server_list_update(tray);

        let conn_state = *tray.conn_state_cache.lock().unwrap();
        tray_ui::update_tray_ui(tray, conn_state);
    }
}

/// Bring the window to the foreground. Guarded so each native call is a no-op when
/// already in the desired state: this both cuts the message churn that feeds the
/// re-entrancy panic (see `pump_tray_events`) and keeps the `__show` single-instance
/// path cheap when the window is already up.
// Process actions collected from Tray MenuEvent and return which rpc_caches need refresh
fn process_tray_actions(
    tray: &Tray,
    actions: Vec<TrayAction>,
    window: &Window,
) -> Option<RpcRefreshScope> {
    // Debug Log
    // eprintln!("actions={:?}", actions.len());
    let mut rpc_scope = RpcRefreshScope {
        tunnel_settings: false,
        conn_info: false,
        server_list: false,
    };

    for action in actions {
        match action {
            TrayAction::ShowWindow => {
                tray_ui::show_window(window);
            }

            TrayAction::ToggleConnection => {
                tray_rpc::handle_toggle_connection(tray, tray.last_conn_state.get());
                rpc_scope.conn_info = true;
            }

            TrayAction::ToggleGlobalVpn => {
                tray_rpc::handle_toggle_settings(action, tray.tray_settings.clone());
                rpc_scope.tunnel_settings = true;
            }

            TrayAction::ToggleSplitTunnel => {
                tray_rpc::handle_toggle_settings(action, tray.tray_settings.clone());
                rpc_scope.tunnel_settings = true;
            }

            TrayAction::SelectServer(selection) => {
                match &selection {
                    ServerSelection::Auto => {
                        eprintln!("Selected server: Auto");
                    }

                    ServerSelection::Server(server) => {
                        eprintln!(
                            "Selected server: {} / {} hostname={}",
                            server.country, server.city, server.hostname
                        );
                    }
                }
                let state = tray.last_conn_state.get();
                geph5_rt::spawn(async move {
                    tray_rpc::handle_server_selection(selection, state).await;
                })
                .detach();
                rpc_scope.conn_info = true;
                rpc_scope.tunnel_settings = true;
            }

            TrayAction::Quit => {
                handle_quit();
            }
        }
    }

    // return some rpc_scope triggered by certain actions for immediate tray refresh
    if rpc_scope.conn_info || rpc_scope.tunnel_settings || rpc_scope.server_list {
        Some(rpc_scope)
    } else {
        None
    }
}

// Handle Quit App
fn handle_quit() {
    // Honor the invariant: disconnect first, then exit, so the manager is
    // never left active with no tray.
    geph5_rt::spawn(async {
        let _ = manager::stop_daemon().await;
        std::process::exit(0);
    })
    .detach();
}

// Generate a 1s polling for tray_ui refresh (server_list excluded)
fn should_poll_state(tray: &Tray) -> bool {
    let now = Instant::now();
    if now.duration_since(tray.last_tray_state_poll.get()) >= Duration::from_secs(1) {
        tray.last_tray_state_poll.set(now);
        true
    } else {
        false
    }
}

// Generate a 1s polling for tray_ui refresh (server_list excluded)
fn should_poll_server_list(tray: &Tray) -> bool {
    let now = Instant::now();
    if now.duration_since(tray.last_server_list_poll.get()) >= Duration::from_secs(10) {
        tray.last_server_list_poll.set(now);
        true
    } else {
        false
    }
}
