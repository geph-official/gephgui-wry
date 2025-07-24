use once_cell::sync::OnceCell;
use png::Decoder;
use std::cell::RefCell;
use tao::event_loop::EventLoopProxy;
use tao::window::Window;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent, TrayIconId};
use wry::WebView;

/// Type of user event closures used by the UI event loop.
type UserEvent = Box<dyn FnOnce(&WebView, &Window) + Send + 'static>;

static EVENT_LOOP_PROXY: OnceCell<EventLoopProxy<UserEvent>> = OnceCell::new();
thread_local! {
    static TRAY_ICON_HANDLE: RefCell<Option<TrayIcon>> = RefCell::new(None);
}
static ICON_ID: OnceCell<TrayIconId> = OnceCell::new();

/// Initialize the global event loop proxy for tray icon callbacks.
pub fn set_event_loop_proxy(proxy: EventLoopProxy<UserEvent>) {
    EVENT_LOOP_PROXY
        .set(proxy)
        .expect("EventLoopProxy already set");
}

/// Show the tray icon. No-op if already visible.
pub fn show_tray() -> anyhow::Result<()> {
    let proxy = EVENT_LOOP_PROXY
        .get()
        .expect("EventLoopProxy not initialized")
        .clone();

    // Build a simple menu with Show and Quit items
    let menu = Menu::new();
    let show_item = MenuItem::new("Show", true, None);
    let quit_item = MenuItem::new("Quit", true, None);
    menu.append(&show_item)?;
    menu.append(&quit_item)?;
    let show_id = show_item.id().clone();
    let quit_id = quit_item.id().clone();

    // Decode tray icon image as RGBA
    let decoder = Decoder::new(include_bytes!("logo-naked-32px.png").as_ref());
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    let rgba = buf[..info.buffer_size()].to_vec();
    let icon = Icon::from_rgba(rgba, info.width, info.height)?;

    // Create the tray icon
    let builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .with_tooltip("Geph");
    let id = builder.id().clone();
    ICON_ID.set(id.clone()).ok();
    let tray = builder.build()?;

    // Set global event handlers for tray icon and menu events
    let proxy1 = proxy.clone();
    TrayIconEvent::set_event_handler(Some(move |event: tray_icon::TrayIconEvent| {
        if event.id() == &id {
            if let tray_icon::TrayIconEvent::Click { .. } = event {
                let _ = proxy1.send_event(Box::new(|_, window| {
                    window.set_visible(true);
                    window.set_minimized(false);
                    window.set_focus();
                }));
            }
        }
    }));
    let proxy2 = proxy.clone();
    MenuEvent::set_event_handler(Some(move |event: tray_icon::menu::MenuEvent| {
        if event.id() == &show_id {
            let _ = proxy2.send_event(Box::new(|_, window| {
                window.set_visible(true);
                window.set_minimized(false);
                window.set_focus();
            }));
        } else if event.id() == &quit_id {
            std::process::exit(0);
        }
    }));

    // Store the tray icon handle
    TRAY_ICON_HANDLE.with(|cell| {
        *cell.borrow_mut() = Some(tray);
    });
    Ok(())
}

/// Hide and remove the tray icon if present.
pub fn hide_tray() {
    TRAY_ICON_HANDLE.with(|cell| cell.borrow_mut().take());
}

/// Returns whether the tray icon is currently visible.
pub fn is_active() -> bool {
    TRAY_ICON_HANDLE.with(|cell| cell.borrow().is_some())
}
