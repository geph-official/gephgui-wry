#![windows_subsystem = "windows"]

use autoupdate::autoupdate_loop;
use fakefs::FakeFs;
use mtbus::mt_next;

#[cfg(feature = "tray")]
use tao::system_tray::{SystemTray, SystemTrayBuilder};

use tap::Tap;
use tide::Request;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;
use wry::{
    application::{
        dpi::LogicalSize,
        event::{Event, StartCause, WindowEvent},
        event_loop::{ControlFlow, EventLoop},
        window::{Icon, WindowBuilder},
    },
    webview::{WebContext, WebView, WebViewBuilder},
};

mod autoupdate;
mod daemon;
mod fakefs;
mod mtbus;
mod pac;
mod rpc_handler;
use rpc_handler::global_rpc_handler;
const SERVE_ADDR: &str = "127.0.0.1:5678";

const WINDOW_WIDTH: i32 = 380;
const WINDOW_HEIGHT: i32 = 600;

fn main() -> anyhow::Result<()> {
    config_logging();
    smolscale::spawn(autoupdate_loop()).detach();
    // std::thread::sleep(Duration::from_secs(10));
    smolscale::spawn(async {
        let mut app = tide::new();
        app.at("/*").get(test);
        app.listen(SERVE_ADDR).await.expect("cannot listen to http");
    })
    .detach();
    wry_loop()
}

fn wry_loop() -> anyhow::Result<()> {
    eprintln!("entering wry loop");
    let logo_png = png::Decoder::new(include_bytes!("logo-naked-32px.png").as_ref());
    let mut logo_png = logo_png.read_info()?;
    let mut icon_buf = vec![0; logo_png.output_buffer_size()];
    logo_png.next_frame(&mut icon_buf)?;
    let event_loop: EventLoop<Box<dyn FnOnce(&WebView) + Send + 'static>> =
        EventLoop::with_user_event();
    let logo_icon = Icon::from_rgba(icon_buf, logo_png.info().width, logo_png.info().height)?;
    let window = WindowBuilder::new()
        .with_inner_size(LogicalSize {
            width: WINDOW_WIDTH,
            height: WINDOW_HEIGHT,
        })
        .with_resizable(true)
        .with_title("Geph")
        .with_window_icon(Some(logo_icon))
        .build(&event_loop)?;
    eprintln!("resizable?: {}", window.is_resizable());
    let initjs = include_str!("init.js").to_string();

    #[cfg(target_os = "macos")]
    // horrifying HACK
    let initjs = initjs.replace("supports_vpn_conf: true", "supports_vpn_conf: false");

    let webview = WebViewBuilder::new(window)?
        .with_url(&format!("http://{}/index.html", SERVE_ADDR))?
        .with_rpc_handler(global_rpc_handler)
        .with_initialization_script(&initjs)
        .with_web_context(&mut WebContext::new(dirs::config_dir()))
        .build()?;

    let evt_proxy = event_loop.create_proxy();
    std::thread::spawn(move || loop {
        let evt = mt_next();
        evt_proxy.send_event(Box::new(evt)).ok().unwrap();
    });

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(StartCause::Init) => tracing::info!("Wry has started!"),
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                tracing::info!("receiving CloseRequested event");
                *control_flow = ControlFlow::Exit;
                std::process::exit(0);
            }
            Event::RedrawRequested(_) => {
                tracing::info!("REDRAW REQUESTED!!!!!!!!!!!!!!!!!!!!!!!");
                webview.resize().expect("cannot resize window");
            }
            Event::MenuEvent { .. } => webview.window().set_visible(true),
            Event::UserEvent(e) => e(&webview),
            Event::TrayEvent { .. } => webview.window().set_visible(true),
            _ => {}
        }
    });
}

#[cfg(feature = "tray")]
fn create_systray<T>(event_loop: &EventLoop<T>) -> anyhow::Result<SystemTray> {
    use wry::application::menu::ContextMenu;
    use wry::application::menu::MenuItemAttributes;
    let mut tray_menu = ContextMenu::new();
    tray_menu.add_item(MenuItemAttributes::new("Open"));
    let icon = include_bytes!("logo-naked.ico").to_vec();
    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        let mut tmpfile = tempfile::NamedTempFile::new()?;
        tmpfile.write_all(&icon)?;
        tmpfile.flush()?;
        let path = tmpfile.path().to_owned();
        tmpfile.keep()?;
        Ok(SystemTrayBuilder::new(path, Some(tray_menu)).build(event_loop)?)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(SystemTrayBuilder::new(icon, Some(tray_menu)).build(event_loop)?)
    }
}

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
