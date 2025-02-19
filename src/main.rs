#![windows_subsystem = "windows"]

// use autoupdate::autoupdate_loop;
use fakefs::FakeFs;

use mtbus::mt_next;

use rpc::ipc_handle;
// #[cfg(feature = "tray")]
// use tao::system_tray::{SystemTray, SystemTrayBuilder};
use tao::{
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoop, EventLoopBuilder},
    window::WindowBuilder,
};

use tap::Tap;
use tide::Request;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

mod autoupdate;
mod fakefs;
mod mtbus;
mod pac;
mod rpc;

use wry::{
    http::{header::CONTENT_TYPE, Response},
    WebView, WebViewBuilder,
};
const SERVE_ADDR: &str = "127.0.0.1:5678";

const WINDOW_WIDTH: i32 = 380;
const WINDOW_HEIGHT: i32 = 600;

fn main() -> anyhow::Result<()> {
    config_logging();

    smolscale::spawn(async {
        let mut app = tide::new();
        app.at("/*").get(test);
        app.listen(SERVE_ADDR).await.expect("cannot listen to http");
    })
    .detach();

    let event_loop: EventLoop<Box<dyn FnOnce(&WebView) + Send + 'static>> =
        EventLoopBuilder::with_user_event().build();
    let evt_proxy = event_loop.create_proxy();
    std::thread::spawn(move || loop {
        let evt = mt_next();
        evt_proxy.send_event(Box::new(evt)).ok().unwrap();
    });
    let window = WindowBuilder::new().build(&event_loop).unwrap();

    let initjs = include_str!("init.js").to_string();
    #[cfg(target_os = "macos")]
    // horrifying HACK
    let initjs = initjs.replace("supports_vpn_conf: true", "supports_vpn_conf: false");

    let builder = WebViewBuilder::new()
        .with_url("geph://index.html")
        .with_initialization_script(&initjs)
        .with_custom_protocol("geph".to_string(), |_, req| {
            let url = req.uri().path().trim_start_matches('/');
            eprintln!("{:?}", url);
            let url = if url.is_empty() { "index.html" } else { url };
            let resp = FakeFs::get(url);
            if let Some(resp) = resp {
                let mime_type = mime_guess::from_path(url)
                    .first_or_octet_stream()
                    .to_string();
                eprintln!("{}", mime_type);
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
            Event::UserEvent(e) => e(&webview),
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

// fn wry_loop() -> anyhow::Result<()> {
//     eprintln!("entering wry loop");
//     let logo_png = png::Decoder::new(include_bytes!("logo-naked-32px.png").as_ref());
//     let mut logo_png = logo_png.read_info()?;
//     let mut icon_buf = vec![0; logo_png.output_buffer_size()];
//     logo_png.next_frame(&mut icon_buf)?;
//     let event_loop: EventLoop<Box<dyn FnOnce(&WebView) + Send + 'static>> =
//         EventLoop::with_user_event();
//     let logo_icon = Icon::from_rgba(icon_buf, logo_png.info().width, logo_png.info().height)?;
//     let window = WindowBuilder::new()
//         .with_inner_size(LogicalSize {
//             width: WINDOW_WIDTH,
//             height: WINDOW_HEIGHT,
//         })
//         .with_resizable(true)
//         .with_title("Geph")
//         .with_window_icon(Some(logo_icon))
//         .build(&event_loop)?;
//     eprintln!("resizable?: {}", window.is_resizable());
//     let initjs = include_str!("init.js").to_string();

//     #[cfg(target_os = "macos")]
//     // horrifying HACK
//     let initjs = initjs.replace("supports_vpn_conf: true", "supports_vpn_conf: false");

//     let webview = WebViewBuilder::new(window)?
//         .with_url(&format!("http://{}/index.html", SERVE_ADDR))?
//         .with_rpc_handler(global_rpc_handler)
//         .with_initialization_script(&initjs)
//         .with_web_context(&mut WebContext::new(dirs::config_dir()))
//         .build()?;

//     let evt_proxy = event_loop.create_proxy();
//     std::thread::spawn(move || loop {
//         let evt = mt_next();
//         evt_proxy.send_event(Box::new(evt)).ok().unwrap();
//     });

//     event_loop.run(move |event, _, control_flow| {
//         *control_flow = ControlFlow::Wait;

//         match event {
//             Event::NewEvents(StartCause::Init) => tracing::info!("Wry has started!"),
//             Event::WindowEvent {
//                 event: WindowEvent::CloseRequested,
//                 ..
//             } => {
//                 tracing::info!("receiving CloseRequested event");
//                 *control_flow = ControlFlow::Exit;
//                 std::process::exit(0);
//             }
//             Event::RedrawRequested(_) => {
//                 tracing::info!("REDRAW REQUESTED!!!!!!!!!!!!!!!!!!!!!!!");
//                 webview.resize().expect("cannot resize window");
//             }
//             Event::MenuEvent { .. } => webview.window().set_visible(true),
//             Event::UserEvent(e) => e(&webview),
//             Event::TrayEvent { .. } => webview.window().set_visible(true),
//             _ => {}
//         }
//     });
// }

async fn test(req: Request<()>) -> tide::Result {
    let url = req.url().path().trim_start_matches('/');
    if let Some(file) = FakeFs::get(url) {
        tracing::debug!("loaded embedded resource {}", url);
        let mime = mime_guess::from_path(url);
        let resp = tide::Response::new(200)
            .tap_mut(|r| r.set_content_type(mime.first_or_octet_stream().as_ref()))
            .tap_mut(|r| r.set_body(file.data.to_vec()));
        Ok(resp)
    } else if url.contains("proxy.pac") {
        Ok("function FindProxyForURL(url, host){return 'PROXY 127.0.0.1:9910';}".into())
    } else {
        tracing::error!("NO SUCH embedded resource {}", url);
        Err(tide::Error::new(404, anyhow::anyhow!("not found")))
    }
}

fn config_logging() {
    let subscriber = FmtSubscriber::builder()
        // .pretty()
        .with_max_level(Level::DEBUG)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
    tracing::debug!("Logging configured!")
}
