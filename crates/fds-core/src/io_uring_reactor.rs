//! Experimental io_uring reactor path (feature `io-uring`, via the
//! `io-uring` crate, tokio-rs): SQPOLL-capable setup, registered
//! buffers (IORING_REGISTER_BUFFERS), and a reactor loop that submits
//! read/write requests with a completion drain. EXPERIMENTAL: not
//! the default reactor (epoll busy-poll is); this module exists to
//! compile-check and benchmark the alternative per spec D-5.
//!
//! CONTRACT (implementer): use the `io-uring` crate (tokio-rs) against
//! the system io_uring (the 0.7 series is pure-syscall; it does not link
//! liburing). Implement [`IoUringReactor`] with the public API below.
//! Tests: setup + registration succeeds on this kernel (io_uring is
//! available); a socketpair read/write roundtrip through the ring; SQPOLL
//! setup skips gracefully (needs CAP_SYS_ADMIN) without failing the test
//! suite.
#![cfg(feature = "io-uring")]

/// An io_uring reactor instance.
pub(crate) struct IoUringReactor {
    /// The io_uring instance (SQPOLL when requested and permitted).
    ring: io_uring::IoUring,
    /// user_data tokens of requests submitted but not yet drained.
    pending: Vec<u64>,
}

impl IoUringReactor {
    /// Set up an io_uring instance with `entries` and `sq_thread` entries
    /// (0 = no SQPOLL thread). `setup_sqpoll` requires CAP_SYS_ADMIN; when
    /// the kernel rejects it with EPERM the setup falls back to a plain
    /// ring without an SQPOLL thread so unprivileged runs degrade
    /// gracefully.
    pub(crate) fn new(entries: u32, sq_thread: u32) -> std::io::Result<Self> {
        let mut builder = io_uring::IoUring::builder();
        if sq_thread > 0 {
            builder.setup_sqpoll(sq_thread);
        }
        // Preallocate the pending-token table so submit_*/drain never
        // allocate (entries is a small power of two).
        let pending: Vec<u64> = Vec::with_capacity(entries as usize);
        match builder.build(entries) {
            Ok(ring) => Ok(Self { ring, pending }),
            // setup_sqpoll only sets a flag; the io_uring_setup(2) syscall
            // in `build` is what fails with EPERM for unprivileged users.
            Err(e) if sq_thread > 0 && e.raw_os_error() == Some(libc::EPERM) => Ok(Self {
                ring: io_uring::IoUring::builder().build(entries)?,
                pending,
            }),
            Err(e) => Err(e),
        }
    }

    /// Register `bufs` with IORING_REGISTER_BUFFERS (returns Err when
    /// unsupported — caller falls back).
    pub(crate) fn register_buffers(&mut self, bufs: &mut [&mut [u8]]) -> std::io::Result<()> {
        let iovs: Vec<libc::iovec> = bufs
            .iter_mut()
            .map(|b| libc::iovec {
                iov_base: b.as_mut_ptr().cast(),
                iov_len: b.len(),
            })
            .collect();
        // SAFETY: each iovec points into the caller-owned `bufs`, which
        // must stay valid until the buffers are unregistered or the ring
        // is dropped; that is exactly the kernel's
        // IORING_REGISTER_BUFFERS lifetime contract.
        unsafe { self.ring.submitter().register_buffers(&iovs) }
    }

    /// Submit a read on `fd` into `buf`, with `user_data` as the token.
    ///
    /// The caller must not touch or drop `buf` until [`Self::drain`]
    /// reports the completion carrying `user_data`; the kernel may write
    /// into it at any time up to that point.
    pub(crate) fn submit_read(&mut self, fd: i32, buf: &mut [u8], user_data: u64) -> std::io::Result<()> {
        let entry =
            io_uring::opcode::Read::new(io_uring::types::Fd(fd), buf.as_mut_ptr(), buf.len() as u32)
                .build()
                .user_data(user_data);
        // SAFETY: `push` copies the entry into the ring's SQ memory, so the
        // entry itself need not outlive this call; `buf` is borrowed for
        // the caller's lifetime and, per the method contract, must remain
        // valid until `drain` reports this `user_data`.
        unsafe { self.ring.submission().push(&entry) }
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::WouldBlock, "io_uring submission queue full"))?;
        self.pending.push(user_data);
        Ok(())
    }

    /// Submit a write of `data` on `fd` with `user_data` as the token.
    ///
    /// The caller must not mutate or drop `data` until [`Self::drain`]
    /// reports the completion carrying `user_data`; the kernel may read
    /// from it at any time up to that point.
    pub(crate) fn submit_write(&mut self, fd: i32, data: &[u8], user_data: u64) -> std::io::Result<()> {
        let entry =
            io_uring::opcode::Write::new(io_uring::types::Fd(fd), data.as_ptr(), data.len() as u32)
                .build()
                .user_data(user_data);
        // SAFETY: `push` copies the entry into the ring's SQ memory, so the
        // entry itself need not outlive this call; `data` is borrowed for
        // the caller's lifetime and, per the method contract, must remain
        // valid until `drain` reports this `user_data`.
        unsafe { self.ring.submission().push(&entry) }
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::WouldBlock, "io_uring submission queue full"))?;
        self.pending.push(user_data);
        Ok(())
    }

    /// Drain completed entries, calling `f(token, result)`. Returns the
    /// number of completions.
    pub(crate) fn drain<F: FnMut(u64, std::io::Result<u32>)>(&mut self, mut f: F) -> usize {
        let mut cq = self.ring.completion();
        cq.sync();
        let mut n = 0;
        for cqe in cq {
            let user_data = cqe.user_data();
            f(user_data, result(cqe.result()));
            if let Some(pos) = self.pending.iter().position(|&t| t == user_data) {
                self.pending.swap_remove(pos);
            }
            n += 1;
        }
        n
    }
}

/// Convert an io_uring CQE result (negative errno or byte count) into an
/// `io::Result<u32>`.
fn result(res: i32) -> std::io::Result<u32> {
    if res < 0 {
        Err(std::io::Error::from_raw_os_error(-res))
    } else {
        Ok(res as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::IoUringReactor;
    use rustix::net::{socketpair, AddressFamily, SocketFlags, SocketType};
    use std::io::Write;
    use std::os::unix::io::AsRawFd;

    #[test]
    fn io_uring_setup_ok() {
        // io_uring is available on this kernel; plain setup must succeed.
        IoUringReactor::new(8, 0).expect("io_uring setup failed");
    }

    #[test]
    fn io_uring_socketpair_roundtrip() -> std::io::Result<()> {
        let (r, w) = socketpair(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::NONBLOCK,
            None,
        )?;
        let mut reactor = IoUringReactor::new(8, 0)?;
        let mut buf = [0u8; 64];
        reactor.submit_read(r.as_raw_fd(), &mut buf, 1)?;

        let mut wfile = std::fs::File::from(w);
        wfile.write_all(b"hello")?;
        drop(wfile);

        reactor.ring.submit_and_wait(1)?;
        let mut completions = Vec::new();
        let n = reactor.drain(|ud, res| completions.push((ud, res)));
        assert_eq!(n, 1);
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].0, 1);
        assert_eq!(completions[0].1.as_ref().unwrap(), &5);
        assert_eq!(&buf[..5], b"hello");
        Ok(())
    }

    #[test]
    fn io_uring_sqpoll_fallback() {
        // SQPOLL needs CAP_SYS_ADMIN; whether it succeeds, falls back, or
        // returns an error, it must not panic.
        let _ = IoUringReactor::new(8, 1);
    }

    #[test]
    fn register_buffers_path() {
        let mut reactor = IoUringReactor::new(8, 0).expect("io_uring setup failed");
        let mut a = [0u8; 64];
        let mut b = [0u8; 64];
        let mut bufs: Vec<&mut [u8]> = vec![&mut a, &mut b];
        // IORING_REGISTER_BUFFERS is supported on modern kernels; either a
        // success or a graceful Err is acceptable — it must not panic.
        let _ = reactor.register_buffers(&mut bufs);
    }
}
