use once_cell::sync::Lazy;

#[allow(clippy::type_complexity)]
static BUS: Lazy<(
    flume::Sender<Box<dyn FnOnce() + Send + 'static>>,
    flume::Receiver<Box<dyn FnOnce() + Send + 'static>>,
)> = Lazy::new(flume::unbounded);

pub fn mt_enqueue(f: impl FnOnce() + Send + 'static) {
    BUS.0.send(Box::new(f)).unwrap()
}

pub fn mt_next() -> impl FnOnce() {
    BUS.1.recv().unwrap()
}
