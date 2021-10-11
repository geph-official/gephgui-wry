#![windows_subsystem = "windows"]

use fakefs::FakeFs;
use tap::Tap;
use tide::Request;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;
use wry::{
    application::{
        dpi::LogicalSize,
        event::{Event, StartCause, WindowEvent},
        event_loop::{ControlFlow, EventLoop},
        window::WindowBuilder,
    },
    webview::{WebContext, WebViewBuilder},
};

mod daemon;
mod fakefs;
mod interface;
mod pac;
use interface::global_rpc_handler;
const SERVE_ADDR: &str = "127.2.3.4:5678";

fn main() -> anyhow::Result<()> {
    config_logging();
    smolscale::spawn(async {
        let mut app = tide::new();
        app.at("/*").get(test);
        app.listen(SERVE_ADDR).await.expect("cannot listen to http");
    })
    .detach();
    wry_loop()
}

#[tracing::instrument]
fn wry_loop() -> anyhow::Result<()> {
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_inner_size(LogicalSize {
            width: 400,
            height: 610,
        })
        // .with_resizable(false)
        .with_title("Geph")
        .build(&event_loop)?;
    let webview = WebViewBuilder::new(window)?
        .with_url(&format!("http://{}/index.html", SERVE_ADDR))?
        .with_rpc_handler(global_rpc_handler)
        .with_web_context(&mut WebContext::new(dirs::config_dir()))
        .build()?;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(StartCause::Init) => tracing::info!("Wry has started!"),
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            Event::RedrawRequested(_) => {
                webview.resize().expect("cannot resize window");
            }
            _ => (),
        }
    });
}

#[tracing::instrument]
async fn test(req: Request<()>) -> tide::Result {
    let url = req.url().path().trim_start_matches('/');
    if let Some(file) = FakeFs::get(url) {
        tracing::debug!("loaded embedded resource {}", url);
        let mime = mime_guess::from_path(url);
        let resp = tide::Response::new(200)
            .tap_mut(|r| r.set_content_type(mime.first_or_octet_stream().as_ref()))
            .tap_mut(|r| r.set_body(file.data.to_vec()));
        Ok(resp)
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
