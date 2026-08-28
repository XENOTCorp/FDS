//! The io_uring reactor + transport datapath (feature `io-uring`, via
//! the `io-uring` crate, tokio-rs): SQPOLL-capable setup, registered
//! buffers (IORING_REGISTER_BUFFERS), and; when
//! `reactor.strategy = io-uring`; the engine's UDP + TCP echo runs
//! entirely through the ring instead of the recvmmsg/sendmmsg/readv/
//! writev syscall datapath.
//!
//! Mechanism: the transport fds are **blocking** (O_NONBLOCK cleared in
//! [`IoUringDatapath::new`]), so an in-flight ring op waits in the
//! kernel and completes when data arrives; the loop is
//! completion-driven, never EAGAIN-spinning. UDP receives are
//! IORING_OP_RECVMSG requests (one per preallocated slot), echoes are
//! IORING_OP_SENDMSG; TCP is IORING_OP_ACCEPT then IORING_OP_READ /
//! IORING_OP_WRITE per connection. A periodic IORING_OP_TIMEOUT wakes
//! the loop so the stop flag and the metrics poll are serviced while
//! idle. The per-slot lifecycle (recv pending -> send pending -> recv)
//! guarantees the kernel never races user space on a buffer.
//!
//! CONTRACT (implementer): use the `io-uring` crate (tokio-rs) against
//! the system io_uring (the 0.7 series is pure-syscall; it does not link
//! liburing). Tests: setup + registration succeeds on this kernel;
//! socketpair read/write roundtrip through the ring; SQPOLL setup skips
//! gracefully (needs CAP_SYS_ADMIN) without failing the test suite; the
//! datapath echoes UDP and TCP over loopback.
#![cfg(feature = "io-uring")]

use crate::conn::{ConnTable, Connection, ConnectionId, CONN_CAP};
use crate::metrics::{Metrics, MetricsServer};
use crate::reactor::Interest;

/// An io_uring reactor instance.
pub struct IoUringReactor {
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
    pub fn new(entries: u32, sq_thread: u32) -> std::io::Result<Self> {
        let mut builder = io_uring::IoUring::builder();
        if sq_thread > 0 {
            builder.setup_sqpoll(sq_thread);
        }
        // Preallocate the pending-token table so submit_*/drain never
        // allocate for the steady state (a fresh TCP connection grows it
        // once, mirroring the accept-path HashMap allocation in the
        // epoll datapath).
        let pending: Vec<u64> = Vec::with_capacity(entries as usize + 64);
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

    /// Push a prepared submission queue entry with `user_data` as its
    /// token and record the token as pending. The entry's referenced
    /// memory (iovecs, buffers, sockaddrs) must stay valid and untouched
    /// until the corresponding completion is drained; the kernel may
    /// read or write it at any time up to that point.
    pub fn push(
        &mut self,
        user_data: u64,
        entry: io_uring::squeue::Entry,
    ) -> std::io::Result<()> {
        // SAFETY: `push` copies the entry into the ring's SQ memory, so
        // the entry itself need not outlive this call; the buffers it
        // references are kept alive by the datapath's lifecycle contract
        // (see the module docs).
        unsafe { self.ring.submission().push(&entry) }
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::WouldBlock, "io_uring submission queue full")
            })?;
        self.pending.push(user_data);
        Ok(())
    }

    /// Register `bufs` with IORING_REGISTER_BUFFERS (returns Err when
    /// unsupported; caller falls back).
    pub fn register_buffers(&mut self, bufs: &mut [&mut [u8]]) -> std::io::Result<()> {
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

    /// Submit a single-shot poll for `fd`'s readiness (`flags` are
    /// `<poll.h>` bits, e.g. `POLLIN`) with `user_data` as the token.
    /// Completes once; the caller re-arms by submitting again.
    pub fn submit_poll(
        &mut self,
        fd: i32,
        flags: u32,
        user_data: u64,
    ) -> std::io::Result<()> {
        let entry = io_uring::opcode::PollAdd::new(io_uring::types::Fd(fd), flags)
            .build()
            .user_data(user_data);
        // SAFETY: as in [`Self::submit_read`]; a poll has no buffer, so
        // the only lifetime is the fd, which outlives the datapath.
        unsafe { self.ring.submission().push(&entry) }
            .map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "io_uring submission queue full",
                )
            })?;
        self.pending.push(user_data);
        Ok(())
    }

    /// Submit everything currently in the submission queue (no wait).
    pub fn submit_all(&mut self) -> std::io::Result<()> {
        self.ring.submit().map(|_| ())
    }

    /// Submit and block until at least one completion arrives (used by
    /// the datapath's event loop; the periodic timeout op guarantees a
    /// completion even when the sockets are idle).
    pub fn submit_and_wait(&mut self, n: u32) -> std::io::Result<()> {
        self.ring.submit_and_wait(n as usize).map(|_| ())
    }

    /// Drain completed entries, calling `f(token, result)`. Returns the
    /// number of completions.
    pub fn drain<F: FnMut(u64, std::io::Result<u32>)>(&mut self, mut f: F) -> usize {
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

/// Map [`Interest`] onto `<poll.h>` bits; ERR/HUP are always requested so
/// a closed/errored fd still surfaces (epoll reports them implicitly).
fn poll_flags(interest: Interest) -> u32 {
    let mut f = libc::POLLERR as u32 | libc::POLLHUP as u32;
    match interest {
        Interest::Readable => f |= libc::POLLIN as u32,
        Interest::Writable => f |= libc::POLLOUT as u32,
        Interest::ReadableWritable => f |= libc::POLLIN as u32 | libc::POLLOUT as u32,
    }
    f
}

// ---------------------------------------------------------------------
// The completion-driven datapath
// ---------------------------------------------------------------------

/// user_data layout: the high nibble is the op class, the low bits the
/// object (UDP slot index, or a TCP [`ConnectionId`] token; the engine
/// keeps worker ids < 2^28, so a token's top nibble is always zero and
/// the two never collide; asserted in `new`).
const KIND_MASK: u64 = 0xF000_0000_0000_0000;
const KIND_UDP_RECV: u64 = 0x1000_0000_0000_0000;
const KIND_UDP_SEND: u64 = 0x2000_0000_0000_0000;
const KIND_ACCEPT: u64 = 0x3000_0000_0000_0000;
const KIND_TCP_READ: u64 = 0x4000_0000_0000_0000;
const KIND_TCP_WRITE: u64 = 0x8000_0000_0000_0000;
const KIND_POLL: u64 = 0xC000_0000_0000_0000;
const KIND_TIMEOUT: u64 = 0xD000_0000_0000_0000;

/// In-flight UDP receive slots (recv or send pending per slot).
const UDP_SLOTS: usize = 64;
/// Per-TCP-connection echo buffer.
const CONN_BUF: usize = 8192;
/// Periodic wakeup (ms) so the stop flag and metrics poll are serviced
/// while the sockets are idle.
const TIMEOUT_MS: u64 = 100;

/// Per-UDP-slot lifecycle: exactly one ring op references the slot at a
/// time, so the kernel and user space never touch the buffer together.
#[derive(Clone, Copy, PartialEq)]
enum UdpPhase {
    Idle,
    RecvPending,
    SendPending,
}

/// One UDP receive slot: buffer + source-address storage + the msghdr
/// the ring ops point at.
struct UdpSlot {
    buf: Box<[u8; crate::udp::MAX_DATAGRAM]>,
    iov: libc::iovec,
    msg: libc::msghdr,
    name: Box<libc::sockaddr_storage>,
    phase: UdpPhase,
}

/// One accepted TCP connection: fd + echo buffer + the op in flight.
struct TcpRingConn {
    fd: i32,
    buf: Box<[u8; CONN_BUF]>,
}

/// The completion-driven UDP + TCP echo datapath.
pub struct IoUringDatapath {
    ring: IoUringReactor,
    core: usize,
    udp_fd: i32,
    listen_fd: i32,
    slots: Box<[UdpSlot]>,
    /// Accept-scratch: the kernel writes the peer address here.
    accept_addr: Box<libc::sockaddr_storage>,
    accept_len: libc::socklen_t,
    accept_pending: bool,
    /// token -> connection (accepted fds are owned by the datapath and
    /// closed on drop).
    conns: std::collections::HashMap<u64, TcpRingConn>,
    /// Hot/cold connection state + slot allocation (per the framework).
    conn_table: ConnTable<CONN_CAP>,
    metrics_fd: Option<i32>,
    poll_pending: bool,
    /// Stable address for the periodic timeout op (the kernel may read
    /// it while the op is pending).
    timeout: io_uring::types::Timespec,
    timeout_pending: bool,
}

impl IoUringDatapath {
    /// Build the datapath for one worker. `udp_fd`/`listen_fd` are
    /// borrowed (owned by the engine's sockets); both are switched to
    /// blocking so ring ops wait in-kernel instead of EAGAINing.
    pub fn new(
        core: usize,
        udp_fd: i32,
        listen_fd: i32,
        metrics_fd: Option<i32>,
        entries: u32,
        sq_thread: u32,
    ) -> std::io::Result<Self> {
        assert!(
            core < (1 << 28),
            "io_uring datapath: worker id must fit below the KIND_MASK nibble"
        );
        // The initial submission is UDP_SLOTS recvs + accept + timeout +
        // poll; never let a misconfigured ring undercut that.
        let entries = entries.max((UDP_SLOTS + 8) as u32);
        clear_nonblock(udp_fd)?;
        clear_nonblock(listen_fd)?;

        let conn_table: ConnTable<CONN_CAP> = ConnTable::new();        for i in 0..CONN_CAP {
            conn_table.initialize(i, Connection::new("0.0.0.0:0".parse().unwrap(), 0));
        }

        // SAFETY: zeroed msghdr/iovec/sockaddr_storage have no invalid
        // bit patterns; every field the kernel sees is rewritten before
        // each submission.
        let slots: Box<[UdpSlot]> = (0..UDP_SLOTS)
            .map(|_| UdpSlot {
                buf: Box::new([0u8; crate::udp::MAX_DATAGRAM]),
                iov: unsafe { std::mem::zeroed() },
                msg: unsafe { std::mem::zeroed() },
                name: Box::new(unsafe { std::mem::zeroed() }),
                phase: UdpPhase::Idle,
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();

        Ok(IoUringDatapath {
            ring: IoUringReactor::new(entries, sq_thread)?,
            core,
            udp_fd,
            listen_fd,
            slots,
            accept_addr: Box::new(unsafe { std::mem::zeroed() }),
            accept_len: std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t,
            accept_pending: false,
            conns: std::collections::HashMap::with_capacity(CONN_CAP + 64),
            conn_table,
            metrics_fd,
            poll_pending: false,
            timeout: io_uring::types::Timespec::from(std::time::Duration::from_millis(TIMEOUT_MS)),
            timeout_pending: false,
        })
    }

    /// Run the completion-driven loop until `stop`. When `busy_poll` is
    /// set (the engine's default latency contract), the loop submits and
    /// syncs the CQ without blocking; completion pickup is immediate at
    /// the cost of CPU while idle, mirroring the epoll worker. Without
    /// it, `submit_and_wait` blocks on the next completion (event-driven,
    /// idle-friendly, but adds wakeup latency). All buffers and the ring
    /// live for the datapath's lifetime; nothing allocates per event.
    pub fn run(
        &mut self,
        stop: &(dyn Fn() -> bool + Send + Sync),
        metrics: &Metrics,
        core: usize,
        metrics_server: &mut Option<MetricsServer>,
        busy_poll: bool,
    ) -> std::io::Result<()> {
        for slot in 0..self.slots.len() {
            self.submit_udp_recv(slot)?;
        }
        self.submit_accept()?;
        self.submit_timeout()?;
        if self.metrics_fd.is_some() {
            self.submit_poll()?;
        }

        // Preallocated completion buffer, reused every iteration.
        let mut completions: Vec<(u64, std::io::Result<u32>)> = Vec::with_capacity(64);
        while !stop() {
            if busy_poll {
                // Non-blocking submit + CQ sync; the loop spins while
                // idle (same CPU contract as the epoll busy-poll).
                self.ring.submit_all()?;
            } else {
                self.ring.submit_and_wait(1)?;
            }
            completions.clear();
            self.ring.drain(|ud, res| completions.push((ud, res)));
            for (ud, res) in completions.drain(..) {
                self.dispatch(ud, res, metrics, core, metrics_server)?;
            }
        }
        Ok(())
    }

    /// One completion: advance the slot/connection lifecycle, update the
    /// per-core counters, and re-submit the next op for the object.
    fn dispatch(
        &mut self,
        user_data: u64,
        res: std::io::Result<u32>,
        metrics: &Metrics,
        core: usize,
        metrics_server: &mut Option<MetricsServer>,
    ) -> std::io::Result<()> {
        match user_data & KIND_MASK {
            KIND_UDP_RECV => {
                let slot = (user_data & !KIND_MASK) as usize;
                match res {
                    Ok(n) => {
                        let n = n as usize;
                        metrics.add_packets(core, 1);
                        metrics.add_bytes(core, n as u64);
                        self.submit_udp_send(slot, n)?;
                    }
                    Err(_) => {
                        metrics.add_drops(core, 1);
                        self.submit_udp_recv(slot)?;
                    }
                }
            }
            KIND_UDP_SEND => {
                let slot = (user_data & !KIND_MASK) as usize;
                // Buffer + source address are free again; re-arm the recv.
                self.submit_udp_recv(slot)?;
            }
            KIND_ACCEPT => {
                if let Ok(fd) = res {
                    let fd = fd as i32;
                    let peer = addr_from_storage(&self.accept_addr);
                    // Index-based acquire (no guard): the slot stays
                    // owned until close_tcp releases it exactly once.
                    // A guard would release at accept (its Drop) and
                    // double-release at close.
                    match self.conn_table.acquire_index() {
                        Some(idx) => {
                            // Cold state: peer on setup (the hot/cold
                            // split in action).
                            self.conn_table.conn_mut(idx).cold.peer = peer;
                            let token = ConnectionId::new(self.core as u32, idx as u32).as_u64();
                            self.conns.insert(
                                token,
                                TcpRingConn {
                                    fd,
                                    buf: Box::new([0u8; CONN_BUF]),
                                },
                            );
                            self.submit_tcp_read(token)?;
                        }
                        None => {
                            metrics.add_drops(core, 1);
                            // SAFETY: closing a freshly accepted fd
                            // this datapath owns.
                            unsafe {
                                libc::close(fd);
                            }
                        }
                    }
                }
                self.submit_accept()?;
            }
            KIND_TCP_READ => {
                let token = user_data & !KIND_MASK;
                match res {
                    Ok(n) if n > 0 => {
                        let n = n as usize;
                        metrics.add_packets(core, 1);
                        metrics.add_bytes(core, n as u64);
                        let slot = ConnectionId::from_u64(token).slot() as usize;
                        let hot = &mut self.conn_table.conn_mut(slot).hot;
                        hot.seq = hot.seq.wrapping_add(n as u32);
                        hot.last_activity = crate::util::now_ticks();
                        self.submit_tcp_write(token, n)?;
                    }
                    Ok(_) => {
                        // Clean EOF.
                        self.close_tcp(token);
                    }
                    Err(_) => {
                        metrics.add_drops(core, 1);
                        self.close_tcp(token);
                    }
                }
            }
            KIND_TCP_WRITE => {
                let token = user_data & !KIND_MASK;
                match res {
                    Ok(_) => self.submit_tcp_read(token)?,
                    Err(_) => {
                        metrics.add_drops(core, 1);
                        self.close_tcp(token);
                    }
                }
            }
            KIND_POLL => {
                // Metrics client pending: serve until none remain, then
                // re-arm the single-shot poll.
                if let Some(s) = metrics_server {
                    while let Ok(true) = s.poll_once(metrics) {}
                }
                self.submit_poll()?;
            }
            KIND_TIMEOUT => {
                // Periodic wakeup (fires with -ETIME when idle). Re-arm.
                self.submit_timeout()?;
            }
            _ => {
                // Unknown token: nothing to advance.
            }
        }
        Ok(())
    }

    /// Submit a UDP recv into `slot` (single-shot; waits in-kernel).
    fn submit_udp_recv(&mut self, slot: usize) -> std::io::Result<()> {
        let s = &mut self.slots[slot];
        s.phase = UdpPhase::RecvPending;
        s.iov = libc::iovec {
            // SAFETY: the slot's boxed buffer is owned by this datapath
            // and untouched between this submission and its completion.
            iov_base: s.buf.as_mut_ptr().cast(),
            iov_len: s.buf.len(),
        };
        s.msg = libc::msghdr {
            // SAFETY: the boxed sockaddr_storage is owned by this
            // datapath and untouched until the completion.
            msg_name: (&mut *s.name as *mut libc::sockaddr_storage).cast(),
            msg_namelen: std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t,
            msg_iov: &mut s.iov,
            msg_iovlen: 1,
            msg_control: std::ptr::null_mut(),
            msg_controllen: 0,
            msg_flags: 0,
        };
        // SAFETY: the msghdr, iovec, buffer and name live in the slot
        // for the datapath's lifetime; the lifecycle guarantees nothing
        // else touches them until the completion is dispatched.
        let user_data = KIND_UDP_RECV | slot as u64;
        let entry = io_uring::opcode::RecvMsg::new(
            io_uring::types::Fd(self.udp_fd),
            &mut s.msg as *mut libc::msghdr,
        )
        .build()
        .user_data(user_data);
        self.ring.push(user_data, entry)
    }

    /// Echo the datagram received into `slot` (n bytes) back to its
    /// source: the msghdr still points at the filled source address.
    fn submit_udp_send(&mut self, slot: usize, n: usize) -> std::io::Result<()> {
        let s = &mut self.slots[slot];
        s.phase = UdpPhase::SendPending;
        s.iov = libc::iovec {
            // SAFETY: the buffer holds `n` received bytes; the kernel
            // only reads them for the duration of the send op.
            iov_base: s.buf.as_ptr().cast_mut().cast(),
            iov_len: n,
        };
        // SAFETY: `s.msg` is fully initialized (recv filled the name and
        // namelen; we updated the iov above) and the kernel only reads
        // it while the op is pending.
        let user_data = KIND_UDP_SEND | slot as u64;
        let entry = io_uring::opcode::SendMsg::new(
            io_uring::types::Fd(self.udp_fd),
            &s.msg as *const libc::msghdr,
        )
        .build()
        .user_data(user_data);
        self.ring.push(user_data, entry)
    }

    /// Submit an accept on the listener (waits in-kernel for a
    /// connection; the accepted fd is blocking + CLOEXEC so ring reads
    /// on it also wait).
    fn submit_accept(&mut self) -> std::io::Result<()> {
        self.accept_pending = true;
        self.accept_len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        // SAFETY: the accept scratch lives for the datapath's lifetime
        // and is untouched until the completion is dispatched.
        let entry = io_uring::opcode::Accept::new(
            io_uring::types::Fd(self.listen_fd),
            (&mut *self.accept_addr as *mut libc::sockaddr_storage).cast::<libc::sockaddr>(),
            &mut self.accept_len as *mut libc::socklen_t,
        )
        .flags(libc::SOCK_CLOEXEC)
        .build()
        .user_data(KIND_ACCEPT);
        self.ring.push(KIND_ACCEPT, entry)
    }

    /// Submit a read for `token`'s connection (waits in-kernel for data).
    fn submit_tcp_read(&mut self, token: u64) -> std::io::Result<()> {
        let c = match self.conns.get_mut(&token) {
            Some(c) => c,
            None => return Ok(()), // already closed
        };
        // SAFETY: the connection's boxed buffer is owned by the datapath
        // and untouched between this submission and its completion.
        let user_data = KIND_TCP_READ | token;
        let entry = io_uring::opcode::Read::new(
            io_uring::types::Fd(c.fd),
            c.buf.as_mut_ptr(),
            c.buf.len() as u32,
        )
        .build()
        .user_data(user_data);
        self.ring.push(user_data, entry)
    }

    /// Echo `n` received bytes back to `token`'s connection.
    fn submit_tcp_write(&mut self, token: u64, n: usize) -> std::io::Result<()> {
        let c = match self.conns.get_mut(&token) {
            Some(c) => c,
            None => return Ok(()), // already closed
        };
        // SAFETY: the buffer holds `n` received bytes; the kernel only
        // reads them for the duration of the write op.
        let user_data = KIND_TCP_WRITE | token;
        let entry = io_uring::opcode::Write::new(
            io_uring::types::Fd(c.fd),
            c.buf.as_ptr(),
            n as u32,
        )
        .build()
        .user_data(user_data);
        self.ring.push(user_data, entry)
    }

    /// Close `token`'s connection: drop the ring state, close the fd
    /// (owned by the datapath), release the ConnTable slot.
    fn close_tcp(&mut self, token: u64) {
        if let Some(c) = self.conns.remove(&token) {
            let slot = ConnectionId::from_u64(token).slot() as usize;
            // SAFETY: the fd was accepted by this datapath and is closed
            // exactly once, here.
            unsafe {
                libc::close(c.fd);
            }
            // Exactly one release: the slot was acquired index-only at
            // accept, so no guard will release it again.
            self.conn_table.release_slot(slot);
        }
    }

    /// Submit the single-shot poll for the metrics listener.
    fn submit_poll(&mut self) -> std::io::Result<()> {
        self.poll_pending = true;
        match self.metrics_fd {
            Some(fd) => self.ring.submit_poll(fd, poll_flags(Interest::Readable), KIND_POLL),
            None => Ok(()),
        }
    }

    /// Submit the periodic timeout (keeps the loop waking while idle).
    fn submit_timeout(&mut self) -> std::io::Result<()> {
        self.timeout_pending = true;
        // SAFETY: `self.timeout` has a stable address for the datapath's
        // lifetime; the kernel reads it while the op is pending.
        let entry = io_uring::opcode::Timeout::new(&self.timeout as *const io_uring::types::Timespec)
            // count=1: fire when the timespec elapses OR one completion
            // posts, whichever first; count=0 would be an indefinite
            // timeout that never wakes an idle loop.
            .count(1)
            .build()
            .user_data(KIND_TIMEOUT);
        self.ring.push(KIND_TIMEOUT, entry)
    }
}

impl Drop for IoUringDatapath {
    fn drop(&mut self) {
        // Close the accepted connection fds this datapath owns (the UDP
        // socket and listener are borrowed from the engine).
        for (_, c) in self.conns.drain() {
            // SAFETY: accepted fds owned by this datapath, closed once.
            unsafe {
                libc::close(c.fd);
            }
        }
    }
}

/// Clear O_NONBLOCK on `fd` (ring ops on a blocking socket wait in the
/// kernel instead of completing with EAGAIN).
fn clear_nonblock(fd: i32) -> std::io::Result<()> {
    // SAFETY: `fd` is a live descriptor owned by the caller; F_GETFL
    // reads the current flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: F_SETFL with the nonblock bit cleared on a live fd.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Read a `sockaddr_storage` filled by the kernel on an AF_INET socket
/// as a std address (IPv4 only, matching the engine).
fn addr_from_storage(ss: &libc::sockaddr_storage) -> std::net::SocketAddr {
    use std::net::{Ipv4Addr, SocketAddrV4};
    assert_eq!(ss.ss_family as libc::c_int, libc::AF_INET);
    // SAFETY: AF_INET guarantees the kernel wrote a `sockaddr_in` at
    // this address; both structs start with the family field.
    let sin = unsafe { &*(ss as *const libc::sockaddr_storage).cast::<libc::sockaddr_in>() };
    // The kernel stores address and port in network byte order.
    std::net::SocketAddr::V4(SocketAddrV4::new(
        Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)),
        u16::from_be(sin.sin_port),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::net::{socketpair, AddressFamily, SocketFlags, SocketType};
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

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
        // The read op is pushed directly (the submit_read helper was
        // pruned as dead code); this still exercises the ring read path.
        let entry =
            io_uring::opcode::Read::new(io_uring::types::Fd(r.as_raw_fd()), buf.as_mut_ptr(), buf.len() as u32)
                .build()
                .user_data(1);
        // SAFETY: push copies the entry into the SQ; `buf` stays alive
        // until the completion is drained below.
        unsafe { reactor.ring.submission().push(&entry) }
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::WouldBlock, "sq full"))?;

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
        // success or a graceful Err is acceptable; it must not panic.
        let _ = reactor.register_buffers(&mut bufs);
    }

    /// Full datapath smoke over loopback: UDP echo and TCP echo through
    /// the ring, then a clean stop. The datapath runs on this thread (it
    /// is single-owner, not `Send`); a client thread drives the I/O and
    /// flips the stop flag.
    #[test]
    fn datapath_udp_tcp_loopback_echo() {
        let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let udp_addr = udp.local_addr().unwrap();
        let tcp_addr = tcp.local_addr().unwrap();

        let mut datapath =
            IoUringDatapath::new(0, udp.as_raw_fd(), tcp.as_raw_fd(), None, 128, 0).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let metrics = Metrics::new(1);

        let client = std::thread::spawn(move || {
            // UDP echo through the ring.
            let client = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            client.send_to(b"ring udp echo", udp_addr).unwrap();
            let mut buf = [0u8; 64];
            let (n, _) = client.recv_from(&mut buf).unwrap();
            assert_eq!(&buf[..n], b"ring udp echo");

            // TCP echo through the ring (accept -> read -> write).
            let mut stream = std::net::TcpStream::connect(tcp_addr).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream.write_all(b"ring tcp echo").unwrap();
            let mut tbuf = [0u8; 64];
            let n = stream.read(&mut tbuf).unwrap();
            assert_eq!(&tbuf[..n], b"ring tcp echo");

            // Jumbo UDP through the ring (64 KiB recv buffer, no truncation).
            client.send_to(&[0xABu8; 60_000], udp_addr).unwrap();
            let mut jumbo = vec![0u8; 70_000];
            let (n, _) = client.recv_from(&mut jumbo).unwrap();
            assert_eq!(n, 60_000);
            assert!(jumbo[..n].iter().all(|&b| b == 0xAB));

            stop2.store(true, Ordering::Relaxed);
        });

        datapath
            .run(&(move || stop.load(Ordering::Relaxed)), &metrics, 0, &mut None, true)
            .expect("datapath run failed");
        client.join().unwrap();
    }
}
