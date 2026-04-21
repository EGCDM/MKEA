use std::sync::atomic::{AtomicBool, Ordering};

static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn request_stop() {
    STOP_REQUESTED.store(true, Ordering::SeqCst);
}

pub fn clear_stop_request() {
    STOP_REQUESTED.store(false, Ordering::SeqCst);
}

pub fn is_stop_requested() -> bool {
    STOP_REQUESTED.load(Ordering::SeqCst)
}
