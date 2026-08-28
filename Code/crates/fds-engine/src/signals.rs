//! Async-signal-safe Ctrl-C handling (no dependencies): a signal handler
//! that only stores to an atomic; the engine loop polls it.

use std::sync::atomic::{AtomicBool, Ordering};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

/// Install the SIGINT handler (idempotent).
pub(crate) fn install() {
    // SAFETY: the handler does only an atomic store, which is
    // async-signal-safe; libc::signal is safe to call once at startup.
    unsafe {
        libc::signal(libc::SIGINT, on_sigint as *const () as libc::sighandler_t);
    }
}

pub(crate) fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}
