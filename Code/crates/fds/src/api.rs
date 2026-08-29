//! Stable application-facing API (standard \[IO\]).
//!
//! Two shapes over one core:
//!
//! 1. **Driver/callback shape** ([`Driver`]): register file
//!    descriptors with the kernel poller, poll for readiness, and read
//!    the delivered per-token [`Event`]s. This is the shape of the
//!    io_uring and epoll reactors. Implementations: [`EpollDriver`]
//!    (default) and [`IoUringDriver`] (feature `io-uring`).
//! 2. **Async shape** ([`AsyncRead`], [`AsyncWrite`], [`AsyncAccept`],
//!    [`AsyncDatagram`]): `poll_*` methods in the `std::task::Poll`
//!    shape. A driver provides readiness; the `poll_*` methods do the
//!    nonblocking work and return `Pending` when the kernel would
//!    block. This is the shape of the standard async traits; you can
//!    drive it with a no-op waker ([`noop_context`]) or with any async
//!    runtime that calls the `poll_*` methods.
//!
//! The concrete types ([`TcpStream`], [`TcpListener`], [`UdpSocket`])
//! wrap the fds transports and implement both shapes.
//!
//! # Example
//!
//! ```no_run
//! use fds::api::{AsyncAccept, Driver, EpollDriver, Interest, TcpListener, noop_context};
//! use std::os::fd::AsRawFd;
//! use std::task::Poll;
//!
//! let mut driver = EpollDriver::new(64).unwrap();
//! let mut listener = TcpListener::bind("127.0.0.1:0".parse().unwrap()).unwrap();
//! let lfd = listener.as_raw_fd();
//! driver.register(lfd, 0, Interest::Readable).unwrap();
//! let mut ctx = noop_context();
//! loop {
//!     driver.poll(Some(std::time::Duration::from_millis(100))).unwrap();
//!     for ev in driver.events() {
//!         if ev.token == 0 && ev.readable {
//!             if let Poll::Ready(Ok(Some(_))) = listener.poll_accept(&mut ctx) {
//!                 // handle the stream
//!             }
//!         }
//!     }
//!     driver.clear_events();
//! }
//! ```

use crate::reactor::Reactor;
use std::io;
use std::task::{Context, Poll, Waker};

pub use crate::reactor::Interest;

/// One delivered readiness event (token + ready flags).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Event {
    pub token: u64,
    pub readable: bool,
    pub writable: bool,
    pub hang_up: bool,
    pub error: bool,
}

/// A no-op waker context for driving `poll_*` methods without an async
/// runtime: readiness comes from a [`Driver`], so waker registration is
/// not needed.
pub fn noop_context() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

/// The driver/callback shape: register fds, poll for readiness, and
/// read the delivered events. Object-safe so applications can select
/// the backend at runtime.
pub trait Driver {
    /// Register `fd` for `interest` under `token`.
    fn register(&mut self, fd: i32, token: u64, interest: Interest) -> io::Result<()>;
    /// Change the interest of a registered fd.
    fn modify(&mut self, fd: i32, token: u64, interest: Interest) -> io::Result<()>;
    /// Remove a registration.
    fn unregister(&mut self, fd: i32) -> io::Result<()>;
    /// Poll once. `Some(timeout)` bounds the wait; `None` blocks until
    /// at least one event. Returns the number of events delivered.
    fn poll(&mut self, timeout: Option<std::time::Duration>) -> io::Result<usize>;
    /// The events delivered by the last `poll` (valid until the next
    /// `poll` or `clear_events`).
    fn events(&self) -> &[Event];
    /// Discard the delivered events.
    fn clear_events(&mut self);
}

/// epoll-backed [`Driver`]: wraps [`crate::reactor::Reactor`] with its
/// edge-triggered, drain-to-EAGAIN discipline.
pub struct EpollDriver {
    reactor: Reactor,
    events: Vec<Event>,
}

impl EpollDriver {
    /// Create a driver with a preallocated event array of `max_events`.
    pub fn new(max_events: usize) -> io::Result<Self> {
        Ok(EpollDriver {
            reactor: Reactor::new(max_events)?,
            events: Vec::with_capacity(max_events.max(1)),
        })
    }
}

impl Driver for EpollDriver {
    fn register(&mut self, fd: i32, token: u64, interest: Interest) -> io::Result<()> {
        self.reactor.register(fd, token, interest)
    }
    fn modify(&mut self, fd: i32, token: u64, interest: Interest) -> io::Result<()> {
        self.reactor.modify(fd, token, interest)
    }
    fn unregister(&mut self, fd: i32) -> io::Result<()> {
        self.reactor.unregister(fd)
    }
    fn poll(&mut self, timeout: Option<std::time::Duration>) -> io::Result<usize> {
        let t = timeout.map(|d| crate::reactor::PollTimeout {
            tv_sec: d.as_secs() as _,
            tv_nsec: d.subsec_nanos() as _,
        });
        let n = self.reactor.poll_timeout(t.as_ref())?;
        self.events.clear();
        let mut evbuf = vec![crate::reactor::EpollEvent::default(); n.max(1)];
        let m = self.reactor.copy_events(n, &mut evbuf);
        self.events.extend(
            evbuf
                .iter()
                .take(m)
                .map(|e| Event {
                    token: e.token,
                    readable: e.readable,
                    writable: e.writable,
                    hang_up: e.hang_up,
                    error: e.error,
                }),
        );
        Ok(m)
    }
    fn events(&self) -> &[Event] {
        &self.events
    }
    fn clear_events(&mut self) {
        self.events.clear();
    }
}

/// io_uring-backed [`Driver`] (feature `io-uring`): readiness via
/// single-shot `IORING_OP_POLL_ADD` ops. Each registered fd has one
/// poll op in flight; a completion wakes the poller and the op is
/// re-armed immediately. `modify` cancels the in-flight op and
/// submits a new one; the per-token in-flight count keeps the two
/// completions (cancel + readiness) from double-arming.
#[cfg(feature = "io-uring")]
pub struct IoUringDriver {
    reactor: crate::io_uring_reactor::IoUringReactor,
    events: Vec<Event>,
    /// token -> (fd, interest, polls in flight).
    registrations: std::collections::HashMap<u64, (i32, Interest, u32)>,
}

#[cfg(feature = "io-uring")]
impl IoUringDriver {
    /// Create a driver over a ring with `entries` SQEs.
    pub fn new(entries: u32) -> io::Result<Self> {
        Ok(IoUringDriver {
            reactor: crate::io_uring_reactor::IoUringReactor::new(entries, 0)?,
            events: Vec::new(),
            registrations: std::collections::HashMap::new(),
        })
    }

    fn poll_flags(i: Interest) -> u32 {
        let mut f = libc::POLLERR as u32 | libc::POLLHUP as u32;
        match i {
            Interest::Readable => f |= libc::POLLIN as u32,
            Interest::Writable => f |= libc::POLLOUT as u32,
            Interest::ReadableWritable => f |= libc::POLLIN as u32 | libc::POLLOUT as u32,
        }
        f
    }
}

#[cfg(feature = "io-uring")]
impl Driver for IoUringDriver {
    fn register(&mut self, fd: i32, token: u64, interest: Interest) -> io::Result<()> {
        if self.registrations.contains_key(&token) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "token registered"));
        }
        self.reactor.submit_poll(fd, Self::poll_flags(interest), token)?;
        self.registrations.insert(token, (fd, interest, 1));
        Ok(())
    }

    fn modify(&mut self, fd: i32, token: u64, interest: Interest) -> io::Result<()> {
        let Some(&(_, _, in_flight)) = self.registrations.get(&token) else {
            return self.register(fd, token, interest);
        };
        // Cancel the old poll and arm a new one with the new interest.
        // The cancel completion (skipped below) balances the count.
        let _ = self.reactor.ring_cancel(token);
        self.reactor.submit_poll(fd, Self::poll_flags(interest), token)?;
        self.registrations.insert(token, (fd, interest, in_flight + 1));
        Ok(())
    }

    fn unregister(&mut self, fd: i32) -> io::Result<()> {
        let token = match self
            .registrations
            .iter()
            .find(|(_, (f, _, _))| *f == fd)
            .map(|(t, _)| *t)
        {
            Some(t) => t,
            None => return Ok(()),
        };
        let _ = self.reactor.ring_cancel(token);
        self.registrations.remove(&token);
        Ok(())
    }

    fn poll(&mut self, timeout: Option<std::time::Duration>) -> io::Result<usize> {
        if timeout.is_none() {
            self.reactor.submit_and_wait(1)?;
        } else {
            self.reactor.submit_all()?;
        }
        self.events.clear();
        let mut completions: Vec<(u64, io::Result<u32>)> = Vec::with_capacity(64);
        self.reactor
            .drain(|ud, res| completions.push((ud, res)));
        for (token, res) in completions {
            let Some(&(fd, interest, in_flight)) = self.registrations.get(&token) else {
                continue; // unregistered while in flight
            };
            let cancelled = res
                .as_ref()
                .err()
                .and_then(|e| e.raw_os_error())
                == Some(libc::ECANCELED);
            let in_flight = in_flight.saturating_sub(1);
            if !cancelled {
                let ok = res.is_ok();
                self.events.push(Event {
                    token,
                    readable: ok && matches!(interest, Interest::Readable | Interest::ReadableWritable),
                    writable: ok && matches!(interest, Interest::Writable | Interest::ReadableWritable),
                    hang_up: !ok,
                    error: !ok,
                });
            }
            if in_flight == 0 {
                // The last completion for this token: re-arm the poll.
                self.reactor.submit_poll(fd, Self::poll_flags(interest), token)?;
                self.registrations.insert(token, (fd, interest, 1));
            } else {
                self.registrations.insert(token, (fd, interest, in_flight));
            }
        }
        Ok(self.events.len())
    }

    fn events(&self) -> &[Event] {
        &self.events
    }

    fn clear_events(&mut self) {
        self.events.clear();
    }
}

// ---------------------------------------------------------------------
// Async shape
// ---------------------------------------------------------------------

/// The async read shape: nonblocking read in `std::task::Poll` form.
pub trait AsyncRead {
    /// Read into `buf`. `Ready(Ok(0))` is EOF. `Pending` means the
    /// kernel would block; poll a [`Driver`] for readiness first.
    fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>>;
}

/// The async write shape: nonblocking write in `std::task::Poll` form.
pub trait AsyncWrite {
    /// Write `buf`. `Ready(Ok(n))` reports `n` bytes accepted by the
    /// kernel. `Pending` means the kernel would block.
    fn poll_write(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>>;
    /// Report whether all buffered data has reached the kernel.
    fn poll_flush(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>>;
}

/// The async accept shape.
pub trait AsyncAccept {
    type Stream;
    /// Accept the next connection. `Ready(Ok(None))` means the
    /// accept queue is empty (drain to EAGAIN).
    fn poll_accept(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<Option<Self::Stream>>>;
}

/// The async datagram shape.
pub trait AsyncDatagram {
    /// Receive one datagram into `buf`; `Ready(Ok((n, src)))`.
    fn poll_recv_from(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<(usize, std::net::SocketAddr)>>;
    /// Send one datagram to `dst`; `Ready(Ok(n))`.
    fn poll_send_to(
        &mut self,
        cx: &mut Context<'_>,
        buf: &[u8],
        dst: std::net::SocketAddr,
    ) -> Poll<io::Result<usize>>;
}

/// An async TCP stream: nonblocking read and write over the fds TCP
/// transport. Wrap it in a [`crate::conn::ConnectionSlot`]-backed table
/// in a server; the echo engine's patterns apply.
pub struct TcpStream {
    inner: crate::tcp::TcpStream,
}

impl TcpStream {
    /// Adopt a transport stream (e.g. from [`TcpListener::poll_accept`]).
    pub fn new(inner: crate::tcp::TcpStream) -> Self {
        TcpStream { inner }
    }
    /// The raw fd, for registration with a [`Driver`].
    pub fn as_raw_fd(&self) -> i32 {
        self.inner.as_raw_fd()
    }
}

impl AsyncRead for TcpStream {
    fn poll_read(&mut self, _cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        match self.inner.read(buf) {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Poll::Pending,
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

impl AsyncWrite for TcpStream {
    fn poll_write(&mut self, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.inner.write(buf) {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Poll::Pending,
            Err(e) => Poll::Ready(Err(e)),
        }
    }
    fn poll_flush(&mut self, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // No userspace buffering: every accepted byte is in the kernel.
        Poll::Ready(Ok(()))
    }
}

/// An async TCP listener.
pub struct TcpListener {
    inner: crate::tcp::TcpListener,
}

impl TcpListener {
    /// Bind + listen on `addr`.
    pub fn bind(addr: std::net::SocketAddr) -> io::Result<Self> {
        Ok(TcpListener {
            inner: crate::tcp::TcpListener::bind(addr, &crate::config::TcpConfig::default(), 128)?,
        })
    }
    /// Bind + listen with an explicit transport configuration.
    pub fn bind_with(
        addr: std::net::SocketAddr,
        cfg: &crate::config::TcpConfig,
        backlog: i32,
    ) -> io::Result<Self> {
        Ok(TcpListener {
            inner: crate::tcp::TcpListener::bind(addr, cfg, backlog)?,
        })
    }
    /// The raw fd, for registration with a [`Driver`].
    pub fn as_raw_fd(&self) -> i32 {
        self.inner.as_raw_fd()
    }
    pub fn local_addr(&self) -> io::Result<std::net::SocketAddr> {
        self.inner.local_addr()
    }
}

impl AsyncAccept for TcpListener {
    type Stream = TcpStream;
    fn poll_accept(
        &mut self,
        _cx: &mut Context<'_>,
    ) -> Poll<io::Result<Option<Self::Stream>>> {
        match self.inner.accept() {
            Ok(Some((stream, _peer))) => Poll::Ready(Ok(Some(TcpStream::new(stream)))),
            Ok(None) => Poll::Ready(Ok(None)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Poll::Pending,
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

/// An async UDP socket.
pub struct UdpSocket {
    inner: crate::udp::UdpSocket,
}

impl UdpSocket {
    /// Bind on `addr` with default transport settings.
    pub fn bind(addr: std::net::SocketAddr) -> io::Result<Self> {
        Ok(UdpSocket {
            inner: crate::udp::UdpSocket::new(addr, &crate::config::UdpConfig::default())?,
        })
    }
    /// Bind with an explicit transport configuration.
    pub fn bind_with(
        addr: std::net::SocketAddr,
        cfg: &crate::config::UdpConfig,
    ) -> io::Result<Self> {
        Ok(UdpSocket {
            inner: crate::udp::UdpSocket::new(addr, cfg)?,
        })
    }
    /// The raw fd, for registration with a [`Driver`].
    pub fn as_raw_fd(&self) -> i32 {
        self.inner.as_raw_fd()
    }
    pub fn local_addr(&self) -> io::Result<std::net::SocketAddr> {
        self.inner.local_addr()
    }
}

impl AsyncDatagram for UdpSocket {
    fn poll_recv_from(
        &mut self,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<(usize, std::net::SocketAddr)>> {
        match self.inner.recv_from(buf) {
            Ok((n, src)) => Poll::Ready(Ok((n, src))),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Poll::Pending,
            Err(e) => Poll::Ready(Err(e)),
        }
    }
    fn poll_send_to(
        &mut self,
        _cx: &mut Context<'_>,
        buf: &[u8],
        dst: std::net::SocketAddr,
    ) -> Poll<io::Result<usize>> {
        match self.inner.send_to(buf, dst) {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Poll::Pending,
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::time::Duration;

    /// Drive a full-duplex echo over the API with the epoll driver:
    /// accept, register both ends, and exchange 64 KiB each way.
    #[test]
    fn api_epoll_full_duplex_echo() {
        let mut driver = EpollDriver::new(64).unwrap();
        let mut listener = TcpListener::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let laddr = listener.local_addr().unwrap();
        let lfd = listener.as_raw_fd();
        driver.register(lfd, 1, Interest::Readable).unwrap();

        let mut ctx = noop_context();
        let mut peer = std::net::TcpStream::connect(laddr).unwrap();
        peer.set_nonblocking(true).unwrap();
        peer.write_all(&[0x5A; 65536]).unwrap();

        let mut server: Option<TcpStream> = None;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while server.is_none() && std::time::Instant::now() < deadline {
            driver.poll(Some(Duration::from_millis(20))).unwrap();
            for ev in driver.events() {
                if ev.token == 1 && ev.readable {
                    if let Poll::Ready(Ok(Some(s))) = listener.poll_accept(&mut ctx) {
                        server = Some(s);
                    }
                }
            }
            driver.clear_events();
        }
        let mut server = server.expect("no accept within 5s");
        let sfd = server.as_raw_fd();
        driver.register(sfd, 2, Interest::Readable).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();

        // Echo loop: read 64 KiB from the peer, write it back.
        let mut buf = [0u8; 65536];
        let mut got = 0usize;
        while got < 65536 && std::time::Instant::now() < deadline {
            driver.poll(Some(Duration::from_millis(20))).unwrap();
            for ev in driver.events() {
                if ev.token == 2 && ev.readable {
                    match server.poll_read(&mut ctx, &mut buf[got..]) {
                        Poll::Ready(Ok(0)) => panic!("unexpected EOF"),
                        Poll::Ready(Ok(n)) => {
                            got += n;
                            let mut off = 0;
                            while off < n {
                                match server.poll_write(&mut ctx, &buf[got - n + off..got]) {
                                    Poll::Ready(Ok(w)) => off += w,
                                    _ => break,
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            driver.clear_events();
        }
        assert_eq!(got, 65536, "server did not read the full payload");

        let mut echo = [0u8; 65536];
        peer.read_exact(&mut echo).expect("echo round trip");
        assert!(echo.iter().all(|&b| b == 0x5A));
    }

    /// Datagram round trip over the API with the epoll driver.
    #[test]
    fn api_epoll_udp_echo() {
        let mut driver = EpollDriver::new(64).unwrap();
        let mut server = UdpSocket::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let saddr = server.local_addr().unwrap();
        driver.register(server.as_raw_fd(), 7, Interest::Readable).unwrap();

        let mut ctx = noop_context();
        let client = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        client.send_to(b"api udp echo", saddr).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut got = 0usize;
        let mut buf = [0u8; 64];
        while got == 0 && std::time::Instant::now() < deadline {
            driver.poll(Some(Duration::from_millis(20))).unwrap();
            for ev in driver.events() {
                if ev.token == 7 && ev.readable {
                    if let Poll::Ready(Ok((n, src))) = server.poll_recv_from(&mut ctx, &mut buf) {
                        got = n;
                        let _ = server.poll_send_to(&mut ctx, &buf[..n], src);
                    }
                }
            }
            driver.clear_events();
        }
        assert_eq!(got, b"api udp echo".len());
        let mut rbuf = [0u8; 64];
        client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let n = client.recv(&mut rbuf).unwrap();
        assert_eq!(&rbuf[..n], b"api udp echo");
    }

    /// io_uring-backed driver: same full-duplex echo, proving the
    /// backend is interchangeable behind the [`Driver`] trait.
    #[cfg(feature = "io-uring")]
    #[test]
    fn api_io_uring_full_duplex_echo() {
        let mut driver = IoUringDriver::new(64).unwrap();
        let mut listener = TcpListener::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let laddr = listener.local_addr().unwrap();
        let lfd = listener.as_raw_fd();
        driver.register(lfd, 1, Interest::Readable).unwrap();

        let mut ctx = noop_context();
        let mut peer = std::net::TcpStream::connect(laddr).unwrap();
        peer.set_nonblocking(true).unwrap();
        peer.write_all(&[0x3C; 8192]).unwrap();

        let mut server: Option<TcpStream> = None;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while server.is_none() && std::time::Instant::now() < deadline {
            driver.poll(Some(Duration::from_millis(20))).unwrap();
            for ev in driver.events() {
                if ev.token == 1 && ev.readable {
                    if let Poll::Ready(Ok(Some(s))) = listener.poll_accept(&mut ctx) {
                        server = Some(s);
                    }
                }
            }
            driver.clear_events();
        }
        let mut server = server.expect("no accept within 5s");
        let sfd = server.as_raw_fd();
        driver.register(sfd, 2, Interest::Readable).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();

        let mut buf = [0u8; 8192];
        let mut got = 0usize;
        while got < 8192 && std::time::Instant::now() < deadline {
            driver.poll(Some(Duration::from_millis(20))).unwrap();
            for ev in driver.events() {
                if ev.token == 2 && ev.readable {
                    match server.poll_read(&mut ctx, &mut buf[got..]) {
                        Poll::Ready(Ok(0)) => panic!("unexpected EOF"),
                        Poll::Ready(Ok(n)) => {
                            got += n;
                            let mut off = 0;
                            while off < n {
                                match server.poll_write(&mut ctx, &buf[got - n + off..got]) {
                                    Poll::Ready(Ok(w)) => off += w,
                                    _ => break,
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            driver.clear_events();
        }
        assert_eq!(got, 8192);
        let mut echo = [0u8; 8192];
        peer.read_exact(&mut echo).unwrap();
        assert!(echo.iter().all(|&b| b == 0x3C));
    }
}
