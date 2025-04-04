#![windows_subsystem = "windows"]


use autoupdate::check_update_loop;
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
    window::{Icon, Window, WindowBuilder},
};

mod autoupdate;
mod daemon;
mod fakefs;

mod mtbus;
mod pac;
mod rpc;

use wry::{
    WebContext, WebView, WebViewBuilder,
};

const WINDOW_WIDTH: i32 = 400;
const WINDOW_HEIGHT: i32 = 720;

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
    smolscale::spawn(check_update_loop()).detach();

    // Start a simple HTTP server in a separate thread
    std::thread::spawn(|| {
        let server = tiny_http::Server::http("127.0.0.1:5678").unwrap();
        for request in server.incoming_requests() {
            let url = request.url().trim_start_matches('/');
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
    std::thread::spawn(move || loop {
        let evt = mt_next();
        evt_proxy.send_event(Box::new(evt)).ok().unwrap();
    });

    let window = WindowBuilder::new()
        .with_resizable(true)
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

    let initjs = include_str!("init.js").to_string();
    #[cfg(target_os = "macos")]
    // horrifying HACK
    let initjs = initjs.replace("supports_vpn_conf: true", "supports_vpn_conf: false");

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
