use once_cell::sync::Lazy;
use tao::window::Window;
use wry::WebView;

#[allow(clippy::type_complexity)]
static BUS: Lazy<(
    flume::Sender<Box<dyn FnOnce(&WebView, &Window) + Send + 'static>>,
    flume::Receiver<Box<dyn FnOnce(&WebView, &Window) + Send + 'static>>,
)> = Lazy::new(flume::unbounded);

pub fn mt_enqueue(f: impl FnOnce(&WebView, &Window) + Send + 'static) {
    BUS.0.send(Box::new(f)).unwrap()
}

pub fn mt_next() -> impl FnOnce(&WebView, &Window) {
    BUS.1.recv().unwrap()
}
