use once_cell::sync::Lazy;
use wry::WebView;

#[allow(clippy::type_complexity)]
static BUS: Lazy<(
    flume::Sender<Box<dyn FnOnce(&WebView) + Send + 'static>>,
    flume::Receiver<Box<dyn FnOnce(&WebView) + Send + 'static>>,
)> = Lazy::new(flume::unbounded);

pub fn mt_enqueue(f: impl FnOnce(&WebView) + Send + 'static) {
    BUS.0.send(Box::new(f)).unwrap()
}

pub fn mt_next() -> impl FnOnce(&WebView) {
    BUS.1.recv().unwrap()
}
