//! Small platform utilities shared by the engine and by applications
//! building on the FDS primitives.

/// Pin the calling thread to logical CPU `core` (`sched_setaffinity`).
pub fn pin_to_core(core: usize) -> std::io::Result<()> {
    let mut set = rustix::thread::CpuSet::new();
    set.set(core);
    rustix::thread::sched_setaffinity(None, &set).map_err(std::io::Error::from)
}

/// Coarse monotonic ticks (seconds since first call) for hot-state
/// activity stamps; no clock syscall per packet (Instant::elapsed reads
/// a vDSO time). Shared by the epoll and io_uring datapaths.
pub fn now_ticks() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_advance() {
        let a = now_ticks();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = now_ticks();
        assert!(b >= a);
    }

    #[test]
    fn pin_zero_is_ok_or_unavailable() {
        // Pinning to CPU 0 either works or reports EINVAL (no such CPU
        // / no permission); it must never panic.
        if let Ok(()) = pin_to_core(0) {}
    }
}
