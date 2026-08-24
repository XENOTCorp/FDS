//! Experimental io_uring reactor path (feature `io-uring`, links
//! liburing via the `io-uring` crate): SQPOLL-capable setup, registered
//! buffers (IORING_REGISTER_BUFFERS), and a reactor loop that submits
//! read/write/accept requests with a completion drain. EXPERIMENTAL: not
//! the default reactor (epoll busy-poll is); this module exists to
//! compile-check and benchmark the alternative per spec D-5.
//!
//! CONTRACT (implementer): use the `io-uring` crate (tokio-rs) with the
//! `liburing` feature against the system liburing. Implement
//! [`IoUringReactor`] with the public API below. Tests: setup +
//! registration succeeds on this kernel (io_uring is available); a
//! socketpair read/write roundtrip through the ring; SQPOLL setup skips
//! gracefully (needs CAP_SYS_ADMIN) without failing the test suite.

/// An io_uring reactor instance.
pub(crate) struct IoUringReactor {
    // CONTRACT: implementer wraps io_uring::IoUring + registered buffers.
    _private: (),
}

impl IoUringReactor {
    /// Set up an io_uring instance with `entries` and `sq_thread` entries
    /// (0 = no SQPOLL thread).
    pub(crate) fn new(entries: u32, sq_thread: u32) -> std::io::Result<Self> {
        let _ = (entries, sq_thread);
        todo!("IoUringReactor::new: implemented by fds-core milestone task")
    }

    /// Register `bufs` with IORING_REGISTER_BUFFERS (returns Err when
    /// unsupported — caller falls back).
    pub(crate) fn register_buffers(&mut self, bufs: &mut [&mut [u8]]) -> std::io::Result<()> {
        let _ = bufs;
        todo!("IoUringReactor::register_buffers: implemented by fds-core milestone task")
    }

    /// Submit a read on `fd` into `buf`, with `user_data` as the token.
    pub(crate) fn submit_read(&mut self, fd: i32, buf: &mut [u8], user_data: u64) -> std::io::Result<()> {
        let _ = (fd, buf, user_data);
        todo!("IoUringReactor::submit_read: implemented by fds-core milestone task")
    }

    /// Submit a write of `data` on `fd` with `user_data` as the token.
    pub(crate) fn submit_write(&mut self, fd: i32, data: &[u8], user_data: u64) -> std::io::Result<()> {
        let _ = (fd, data, user_data);
        todo!("IoUringReactor::submit_write: implemented by fds-core milestone task")
    }

    /// Drain completed entries, calling `f(token, result)`. Returns the
    /// number of completions.
    pub(crate) fn drain<F: FnMut(u64, std::io::Result<u32>)>(&mut self, f: F) -> usize {
        let _ = f;
        todo!("IoUringReactor::drain: implemented by fds-core milestone task")
    }
}
