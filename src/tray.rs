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

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use tao::window::Window;
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

use crate::manager;

static TUNNEL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Last-polled tunnel-active (connecting/connected) state. Read synchronously by
/// the close handler to decide hide-to-tray vs. exit.
pub fn tunnel_active() -> bool {
    TUNNEL_ACTIVE.load(Ordering::Relaxed)
}

/// Background task mirroring `manager::manager_connected()` into `TUNNEL_ACTIVE` ~1s.
pub fn spawn_state_poll() {
    smolscale::spawn(async {
        loop {
            TUNNEL_ACTIVE.store(manager::manager_connected().await, Ordering::Relaxed);
            smol::Timer::after(Duration::from_secs(1)).await;
        }
    })
    .detach();
}

/// Owns the live tray icon (dropping it removes the icon, so it must outlive the
/// event loop) plus the menu items we toggle/identify on click.
pub struct Tray {
    _tray: TrayIcon,
    show: MenuItem,
    /// A single Connect/Disconnect item whose label tracks the manager state, so
    /// the menu shows only the relevant action instead of both with one greyed out.
    toggle: MenuItem,
    quit: MenuItem,
    /// Localized labels for the two `toggle` states.
    connect_label: &'static str,
    disconnect_label: &'static str,
}

/// Build the tray icon and its context menu. Must be called on the main thread
/// (the event-loop thread): `TrayIcon` is `!Send`, and every backend (the Windows
/// message hook, the macOS `NSStatusItem`, the Linux `gtk`/AppIndicator widget)
/// must be created and serviced on the thread that runs the event loop.
pub fn build_tray() -> anyhow::Result<Tray> {
    let labels = l10n::labels(l10n::detect());
    let show = MenuItem::new(labels.show, true, None);
    // One Connect/Disconnect toggle; `pump_tray_events` keeps its label in sync
    // with the manager state. Starts as "Connect" (disconnected) and is corrected
    // on the first poll.
    let toggle = MenuItem::new(labels.connect, true, None);
    let quit = MenuItem::new(labels.quit, true, None);

    let menu = Menu::new();
    menu.append(&show)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&toggle)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit)?;

    #[allow(unused_mut)]
    let mut builder = TrayIconBuilder::new()
        .with_tooltip("Geph")
        .with_icon(load_icon()?)
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
        show,
        toggle,
        quit,
        connect_label: labels.connect,
        disconnect_label: labels.disconnect,
    })
}

/// Drain pending tray/menu events and refresh menu enablement. Called from the
/// `MainEventsCleared` arm: tray-icon posts its window messages to this same
/// thread's queue, so every click wakes the loop and lands here.
pub fn pump_tray_events(tray: &Tray, window: &Window) {
    let active = tunnel_active();
    // Show exactly one of Connect / Disconnect, matching the manager state.
    let desired_label = if active {
        tray.disconnect_label
    } else {
        tray.connect_label
    };
    if tray.toggle.text().as_str() != desired_label {
        tray.toggle.set_text(desired_label);
    }

    while let Ok(event) = MenuEvent::receiver().try_recv() {
        if event.id == *tray.show.id() {
            show_window(window);
        } else if event.id == *tray.toggle.id() {
            // Connect when disconnected, disconnect when connected.
            if tunnel_active() {
                smolscale::spawn(async {
                    let _ = manager::stop_daemon().await;
                })
                .detach();
            } else {
                smolscale::spawn(async {
                    let _ = manager::reconnect().await;
                })
                .detach();
            }
        } else if event.id == *tray.quit.id() {
            // Honor the invariant: disconnect first, then exit, so the manager is
            // never left active with no tray.
            smolscale::spawn(async {
                let _ = manager::stop_daemon().await;
                std::process::exit(0);
            })
            .detach();
        }
    }

    while let Ok(event) = TrayIconEvent::receiver().try_recv() {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            show_window(window);
        }
    }
}

fn show_window(window: &Window) {
    window.set_visible(true);
    window.set_focus();
}

/// Tray-menu localization, covering the same languages the web frontend supports
/// (gephgui/src/lib/l10n.ts): en, zh-CN, zh-TW, fa, ar, ru, es, uk. The
/// Connect/Disconnect wording matches the frontend's own `l10n.csv`. The tray is
/// a native element built once at startup, so — like the frontend's
/// `detectNearestBrowserLocale` — we pick the nearest language from the OS locale
/// via `sys-locale`, falling back to English.
mod l10n {
    #[derive(Clone, Copy)]
    pub enum Lang {
        En,
        ZhCn,
        ZhTw,
        Fa,
        Ar,
        Ru,
        Es,
        Uk,
    }

    pub fn detect() -> Lang {
        let locale = sys_locale::get_locale().unwrap_or_default().to_lowercase();
        if locale.starts_with("zh") {
            // Traditional Chinese for Taiwan/Hong Kong/Macau or an explicit
            // `Hant` script subtag; Simplified otherwise.
            if ["tw", "hk", "mo", "hant"].iter().any(|t| locale.contains(t)) {
                Lang::ZhTw
            } else {
                Lang::ZhCn
            }
        } else {
            match locale.split(['-', '_']).next().unwrap_or("") {
                "fa" => Lang::Fa,
                "ar" => Lang::Ar,
                "ru" => Lang::Ru,
                "es" => Lang::Es,
                "uk" => Lang::Uk,
                _ => Lang::En,
            }
        }
    }

    pub struct Labels {
        pub show: &'static str,
        pub connect: &'static str,
        pub disconnect: &'static str,
        pub quit: &'static str,
    }

    pub fn labels(lang: Lang) -> Labels {
        match lang {
            Lang::En => Labels {
                show: "Show Geph",
                connect: "Connect",
                disconnect: "Disconnect",
                quit: "Quit",
            },
            Lang::ZhCn => Labels {
                show: "显示 Geph",
                connect: "连接",
                disconnect: "断开",
                quit: "退出",
            },
            Lang::ZhTw => Labels {
                show: "顯示 Geph",
                connect: "連接",
                disconnect: "斷開",
                quit: "結束",
            },
            Lang::Fa => Labels {
                show: "نمایش Geph",
                connect: "اتصال",
                disconnect: "قطع اتصال",
                quit: "خروج",
            },
            Lang::Ar => Labels {
                show: "إظهار Geph",
                connect: "اتصال",
                disconnect: "قطع الاتصال",
                quit: "خروج",
            },
            Lang::Ru => Labels {
                show: "Показать Geph",
                connect: "Подключить",
                disconnect: "Отключить",
                quit: "Выход",
            },
            Lang::Es => Labels {
                show: "Mostrar Geph",
                connect: "Conectar",
                disconnect: "Desconectar",
                quit: "Salir",
            },
            Lang::Uk => Labels {
                show: "Показати Geph",
                connect: "Підключити",
                disconnect: "Відключити",
                quit: "Вийти",
            },
        }
    }
}

/// Decode the embedded logo PNG into a tray icon (mirrors the window-icon decode
/// in main.rs, but produces `tray_icon::Icon` rather than `tao::window::Icon`).
fn load_icon() -> anyhow::Result<Icon> {
    let mut reader = png::Decoder::new(include_bytes!("logo-naked-32px.png").as_ref()).read_info()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    reader.next_frame(&mut buf)?;
    let icon = Icon::from_rgba(buf, reader.info().width, reader.info().height)?;
    Ok(icon)
}
