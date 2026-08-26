//! The reactor core: edge-triggered epoll with a drain-to-EAGAIN
//! busy-poll discipline (standard \[IO\]; thesis ch. 10 reactor-as-trace).
//!
//! A [`Reactor`] owns one epoll instance and a preallocated event array.
//! Every registered fd is edge-triggered: after an event fires, the
//! handler MUST drain the fd until EAGAIN before returning, otherwise no
//! further edge is generated and events are lost. [`Reactor::poll_busy`]
//! busy-polls (timeout 0) until the ready list is empty.

use rustix::event::epoll;
use rustix::event::Timespec;
use rustix::fd::OwnedFd;
use std::os::fd::AsRawFd;

/// Re-exported so consumers can build poll timeouts without depending on
/// rustix directly.
pub use rustix::event::Timespec as PollTimeout;

/// The events a registration is interested in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interest {
    Readable,
    Writable,
    ReadableWritable,
}

impl Interest {
    fn flags(self) -> epoll::EventFlags {
        let f = match self {
            Interest::Readable => epoll::EventFlags::IN,
            Interest::Writable => epoll::EventFlags::OUT,
            Interest::ReadableWritable => epoll::EventFlags::IN | epoll::EventFlags::OUT,
        };
        // Edge-triggered, always: the drain-to-EAGAIN discipline is the
        // engine's hard policy.
        f | epoll::EventFlags::ET
    }
}

/// One delivered event: the token (a packed
/// [`crate::conn::ConnectionId`] or a reserved `TOKEN_*`) and the ready
/// flags.
#[derive(Clone, Copy, Debug, Default)]
pub struct EpollEvent {
    pub token: u64,
    pub readable: bool,
    pub writable: bool,
    pub hang_up: bool,
    pub error: bool,
}

impl EpollEvent {
    fn from_raw(e: &epoll::Event) -> Self {
        let f = e.flags;
        EpollEvent {
            token: e.data.u64(),
            readable: f.contains(epoll::EventFlags::IN),
            writable: f.contains(epoll::EventFlags::OUT),
            hang_up: f.contains(epoll::EventFlags::HUP),
            error: f.contains(epoll::EventFlags::ERR),
        }
    }
}

/// One epoll instance with a preallocated event array (allocated once at
/// startup; the hot path never allocates).
pub struct Reactor {
    ep: OwnedFd,
    events: Vec<epoll::Event>,
}

impl Reactor {
    /// Create a reactor with a preallocated event array of `max_events`
    /// entries (clamped to at least 1).
    pub fn new(max_events: usize) -> std::io::Result<Self> {
        let ep = epoll::create(epoll::CreateFlags::CLOEXEC)?;
        // Preallocated: only the first `n` written by epoll_wait are read.
        let events = vec![
            epoll::Event {
                data: epoll::EventData::new_u64(0),
                flags: epoll::EventFlags::empty(),
            };
            max_events.max(1)
        ];
        Ok(Reactor { ep, events })
    }

    /// Register `fd` for `interest` with token `token` (EPOLL_CTL_ADD).
    pub fn register(&self, fd: i32, token: u64, interest: Interest) -> std::io::Result<()> {
        let data = epoll::EventData::new_u64(token);
        // SAFETY: `fd` is a live descriptor owned by the caller for the
        // duration of the call; epoll_ctl installs the interest into the
        // kernel without retaining the BorrowedFd.
        let borrowed = unsafe { rustix::fd::BorrowedFd::borrow_raw(fd) };
        epoll::add(&self.ep, borrowed, data, interest.flags()).map_err(std::io::Error::from)
    }

    /// Change the interest for an already-registered fd (EPOLL_CTL_MOD).
    pub fn modify(&self, fd: i32, token: u64, interest: Interest) -> std::io::Result<()> {
        let data = epoll::EventData::new_u64(token);
        // SAFETY: as in [`Reactor::register`].
        let borrowed = unsafe { rustix::fd::BorrowedFd::borrow_raw(fd) };
        epoll::modify(&self.ep, borrowed, data, interest.flags()).map_err(std::io::Error::from)
    }

    /// Remove a registration (EPOLL_CTL_DEL).
    pub fn unregister(&self, fd: i32) -> std::io::Result<()> {
        // SAFETY: as in [`Reactor::register`].
        let borrowed = unsafe { rustix::fd::BorrowedFd::borrow_raw(fd) };
        epoll::delete(&self.ep, borrowed).map_err(std::io::Error::from)
    }

    /// Poll once with the given timeout: `None` blocks, `Some(0)` is a
    /// non-blocking busy poll. Returns the number of events delivered.
    pub fn poll_timeout(&mut self, timeout: Option<&Timespec>) -> std::io::Result<usize> {
        let n = epoll::wait(&self.ep, &mut self.events, timeout)?;
        Ok(n)
    }

    /// One zero-timeout poll: returns the events of a single epoll batch.
    /// Pairs with [`Reactor::delivered`]; unlike [`Reactor::poll_busy`],
    /// it never drains multiple batches, so the delivered set is complete.
    pub fn poll_once(&mut self) -> std::io::Result<usize> {
        let zero = Timespec { tv_sec: 0, tv_nsec: 0 };
        let n = epoll::wait(&self.ep, &mut self.events, Some(&zero))?;
        Ok(n)
    }

    /// Busy-poll: drain the ready list with timeout 0 until empty, then
    /// return the total number of events delivered.
    pub fn poll_busy(&mut self) -> std::io::Result<usize> {
        let zero = Timespec { tv_sec: 0, tv_nsec: 0 };
        let mut total = 0;
        loop {
            let n = epoll::wait(&self.ep, &mut self.events, Some(&zero))?;
            if n == 0 {
                break;
            }
            total += n;
            for i in 0..n {
                handler_dispatch(&self.events[i]);
            }
        }
        Ok(total)
    }

    /// The events from the most recent poll, converted.
    pub fn delivered(&self, n: usize) -> impl Iterator<Item = EpollEvent> + '_ {
        self.events.iter().take(n).map(EpollEvent::from_raw)
    }

    /// Copy the first `n` delivered events into `out` (converted), so the
    /// caller can process them without holding a borrow on the reactor.
    /// Returns the number copied.
    pub fn copy_events(&self, n: usize, out: &mut [EpollEvent]) -> usize {
        let m = n.min(self.events.len()).min(out.len());
        for (dst, src) in out.iter_mut().zip(self.events.iter()).take(m) {
            *dst = EpollEvent::from_raw(src);
        }
        m
    }

    /// The raw epoll fd (for tests / io_uring handoff).
    pub fn as_raw_fd(&self) -> i32 {
        self.ep.as_raw_fd()
    }
}

#[inline]
fn handler_dispatch(_e: &epoll::Event) {
    // Transport handlers are wired by the application (see
    // examples/bench_udp.rs); the reactor core only delivers events.
}

impl std::fmt::Debug for Reactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reactor")
            .field("ep", &self.ep.as_raw_fd())
            .field("max_events", &self.events.len())
            .finish()
    }
}

/// The readiness source for the epoll worker loop: edge-triggered,
/// drain-to-EAGAIN, busy-poll (the default strategy). The io_uring
/// strategy replaces the whole readiness+drain model with a
/// completion-driven datapath
/// ([`crate::io_uring_reactor::IoUringDatapath`]).
#[cfg(test)]
mod tests {
    use super::*;
    use rustix::io::{read, write};
    use rustix::net::socketpair;

    fn make_pair() -> (std::os::unix::io::OwnedFd, std::os::unix::io::OwnedFd) {
        // AF_UNIX SOCK_STREAM pair, both ends nonblocking.
        let (a, b) = socketpair(
            rustix::net::AddressFamily::UNIX,
            rustix::net::SocketType::STREAM,
            rustix::net::SocketFlags::CLOEXEC,
            None,
        )
        .expect("socketpair");
        rustix::io::ioctl_fionbio(&a, true).expect("nonblock");
        rustix::io::ioctl_fionbio(&b, true).expect("nonblock");
        (a, b)
    }

    #[test]
    fn edge_triggered_delivers_once_then_drain() {
        let mut r = Reactor::new(8).unwrap();
        let (a, b) = make_pair();
        r.register(a.as_raw_fd(), 7, Interest::Readable).unwrap();
        write(&b, b"x").unwrap();

        // Edge fires once.
        let n = r.poll_timeout(Some(&Timespec { tv_sec: 0, tv_nsec: 0 })).unwrap();
        assert_eq!(n, 1);
        let ev = r.delivered(n).next().unwrap();
        assert!(ev.readable);
        assert_eq!(ev.token, 7);

        // Without draining, the edge does NOT re-fire (ET semantics).
        let n2 = r.poll_timeout(Some(&Timespec { tv_sec: 0, tv_nsec: 0 })).unwrap();
        assert_eq!(n2, 0, "edge-triggered: no new edge until drained");

        // Draining to EAGAIN re-arms the edge: a second write fires again.
        let mut buf = [0u8; 8];
        let got = read(&a, &mut buf).unwrap();
        assert_eq!(got, 1);
        assert_eq!(buf[0], b'x');
        write(&b, b"y").unwrap();
        let n3 = r.poll_timeout(Some(&Timespec { tv_sec: 0, tv_nsec: 0 })).unwrap();
        assert_eq!(n3, 1);
        let mut buf2 = [0u8; 8];
        read(&a, &mut buf2).unwrap();
    }

    #[test]
    fn busy_poll_drains_to_empty() {
        let mut r = Reactor::new(8).unwrap();
        let (a, b) = make_pair();
        r.register(a.as_raw_fd(), 1, Interest::Readable).unwrap();
        write(&b, b"abc").unwrap();
        let total = r.poll_busy().unwrap();
        assert!(total >= 1);
        // Drain the fd; subsequent busy polls find nothing.
        let mut buf = [0u8; 8];
        read(&a, &mut buf).unwrap();
        let n = r.poll_timeout(Some(&Timespec { tv_sec: 0, tv_nsec: 0 })).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn unregister_stops_events() {
        let mut r = Reactor::new(8).unwrap();
        let (a, b) = make_pair();
        r.register(a.as_raw_fd(), 5, Interest::Readable).unwrap();
        r.unregister(a.as_raw_fd()).unwrap();
        write(&b, b"z").unwrap();
        let n = r.poll_timeout(Some(&Timespec { tv_sec: 0, tv_nsec: 0 })).unwrap();
        assert_eq!(n, 0);
    }
}
