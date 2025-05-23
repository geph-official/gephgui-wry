use once_cell::sync::Lazy;
use tao::{event_loop::EventLoopWindowTarget, window::Window};
use wry::WebView;
use tao::system_tray::SystemTray;

pub type MtHandler<T> = Box<
    dyn FnOnce(&WebView, &Window, &EventLoopWindowTarget<T>, &mut Option<SystemTray>, &mut bool)
        + Send
        + 'static,
>;

#[allow(clippy::type_complexity)]
static BUS: Lazy<(flume::Sender<MtHandler<()>>, flume::Receiver<MtHandler<()>>)> =
    Lazy::new(flume::unbounded);

pub fn mt_enqueue(f: MtHandler<()>) {
    BUS.0.send(f).unwrap()
}

pub fn mt_next() -> MtHandler<()> {
    BUS.1.recv().unwrap()
}
