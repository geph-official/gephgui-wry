#![windows_subsystem = "windows"]

use fakefs::FakeFs;

use mtbus::{mt_enqueue, mt_next};

use rpc::ipc_handle;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop, EventLoopBuilder},
    window::{Icon, Window, WindowBuilder},
};

#[cfg(target_os = "macos")]
use tao::platform::macos::{ActivationPolicy, EventLoopWindowTargetExtMacOS};
#[cfg(target_os = "macos")]
use tray_icon::menu::{Menu, PredefinedMenuItem, Submenu};

mod autoupdate;
#[cfg(target_os = "linux")]
mod bootstrap;
mod manager;
mod fakefs;

mod mtbus;
mod rpc;
mod tray;

use wry::{WebContext, WebView, WebViewBuilder};

const WINDOW_WIDTH: i32 = 400;
const WINDOW_HEIGHT: i32 = 720;

fn main() -> anyhow::Result<()> {
    unsafe {
        std::env::remove_var("http_proxy");
        std::env::remove_var("https_proxy");
        std::env::remove_var("HTTP_PROXY");
        std::env::remove_var("HTTPS_PROXY");
    }

    // The loopback HTTP port doubles as a single-instance lock: only one instance
    // can bind it. A second launch (e.g. the user opens Geph while an autostarted
    // `--hidden` instance is already running) fails to bind, so it pings the
    // running instance to surface its window and then exits — otherwise we'd end
    // up with two tray icons. Do this first, before any startup work, so a second
    // launch bails immediately instead of after the autoupdate network check.
    let server = match tiny_http::Server::http("127.0.0.1:5678") {
        Ok(server) => server,
        Err(_) => {
            use std::io::Write;
            if let Ok(mut stream) = std::net::TcpStream::connect("127.0.0.1:5678") {
                let _ = stream.write_all(b"GET /__show HTTP/1.0\r\nHost: localhost\r\n\r\n");
            }
            std::process::exit(0);
        }
    };

    // The engine no longer runs in-process: a separate privileged `geph manager`
    // owns the tunnel, and we talk to it over its control protocol (see manager.rs).

    // DO NOT run the autoupdate logic on flatpak, but otherwise it's good
    if std::env::var("FLATPAK_ID").is_err() {
        geph5_rt::block_on(autoupdate::prompt_cached_update_if_available())?;
        geph5_rt::spawn(autoupdate::download_update_loop()).detach();
    }

    // Make sure the privileged host manager is installed, current, and answering
    // before we bring up the webview that talks to it. May show a native dialog and
    // elevate via pkexec, or ask for a relaunch; returns false if we should exit now.
    #[cfg(target_os = "linux")]
    if !bootstrap::ensure_manager() {
        return Ok(());
    }

    // Start a simple HTTP server in a separate thread
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let url = request.url().trim_start_matches('/');
            // Single-instance "show yourself" ping from a second launch.
            if url == "__show" {
                mt_enqueue(|_, window| {
                    window.set_visible(true);
                    window.set_focus();
                });
                request
                    .respond(tiny_http::Response::from_string("ok"))
                    .ok();
                continue;
            }
            let url = if url.is_empty() { "index.html" } else { url };

            if let Some(resp) = FakeFs::get(url) {
                let mime_type = mime_guess::from_path(url)
                    .first_or_octet_stream()
                    .to_string();

                let response = tiny_http::Response::from_data(resp.data).with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], mime_type.as_bytes())
                        .unwrap(),
                );

                request.respond(response).ok();
            } else {
                let response = tiny_http::Response::from_string("Not found").with_status_code(404);
                request.respond(response).ok();
            }
        }
    });

    let event_loop: EventLoop<Box<dyn FnOnce(&WebView, &Window) + Send + 'static>> =
        EventLoopBuilder::with_user_event().build();
    let evt_proxy = event_loop.create_proxy();
    std::thread::spawn(move || {
        loop {
            let evt = mt_next();
            evt_proxy.send_event(Box::new(evt)).ok().unwrap();
        }
    });

    // Launched at login via the installer's autostart shortcut with `--hidden`:
    // come up as just the tray icon, no window. Manual launches show the window.
    let start_hidden = std::env::args().any(|arg| arg == "--hidden");

    let window = WindowBuilder::new()
        .with_resizable(true)
        .with_visible(!start_hidden)
        .with_inner_size(LogicalSize {
            width: WINDOW_WIDTH,
            height: WINDOW_HEIGHT,
        })
        .with_title("Geph")
        .with_window_icon({
            let logo_png = png::Decoder::new(include_bytes!("logo-naked-32px.png").as_ref());
            let mut logo_png = logo_png.read_info()?;
            let mut icon_buf = vec![0; logo_png.output_buffer_size()];
            logo_png.next_frame(&mut icon_buf)?;

            let logo_icon =
                Icon::from_rgba(icon_buf, logo_png.info().width, logo_png.info().height)?;
            Some(logo_icon)
        })
        .build(&event_loop)
        .unwrap();

    #[cfg(target_os = "macos")]
    {
        let menu = edit_menu();
        menu?.init_for_nsapp();
    }

    // The VPN configuration UI is enabled everywhere: geph5 supports full-tunnel
    // VPN on macOS (see f84ffef), and on Flatpak Linux the privileged host
    // manager (bootstrapped at startup) owns the TUN/routing/kill-switch, so
    // full-tunnel works from the sandbox too.
    let initjs = include_str!("init.js").to_string();

    let mut wctx = WebContext::new(dirs::config_dir());
    let builder = WebViewBuilder::with_web_context(&mut wctx)
        .with_url("http://127.0.0.1:5678")
        .with_initialization_script(&initjs)
        .with_ipc_handler(|req| {
            let req = req.into_body();
            ipc_handle(req).unwrap();
        });

    #[cfg(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    ))]
    let webview = builder.build(&window)?;

    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    )))]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let vbox = window.default_vbox().unwrap();
        builder.build_gtk(vbox)?
    };

    // The tray icon must be created on (and live on) the event-loop thread, and is
    // kept alive by being moved into the `run` closure below. It is built inside
    // the loop's Init arm rather than here: on macOS the status item must be
    // created AFTER the activation-policy re-assert below — changing the policy
    // of a bundle-launched app tears down any NSStatusItem created before it, so
    // building it up-front yields an app with no tray icon when launched from
    // Geph.app (while working fine when the binary is run from a terminal, where
    // the policy change is a no-op). The poll task keeps `tray::tunnel_active()`
    // fresh for the close handler.
    tray::spawn_state_poll();
    let mut tray: Option<tray::Tray> = None;

    event_loop.run(move |event, _event_loop_target, control_flow| {
        // Wake ~once a second so the tray menu's enabled/disabled state (Connect vs.
        // Disconnect) stays in sync with the manager and pending tray events drain
        // promptly. tray-icon delivers clicks/menu events through global channels
        // that we poll in `MainEventsCleared`, so a steady tick keeps the tray
        // responsive on every platform.
        *control_flow =
            ControlFlow::WaitUntil(std::time::Instant::now() + std::time::Duration::from_secs(1));
        match event {
            Event::NewEvents(tao::event::StartCause::Init) => {
                // Force the Dock icon on macOS. tao defaults to `ActivationPolicy::Regular`
                // and normally applies it in `applicationDidFinishLaunching`, which would
                // give us a Dock tile. But on our startup path the webview and app menu
                // touch the shared `NSApplication` before `event_loop.run()` (and the
                // update check can pump a modal loop), so tao's launch-time policy
                // application is skipped and the process ends up effectively
                // "accessory": window but no Dock icon. Re-asserting Regular once, from
                // inside the loop, restores it.
                #[cfg(target_os = "macos")]
                _event_loop_target.set_activation_policy_at_runtime(ActivationPolicy::Regular);

                // Build the tray only now: on macOS the policy re-assert above would
                // destroy a status item created before it (see the comment at the
                // `tray` declaration). Tray failure is logged rather than fatal — a
                // missing tray icon shouldn't take the whole GUI down.
                match tray::build_tray() {
                    Ok(t) => tray = Some(t),
                    Err(err) => eprintln!("failed to build tray icon: {err:#}"),
                }
            }
            Event::UserEvent(e) => e(&webview, &window),
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                // The `geph manager` is a persistent, privileged process that owns
                // the tunnel and keeps managing it in the background.
                //
                // We must never leave the manager active with no tray icon, so while
                // it's connecting/connected we only hide to tray; we exit (taking the
                // tray with us) only once it's disconnected. The tray's "Quit" item
                // disconnects first, then exits, preserving the same invariant.
                if tray::tunnel_active() {
                    println!("tunnel active; hiding GUI to tray instead of exiting");
                    window.set_visible(false);
                } else {
                    println!("tunnel down; closing the GUI");
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::MainEventsCleared => {
                if let Some(tray) = &tray {
                    tray::pump_tray_events(tray, &window);
                }
            }
            Event::RedrawRequested(_) => {
                // Redraw the application.
                //
                // It's preferable for applications that do not render continuously to render in
                // this event rather than in MainEventsCleared, since rendering in here allows
                // the program to gracefully handle redraws requested by the OS.
            }
            _ => (),
        }
    });
}

#[cfg(target_os = "macos")]
fn edit_menu() -> anyhow::Result<Menu> {
    let edit = Submenu::with_items(
        "Edit",
        true,
        &[
            &PredefinedMenuItem::copy(None),
            &PredefinedMenuItem::paste(None),
        ],
    )?;
    let menu = Menu::new();
    menu.append(&edit)?;
    Ok(menu)
}
