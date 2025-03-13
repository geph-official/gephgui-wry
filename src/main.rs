#![windows_subsystem = "windows"]

use std::process::Command;

// use autoupdate::autoupdate_loop;
use fakefs::FakeFs;

use mtbus::mt_next;

use rpc::ipc_handle;
// #[cfg(feature = "tray")]
// use tao::system_tray::{SystemTray, SystemTrayBuilder};
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop, EventLoopBuilder},
    window::{Window, WindowBuilder},
};

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
mod autoupdate;
mod daemon;
mod fakefs;

mod mtbus;
mod pac;
mod rpc;

use wry::{
    http::{header::CONTENT_TYPE, Response},
    WebContext, WebView, WebViewBuilder,
};

const WINDOW_WIDTH: i32 = 400;
const WINDOW_HEIGHT: i32 = 650;

fn main() -> anyhow::Result<()> {
    geph5_client::logging::init_logging()?;

    // see whether this is a subprocess that simulates "geph5-client --config ..."
    let args = std::env::args().collect::<Vec<_>>();
    if let Some("--config") = args.get(1).map(|s| s.as_str()) {
        let val: serde_json::Value = serde_yaml::from_slice(&std::fs::read(&args[2])?)?;
        let cfg: geph5_client::Config = serde_json::from_value(val)?;
        let client = geph5_client::Client::start(cfg);
        smol::future::block_on(client.wait_until_dead())?;
        return Ok(());
    }

    let event_loop: EventLoop<Box<dyn FnOnce(&WebView, &Window) + Send + 'static>> =
        EventLoopBuilder::with_user_event().build();
    let evt_proxy = event_loop.create_proxy();
    std::thread::spawn(move || loop {
        let evt = mt_next();
        evt_proxy.send_event(Box::new(evt)).ok().unwrap();
    });
    let dpi = get_xft_dpi();
    let window = WindowBuilder::new()
        .with_resizable(true)
        .with_inner_size(LogicalSize {
            width: WINDOW_WIDTH * dpi as i32 / 96,
            height: WINDOW_HEIGHT * dpi as i32 / 96,
        })
        .build(&event_loop)
        .unwrap();

    let initjs = include_str!("init.js").to_string();
    #[cfg(target_os = "macos")]
    // horrifying HACK
    let initjs = initjs.replace("supports_vpn_conf: true", "supports_vpn_conf: false");

    let mut wctx = WebContext::new(dirs::config_dir());
    let builder = WebViewBuilder::with_web_context(&mut wctx)
        .with_url("geph://index.html")
        .with_initialization_script(&initjs)
        .with_custom_protocol("geph".to_string(), |_, req| {
            let url = req.uri().path().trim_start_matches('/');

            let url = if url.is_empty() { "index.html" } else { url };
            let resp = FakeFs::get(url);
            if let Some(resp) = resp {
                let mime_type = mime_guess::from_path(url)
                    .first_or_octet_stream()
                    .to_string();

                Response::builder()
                    .header(CONTENT_TYPE, mime_type)
                    .body(resp.data)
                    .unwrap()
            } else {
                Response::new(Default::default())
            }
        })
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
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(e) => e(&webview, &window),
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                println!("The close button was pressed; stopping");
                *control_flow = ControlFlow::Exit
            }
            Event::MainEventsCleared => {
                // Application update code.

                // Queue a RedrawRequested event.
                //
                // You only need to call this if you've determined that you need to redraw, in
                // applications which do not always need to. Applications that redraw continuously
                // can just render here instead.
                window.request_redraw();
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

fn get_xft_dpi() -> f64 {
    // Try running "xrdb -query"
    let output = match Command::new("xrdb").arg("-query").output() {
        Ok(o) => o,
        Err(_) => return 96.0, // fallback
    };

    // Convert stdout bytes to String
    let stdout = match String::from_utf8(output.stdout) {
        Ok(s) => s,
        Err(_) => return 96.0, // fallback
    };

    // Look for a line that starts with "Xft.dpi:"
    for line in stdout.lines() {
        if let Some(value) = line.strip_prefix("Xft.dpi:") {
            // Attempt to parse the value after the colon
            if let Ok(dpi) = value.trim().parse::<f64>() {
                return dpi;
            }
        }
    }

    // If nothing matched, fall back
    96.0
}
