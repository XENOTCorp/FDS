//! The io_uring reactor + transport datapath (feature `io-uring`, via
//! the `io-uring` crate, tokio-rs): SQPOLL-capable setup, registered
//! buffers (IORING_REGISTER_BUFFERS), provided buffers
//! (IORING_OP_PROVIDE_BUFFERS), multishot recv and accept, and
//! IORING_OP_SEND_ZC with a completion/recycle protocol.
//!
//! Two TCP modes, chosen at runtime:
//!
//! - **Modern** (kernel >= 6.0): the fds stay nonblocking. Accept runs
//!   as IORING_OP_ACCEPT with the multishot flag. Receive runs as
//!   multishot recvmsg against a per-connection provided-buffer group;
//!   the payload offset is read from the `io_uring_recvmsg_out` header
//!   the kernel prepends. Echo runs as IORING_OP_SEND against the
//!   registered buffer pool (one CQE per send; SEND_ZC's extra
//!   notification does not pay on loopback). The buffer id is encoded
//!   in the SQE `user_data`. Flow control is a per-connection watermark: when the
//!   outstanding echo bytes reach the high watermark the multishot
//!   recv is cancelled (backpressure stops reads); at the low
//!   watermark buffers are re-provided and the recv re-armed. Buffering
//!   is bounded by the pool size (POOL_TOTAL x CONN_BUF = 4 MiB).
//!   Submission is batched: re-submits from one completion batch are
//!   flushed in one `io_uring_enter`; with SQPOLL the kernel thread
//!   drains the SQ without an enter at all.
//! - **Legacy** (kernel < 6.0, or when registration/multishot is
//!   rejected at runtime): the previous single-shot accept/read/write
//!   datapath on blocking fds.
//!
//! CONTRACT (implementer): the `io-uring` crate (tokio-rs) against the
//! system io_uring (the 0.7 series is pure-syscall; it does not link
//! liburing). Tests: setup + registration succeeds on this kernel;
//! socketpair read/write roundtrip through the ring; SQPOLL setup skips
//! gracefully (needs CAP_SYS_ADMIN) without failing the test suite; the
//! datapath echoes UDP and TCP over loopback; a TCP write flood echoes
//! to completion (no stall) with bounded buffering.
#![cfg(feature = "io-uring")]

use crate::conn::{ConnTable, Connection, ConnectionId, CONN_CAP};
use crate::metrics::{Metrics, MetricsServer};
use crate::reactor::Interest;
use io_uring::squeue::Flags;
use std::collections::VecDeque;

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
        // allocate for the steady state.
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
        // SAFETY: a poll has no buffer, so the only lifetime is the fd,
        // which outlives the datapath.
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

    /// Number of SQEs queued and not yet submitted. The datapath uses
    /// this to flush the submission batch before the SQ overflows, so a
    /// completion batch's re-submits reach the kernel in one enter.
    pub fn queued(&mut self) -> usize {
        self.ring.submission().len()
    }

    /// Cancel the in-flight op with `user_data`
    /// (IORING_OP_ASYNC_CANCEL). Best-effort: the op may complete
    /// normally before the cancel is processed; the cancelled
    /// completion arrives with `-ECANCELED`.
    pub fn ring_cancel(&mut self, user_data: u64) -> std::io::Result<()> {
        let entry = io_uring::opcode::AsyncCancel::new(user_data)
            .build()
            .user_data(0);
        // SAFETY: push copies the entry into the ring's SQ memory; the
        // cancel carries no buffer pointer.
        unsafe { self.ring.submission().push(&entry) }
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::WouldBlock, "io_uring submission queue full")
            })?;
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
        self.drain_full(|ud, res, _flags| f(ud, res))
    }

    /// Drain completed entries with their CQE flags
    /// (`IORING_CQE_F_MORE`/`F_BUFFER`/`F_NOTIF`), calling
    /// `f(token, result, flags)`.
    pub fn drain_full<F: FnMut(u64, std::io::Result<u32>, u32)>(&mut self, mut f: F) -> usize {
        let mut cq = self.ring.completion();
        cq.sync();
        let mut n = 0;
        for cqe in cq {
            let user_data = cqe.user_data();
            f(user_data, result(cqe.result()), cqe.flags());
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
/// a closed/errored fd still surfaces.
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
// CQE flags (ABI constants from <linux/io_uring.h>)
// ---------------------------------------------------------------------

const CQE_F_BUFFER: u32 = 1;
const CQE_F_MORE: u32 = 2;
const CQE_F_NOTIF: u32 = 8;
const CQE_BUFFER_SHIFT: u32 = 16;

/// The `io_uring_recvmsg_out` header the kernel prepends to every
/// multishot-recvmsg buffer (4 x u32; ABI from <linux/io_uring.h>).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RecvMsgOut {
    namelen: u32,
    controllen: u32,
    payloadlen: u32,
    flags: u32,
}

/// Kernel major/minor from `uname(2)`; `(0, 0)` when unreadable.
fn kernel_version() -> (u32, u32) {
    let mut uts: libc::utsname = unsafe { std::mem::zeroed() };
    // SAFETY: `uts` is a writable utsname buffer of the kernel ABI size.
    if unsafe { libc::uname(&mut uts) } != 0 {
        return (0, 0);
    }
    // SAFETY: the kernel NUL-terminates `release`.
    let release = unsafe { std::ffi::CStr::from_ptr(uts.release.as_ptr()) }.to_string_lossy();
    let mut parts = release.split('.');
    let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor)
}

/// The multishot recvmsg family (recvmsg-multishot, accept-multishot,
/// send_zc, provided buffers, registered buffers) is available from
/// kernel 6.0. Below that the datapath runs in legacy mode.
fn modern_capable() -> bool {
    let (major, minor) = kernel_version();
    (major, minor) >= (6, 0)
}

// ---------------------------------------------------------------------
// The completion-driven datapath
// ---------------------------------------------------------------------

/// user_data layout: the high nibble is the op class, the low bits the
/// object. TCP send user_data encodes the buffer id:
/// `KIND_TCP_SEND | (buf_id << 32) | slot`.
const KIND_MASK: u64 = 0xF000_0000_0000_0000;
const KIND_UDP_RECV: u64 = 0x1000_0000_0000_0000;
const KIND_UDP_SEND: u64 = 0x2000_0000_0000_0000;
const KIND_ACCEPT: u64 = 0x3000_0000_0000_0000;
const KIND_TCP_READ: u64 = 0x4000_0000_0000_0000;
const KIND_TCP_SEND: u64 = 0x5000_0000_0000_0000;
const KIND_TCP_POLLOUT: u64 = 0x6000_0000_0000_0000;
const KIND_TCP_CLOSE: u64 = 0x7000_0000_0000_0000;
const KIND_PROVIDE: u64 = 0x8000_0000_0000_0000;
const KIND_POLL: u64 = 0xC000_0000_0000_0000;
const KIND_TIMEOUT: u64 = 0xD000_0000_0000_0000;

/// In-flight UDP recv/send slots. Matches engine `udp_rx_slots` (D-4):
/// 4 × 64 KiB stays in this CPU's L2. A 64-slot set misses L3 and
/// completes empty RecvMsg with -EAGAIN on a nonblocking socket.
const UDP_SLOTS: usize = 4;
/// Periodic wakeup (ms) so the stop flag and metrics poll are serviced
/// while the sockets are idle.
const TIMEOUT_MS: u64 = 100;

// ---- modern-mode buffer pool ----

/// Total registered buffers (512 x 8 KiB = 4 MiB; the hard bound on
/// in-flight echo data across all connections).
const POOL_TOTAL: usize = 512;
/// Per-buffer capacity (payload budget: CONN_BUF - recvmsg_out header).
const CONN_BUF: usize = 8192;
/// Buffers provided each time a connection arms its receive (the
/// multishot recv consumes these before the group runs dry and the op
/// terminates; a larger batch amortizes the re-arm cycle).
const INIT_PROVIDE: usize = 64;
/// Per-connection high watermark in buffers: above this, reads stop.
const HWM_BUFS: usize = 160;
/// Per-connection low watermark in buffers: below this, reads resume.
const LWM_BUFS: usize = 64;
/// Flush the submission batch once this many SQEs are queued.
const FLUSH_AT: usize = 64;

/// Per-buffer ownership state (a buffer is in exactly one state).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BufState {
    /// In the user-space free list.
    Free,
    /// In a connection's provided-buffer group (kernel-owned).
    InGroup,
    /// Received data awaiting a send (user-space owned by the conn).
    Owned,
    /// Sent via SEND_ZC, awaiting the notification CQEs.
    InKernel,
}

/// Per-buffer payload metadata: where in the buffer the payload starts
/// and its length (set at recv, consumed at send; a partial send
/// advances the offset).
#[derive(Clone, Copy, Default)]
struct BufMeta {
    off: u16,
    len: u16,
}

/// One accepted TCP connection.
struct TcpRingConn {
    fd: i32,
    /// The msghdr the multishot recvmsg op references (must outlive the
    /// op). For a stream socket only msg_namelen/msg_controllen matter.
    recv_msg: Box<libc::msghdr>,
    /// Legacy-mode read buffer (single-shot path; unused in modern).
    legacy_buf: Box<[u8; CONN_BUF]>,
    /// Buffer ids in this connection's provided group, in provide order
    /// (the kernel removes from the group head, FIFO).
    provided: VecDeque<u32>,
    /// Recv'd data awaiting a send submission.
    to_send: VecDeque<u32>,
    /// Sends submitted, awaiting the notification CQE.
    in_kernel: Vec<u32>,
    /// to_send.len() + in_kernel.len() (the flow-control measure).
    outstanding: usize,
    /// True while a multishot recv op is armed.
    recv_armed: bool,
    /// True while a POLLOUT poll is armed.
    poll_out: bool,
    /// True while a REMOVE_BUFFERS drain is in flight (close path).
    draining: bool,
    /// Close in progress: the fd is closed; the group is drained before
    /// the conn is removed.
    closing: bool,
}

impl TcpRingConn {
    fn new(fd: i32) -> Self {
        TcpRingConn {
            fd,
            recv_msg: Box::new(tcp_recv_msg()),
            legacy_buf: Box::new([0u8; CONN_BUF]),
            provided: VecDeque::new(),
            to_send: VecDeque::new(),
            in_kernel: Vec::new(),
            outstanding: 0,
            recv_armed: false,
            poll_out: false,
            draining: false,
            closing: false,
        }
    }
}

/// The modern-mode buffer pool: registered once as fixed buffers and
/// provided to per-connection groups.
struct BufferPool {
    pool: Box<[Box<[u8; CONN_BUF]>]>,
    states: Vec<BufState>,
    metas: Vec<BufMeta>,
    /// Per-buffer count of in-flight SEND_ZC ops. A buffer is recycled
    /// only when every op referencing it has posted its notification
    /// (notifications carry no byte range, so the count is the recycle
    /// signal). Partial sends re-submit the tail with a new op, which
    /// bumps the count again.
    inflight: Vec<u32>,
    /// User-space free list.
    free: Vec<u32>,
}

impl BufferPool {
    fn new() -> BufferPool {
        let pool = (0..POOL_TOTAL)
            .map(|_| Box::new([0u8; CONN_BUF]))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        BufferPool {
            pool,
            states: vec![BufState::Free; POOL_TOTAL],
            metas: vec![BufMeta::default(); POOL_TOTAL],
            inflight: vec![0; POOL_TOTAL],
            free: (0..POOL_TOTAL as u32).rev().collect(),
        }
    }

    fn take_free(&mut self) -> Option<u32> {
        let id = self.free.pop()?;
        self.states[id as usize] = BufState::InGroup;
        Some(id)
    }

    fn give_back(&mut self, id: u32) {
        debug_assert_ne!(self.states[id as usize], BufState::Free);
        self.states[id as usize] = BufState::Free;
        self.free.push(id);
    }

    fn mark_owned(&mut self, id: u32, off: u16, len: u16) {
        self.states[id as usize] = BufState::Owned;
        self.metas[id as usize] = BufMeta { off, len };
    }

    /// Register a SEND_ZC submission referencing `id` (the kernel now
    /// owns the pages until its notification).
    fn submit_zc(&mut self, id: u32) {
        self.states[id as usize] = BufState::InKernel;
        self.inflight[id as usize] += 1;
    }

    /// A SEND_ZC notification for `id` arrived. Recycles the buffer and
    /// returns `true` when this was the last in-flight op. Works for
    /// buffers in `Owned` state too (a partial-send tail still queued
    /// when its connection closed): the count is the only thing that
    /// matters. Spurious notifications (buffer already recycled or
    /// re-provided) are ignored.
    fn notify_zc(&mut self, id: u32) -> bool {
        if self.inflight[id as usize] > 0 {
            self.inflight[id as usize] -= 1;
        }
        if self.inflight[id as usize] == 0
            && matches!(
                self.states[id as usize],
                BufState::Owned | BufState::InKernel
            )
        {
            self.give_back(id);
            return true;
        }
        false
    }

    /// A SEND_ZC op for `id` failed without referencing the pages
    /// (EAGAIN): drop its notification expectation and re-queue.
    fn cancel_zc(&mut self, id: u32) {
        if self.inflight[id as usize] > 0 {
            self.inflight[id as usize] -= 1;
        }
    }

    /// Whether the buffer is free to return to the pool (no kernel
    /// reference outstanding).
    fn zc_settled(&self, id: u32) -> bool {
        self.inflight[id as usize] == 0
    }

    fn ptr(&self, id: u32) -> *const u8 {
        self.pool[id as usize].as_ptr()
    }

    fn ptr_mut(&mut self, id: u32) -> *mut u8 {
        self.pool[id as usize].as_mut_ptr()
    }
}

/// The completion-driven UDP + TCP echo datapath.
pub struct IoUringDatapath {
    ring: IoUringReactor,
    core: usize,
    udp_fd: i32,
    listen_fd: i32,
    slots: Box<[UdpSlot]>,
    /// Accept-scratch (legacy single-shot accept; multishot accept has
    /// no address output, the peer is read with getpeername).
    accept_addr: Box<libc::sockaddr_storage>,
    accept_len: libc::socklen_t,
    /// token -> connection (accepted fds are owned by the datapath and
    /// closed on drop).
    conns: std::collections::HashMap<u64, TcpRingConn>,
    /// Hot/cold connection state + slot allocation (per the framework).
    conn_table: ConnTable<CONN_CAP>,
    metrics_fd: Option<i32>,
    /// Stable address for the periodic timeout op.
    timeout: io_uring::types::Timespec,
    /// Modern (multishot + provided/registered buffers + SEND_ZC) or
    /// legacy (single-shot) datapath.
    legacy: bool,
    pool: BufferPool,
    /// True while the multishot accept is armed (its CQEs carry MORE
    /// until the listener closes).
    accept_multi: bool,
}

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

impl IoUringDatapath {
    /// Build the datapath for one worker. `udp_fd`/`listen_fd` are
    /// borrowed (owned by the engine's sockets). On kernels >= 6.0 the
    /// modern path runs (multishot, provided/registered buffers,
    /// SEND_ZC); otherwise (or when registration fails) the legacy
    /// single-shot path runs.
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
        let entries = entries.max((UDP_SLOTS + 8) as u32);

        let mut datapath = IoUringDatapath {
            ring: IoUringReactor::new(entries, sq_thread)?,
            core,
            udp_fd,
            listen_fd,
            slots: Box::new([]),
            accept_addr: Box::new(unsafe { std::mem::zeroed() }),
            accept_len: std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t,
            conns: std::collections::HashMap::with_capacity(CONN_CAP + 64),
            conn_table: ConnTable::new(),
            metrics_fd,
            timeout: io_uring::types::Timespec::from(std::time::Duration::from_millis(TIMEOUT_MS)),
            legacy: true,
            pool: BufferPool::new(),
            accept_multi: false,
        };
        for i in 0..CONN_CAP {
            datapath
                .conn_table
                .initialize(i, Connection::new("0.0.0.0:0".parse().unwrap(), 0));
        }

        // Modern path needs: kernel >= 6.0, registered buffers, and the
        // provided-buffer machinery. Fall back to legacy when any piece
        // is unavailable so the datapath never fails to start.
        if modern_capable() {
            let mut bufs: Vec<&mut [u8]> = datapath
                .pool
                .pool
                .iter_mut()
                .map(|b| b.as_mut_slice())
                .collect();
            if datapath.ring.register_buffers(&mut bufs).is_ok() {
                datapath.legacy = false;
            }
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
        datapath.slots = slots;
        Ok(datapath)
    }

    /// Run the completion-driven loop until `stop`. When `busy_poll` is
    /// set, the loop submits and syncs the CQ without blocking;
    /// otherwise `submit_and_wait` blocks on the next completion. All
    /// buffers and the ring live for the datapath's lifetime; nothing
    /// allocates per event. Re-submits accumulate in the SQ and are
    /// flushed once per loop pass (one enter); with SQPOLL the kernel
    /// thread drains the SQ without an enter.
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
        self.ring.submit_all()?;

        // Preallocated completion buffer, reused every iteration.
        let mut completions: Vec<(u64, std::io::Result<u32>, u32)> = Vec::with_capacity(64);
        let mut last_beat = std::time::Instant::now();
        while !stop() {
            if busy_poll {
                self.ring.submit_all()?;
            } else {
                self.ring.submit_and_wait(1)?;
            }
            completions.clear();
            self.ring
                .drain_full(|ud, res, flags| completions.push((ud, res, flags)));
            if std::env::var_os("FDS_IOU_DEBUG").is_some() && last_beat.elapsed().as_millis() >= 500 {
                let (p, b, d) = metrics.totals();
                let free = self.pool.free.len();
                let outstanding: usize = self.conns.values().map(|c| c.outstanding).sum();
                let conns = self.conns.len();
                eprintln!(
                    "iou: beat pkts={p} bytes={b} drops={d} pool_free={free}/{} outstanding={outstanding} conns={conns}",
                    POOL_TOTAL
                );
                last_beat = std::time::Instant::now();
            }
            for (ud, res, flags) in completions.drain(..) {
                self.dispatch(ud, res, flags, metrics, core, metrics_server)?;
                // Flush the submission batch before the SQ overflows;
                // with SQPOLL this enter is a no-op (kernel thread
                // drains the SQ).
                if self.ring.queued() >= FLUSH_AT {
                    self.ring.submit_all()?;
                }
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
        flags: u32,
        metrics: &Metrics,
        core: usize,
        metrics_server: &mut Option<MetricsServer>,
    ) -> std::io::Result<()> {
        if std::env::var_os("FDS_IOU_DEBUG").is_some() {
            eprintln!(
                "iou: dispatch kind={:#x} ud={:#x} res={:?} flags={:#x}",
                user_data & KIND_MASK,
                user_data,
                res.as_ref().map(|n| *n),
                flags
            );
        }
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
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.raw_os_error() == Some(libc::EAGAIN) =>
                    {
                        // Nonblocking RecvMsg completed empty: re-arm,
                        // do not count as a drop.
                        self.submit_udp_recv(slot)?;
                    }
                    Err(_) => {
                        metrics.add_drops(core, 1);
                        self.submit_udp_recv(slot)?;
                    }
                }
            }
            KIND_UDP_SEND => {
                let slot = (user_data & !KIND_MASK) as usize;
                self.submit_udp_recv(slot)?;
            }
            KIND_ACCEPT => self.dispatch_accept(res, flags, metrics, core)?,
            KIND_TCP_READ => self.dispatch_tcp_recv(user_data, res, flags, metrics, core)?,
            KIND_TCP_SEND => self.dispatch_tcp_send(user_data, res, flags, metrics, core)?,
            KIND_TCP_POLLOUT => {
                let token = user_data & !KIND_MASK;
                if let Some(c) = self.conns.get_mut(&token) {
                    c.poll_out = false;
                }
                self.flush_sends(token)?;
            }
            KIND_TCP_CLOSE => {
                // RemoveBuffers completion: `res` = buffers removed from
                // the group head (FIFO). Recycle them and finish the
                // close when nothing else references the conn.
                let token = user_data & !KIND_MASK;
                let removed = res.map_or(0, |n| n as usize);
                {
                    let c = self.conns.get_mut(&token);
                    let Some(c) = c else { return Ok(()) };
                    c.draining = false;
                    let take = removed.min(c.provided.len());
                    for _ in 0..take {
                        if let Some(id) = c.provided.pop_front() {
                            self.pool.give_back(id);
                        }
                    }
                }
                self.try_finish_close(token)?;
            }
            KIND_PROVIDE => {
                // ProvideBuffers completion: nothing to advance.
            }
            KIND_POLL => {
                if let Some(s) = metrics_server {
                    while let Ok(true) = s.poll_once(metrics) {}
                }
                self.submit_poll()?;
            }
            KIND_TIMEOUT => {
                self.submit_timeout()?;
            }
            _ => {
                // Unknown token: nothing to advance.
            }
        }
        Ok(())
    }

    /// Accept completions (multishot or single-shot).
    fn dispatch_accept(
        &mut self,
        res: std::io::Result<u32>,
        flags: u32,
        metrics: &Metrics,
        core: usize,
    ) -> std::io::Result<()> {
        if let Ok(fd) = res {
            let fd = fd as i32;
            let peer = if self.legacy {
                addr_from_storage(&self.accept_addr)
            } else {
                peer_of(fd).unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap())
            };
            match self.conn_table.acquire_index() {
                Some(idx) => {
                    self.conn_table.conn_mut(idx).cold.peer = peer;
                    let token = ConnectionId::new(self.core as u32, idx as u32).as_u64();
                    self.conns.insert(token, TcpRingConn::new(fd));
                    if self.legacy {
                        self.submit_legacy_tcp_read(token)?;
                    } else {
                        self.provide_and_arm_recv(token)?;
                    }
                }
                None => {
                    metrics.add_drops(core, 1);
                    // SAFETY: closing a freshly accepted fd this datapath
                    // owns.
                    unsafe {
                        libc::close(fd);
                    }
                }
            }
        }
        if self.accept_multi {
            if flags & CQE_F_MORE == 0 && self.listen_fd >= 0 {
                self.submit_accept()?;
            }
        } else if self.listen_fd >= 0 {
            self.submit_accept()?;
        }
        Ok(())
    }

    /// A multishot (or legacy single-shot) recv completion.
    fn dispatch_tcp_recv(
        &mut self,
        user_data: u64,
        res: std::io::Result<u32>,
        flags: u32,
        metrics: &Metrics,
        core: usize,
    ) -> std::io::Result<()> {
        let token = user_data & !KIND_MASK;
        if self.legacy {
            return self.dispatch_legacy_tcp_recv(token, res, metrics, core);
        }
        let Some(c) = self.conns.get_mut(&token) else {
            return Ok(());
        };
        match res {
            Ok(n) if n > 0 => {
                // The buffer id is in the upper 16 bits of the CQE flags.
                let buf_id = (flags >> CQE_BUFFER_SHIFT) & 0xFFFF;
                if flags & CQE_F_BUFFER == 0 || buf_id as usize >= POOL_TOTAL {
                    metrics.add_drops(core, 1);
                    return Ok(());
                }
                // The kernel prepends io_uring_recvmsg_out; parse it for
                // the payload offset and length.
                // SAFETY: the buffer is pool-owned and registered; the
                // kernel wrote the header before completing.
                let hdr = unsafe {
                    (self.pool.ptr_mut(buf_id) as *const RecvMsgOut).read_unaligned()
                };
                let off = std::mem::size_of::<RecvMsgOut>() + hdr.namelen as usize
                    + hdr.controllen as usize;
                let plen = hdr.payloadlen as usize;
                if off + plen > CONN_BUF {
                    // Malformed completion: recycle the buffer, count a
                    // drop.
                    self.pool.give_back(buf_id);
                    c.provided.retain(|&x| x != buf_id);
                    metrics.add_drops(core, 1);
                    return Ok(());
                }
                self.pool.mark_owned(buf_id, off as u16, plen as u16);
                c.provided.retain(|&x| x != buf_id);
                c.to_send.push_back(buf_id);
                c.outstanding += 1;
                let slot = ConnectionId::from_u64(token).slot() as usize;
                let hot = &mut self.conn_table.conn_mut(slot).hot;
                hot.seq = hot.seq.wrapping_add(plen as u32);
                hot.last_activity = crate::util::now_ticks();
                metrics.add_packets(core, 1);
                metrics.add_bytes(core, plen as u64);
                if flags & CQE_F_MORE == 0 {
                    // The multishot terminated after this data (the
                    // socket is closed or the recv errored): no more
                    // recvs will arrive for this op.
                    c.recv_armed = false;
                }
                if c.outstanding >= HWM_BUFS && c.recv_armed {
                    // Backpressure: stop reading until the echo drains.
                    self.ring.ring_cancel(KIND_TCP_READ | token)?;
                    c.recv_armed = false;
                }
                self.flush_sends(token)?;
            }
            Ok(_) => {
                // EOF: the multishot terminated; close our side.
                c.recv_armed = false;
                self.close_tcp(token, metrics, core)?;
            }
            Err(e) => {
                let errno = e.raw_os_error();
                if errno == Some(libc::ECANCELED) || errno == Some(libc::ENOBUFS) {
                    // Flow-control cancel, or the group ran dry: the recv
                    // is unarmed; re-arm when the watermark allows.
                    c.recv_armed = false;
                    if c.closing {
                        let _ = c;
                        self.maybe_drain_group(token)?;
                        self.try_finish_close(token)?;
                    } else {
                        let _ = c;
                        self.provide_and_arm_recv(token)?;
                    }
                } else if errno == Some(libc::EINVAL) || errno == Some(libc::EOPNOTSUPP) {
                    // Multishot rejected at runtime: downgrade the whole
                    // datapath to legacy (single-shot) mode.
                    let _ = c;
                    self.downgrade_to_legacy()?;
                    self.close_tcp(token, metrics, core)?;
                } else {
                    metrics.add_drops(core, 1);
                    c.recv_armed = false;
                    self.close_tcp(token, metrics, core)?;
                }
            }
        }
        Ok(())
    }

    /// SEND_ZC completion or notification. The `user_data` encodes the
    /// buffer id in bits 32-47 so the notification (which carries no
    /// buffer id) can recycle the exact buffer.
    fn dispatch_tcp_send(
        &mut self,
        user_data: u64,
        res: std::io::Result<u32>,
        flags: u32,
        metrics: &Metrics,
        core: usize,
    ) -> std::io::Result<()> {
        if self.legacy {
            let token = user_data & !KIND_MASK;
            match res {
                Ok(_) => self.submit_legacy_tcp_read(token)?,
                Err(_) => {
                    metrics.add_drops(core, 1);
                    self.close_tcp(token, metrics, core)?;
                }
            }
            return Ok(());
        }
        let slot = (user_data & 0xFFFF_FFFF) as u32;
        let buf_id = ((user_data >> 32) & 0xFFFF) as u32;
        let token = ConnectionId::new(self.core as u32, slot).as_u64();
        let Some(c) = self.conns.get_mut(&token) else {
            // Closed connection with a late notification: the buffer id
            // is in the user_data, so the pool's per-op count recycles
            // it when the last notification arrives.
            if flags & CQE_F_NOTIF != 0 && (buf_id as usize) < POOL_TOTAL {
                self.pool.notify_zc(buf_id);
            }
            return Ok(());
        };
        if flags & CQE_F_NOTIF != 0 {
            // The kernel no longer references the pages this op sent.
            // The buffer is recycled when every op referencing it has
            // posted its notification (partial sends keep the count
            // above zero until the tail is fully sent).
            if self.pool.notify_zc(buf_id) {
                c.in_kernel.retain(|&x| x != buf_id);
                c.outstanding = c.outstanding.saturating_sub(1);
            }
            if c.closing {
                let _ = c;
                self.maybe_drain_group(token)?;
                self.try_finish_close(token)?;
            } else if c.outstanding <= LWM_BUFS {
                let _ = c;
                self.provide_and_arm_recv(token)?;
            }
        } else {
            match res {
                Ok(n) => {
                    let n = n as usize;
                    metrics.add_bytes(core, n as u64);
                    // A send_zc completion may be SHORT: the kernel
                    // accepted only the first n bytes (its send buffer
                    // had that much room). Re-submit the tail from the
                    // new offset; the buffer stays owned by the conn
                    // until the tail is fully sent and notified.
                    let meta = self.pool.metas[buf_id as usize];
                    let remaining = meta.len as usize;
                    if n < remaining && n > 0 {
                        let new_off = meta.off as usize + n;
                        let new_len = remaining - n;
                        if let Some(pos) = c.in_kernel.iter().position(|&x| x == buf_id) {
                            c.in_kernel.swap_remove(pos);
                            c.to_send.push_front(buf_id);
                            self.pool.cancel_zc(buf_id);
                            self.pool.mark_owned(buf_id, new_off as u16, new_len as u16);
                        }
                        self.flush_sends(token)?;
                    } else if n == 0 {
                        // Nothing accepted: treat like EAGAIN.
                        if let Some(pos) = c.in_kernel.iter().position(|&x| x == buf_id) {
                            c.in_kernel.swap_remove(pos);
                            c.to_send.push_front(buf_id);
                            self.pool.cancel_zc(buf_id);
                            self.pool.mark_owned(buf_id, meta.off, meta.len);
                        }
                        if !c.poll_out {
                            self.ring.submit_poll(
                                c.fd,
                                poll_flags(Interest::Writable),
                                KIND_TCP_POLLOUT | token,
                            )?;
                            c.poll_out = true;
                        }
                    } else if self.pool.notify_zc(buf_id) {
                        // Full send: recycle now (IORING_OP_SEND, one CQE).
                        c.in_kernel.retain(|&x| x != buf_id);
                        c.outstanding = c.outstanding.saturating_sub(1);
                        if c.closing {
                            let _ = c;
                            self.maybe_drain_group(token)?;
                            self.try_finish_close(token)?;
                        } else if c.outstanding <= LWM_BUFS {
                            let _ = c;
                            self.provide_and_arm_recv(token)?;
                        }
                    }
                }
                Err(e) if e.raw_os_error() == Some(libc::EAGAIN) => {
                    // Send buffer full: put the buffer back on the send
                    // queue (the notification expectation is dropped;
                    // the pages were never referenced) and arm POLLOUT.
                    let meta = self.pool.metas[buf_id as usize];
                    if let Some(pos) = c.in_kernel.iter().position(|&x| x == buf_id) {
                        c.in_kernel.swap_remove(pos);
                        c.to_send.push_front(buf_id);
                        self.pool.cancel_zc(buf_id);
                        self.pool.mark_owned(buf_id, meta.off, meta.len);
                    }
                    if !c.poll_out {
                        self.ring
                            .submit_poll(c.fd, poll_flags(Interest::Writable), KIND_TCP_POLLOUT | token)?;
                        c.poll_out = true;
                    }
                }
                Err(_) => {
                    metrics.add_drops(core, 1);
                    let _ = c;
                    self.close_tcp(token, metrics, core)?;
                }
            }
        }
        Ok(())
    }

    /// Submit IORING_OP_SEND for every buffer queued on `token`.
    /// The kernel accepts what its send buffer holds and completes the
    /// rest with -EAGAIN (handled in [`Self::dispatch_tcp_send`]);
    /// POLLOUT then arms the writable edge.
    fn flush_sends(&mut self, token: u64) -> std::io::Result<()> {
        loop {
            let outcome = {
                let c = self.conns.get_mut(&token);
                let Some(c) = c else { return Ok(()) };
                if c.closing {
                    return Ok(());
                }
                let Some(buf_id) = c.to_send.front().copied() else {
                    return Ok(());
                };
                let meta = self.pool.metas[buf_id as usize];
                let ptr = unsafe { self.pool.ptr(buf_id).add(meta.off as usize) };
                let user_data = KIND_TCP_SEND | ((buf_id as u64) << 32) | (token & 0xFFFF_FFFF);
                let entry = io_uring::opcode::Send::new(
                    io_uring::types::Fd(c.fd),
                    ptr,
                    meta.len as u32,
                )
                .build()
                .flags(Flags::ASYNC)
                .user_data(user_data);
                // Push BEFORE moving the buffer between lists: a failed
                // push must leave the queue untouched (the buffer is
                // retried on the next flush).
                let queued = self.ring.queued();
                match self.ring.push(user_data, entry) {
                    Ok(()) => {
                        c.to_send.pop_front();
                        c.in_kernel.push(buf_id);
                        self.pool.submit_zc(buf_id);
                        (Ok(()), queued)
                    }
                    Err(e) => (Err(e), queued),
                }
            };
            match outcome.0 {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // SQ full: flush the batch; the queue is untouched,
                    // so the next flush retries the same buffer.
                    self.ring.submit_all()?;
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
            if outcome.1 >= FLUSH_AT {
                self.ring.submit_all()?;
            }
        }
    }

    /// Provide up to INIT_PROVIDE free buffers to `token`'s group and
    /// arm its multishot recv. Buffers are provided ONLY when the recv
    /// will be armed: providing while paused (over the watermark) would
    /// strand them in the group, since nothing drains a group with no
    /// armed recv.
    fn provide_and_arm_recv(&mut self, token: u64) -> std::io::Result<()> {
        let (armed, closing, can_read) = {
            let c = self.conns.get_mut(&token);
            let Some(c) = c else { return Ok(()) };
            (c.recv_armed, c.closing, c.outstanding < HWM_BUFS)
        };
        if closing || armed || !can_read {
            return Ok(());
        }
        // Provide buffers (one op each; the ids need not be contiguous).
        for _ in 0..INIT_PROVIDE {
            let (id, addr) = {
                let c = self.conns.get_mut(&token);
                let Some(c) = c else { break };
                let Some(id) = self.pool.take_free() else { break };
                c.provided.push_back(id);
                (id, self.pool.ptr_mut(id))
            };
            let user_data = KIND_PROVIDE;
            let entry = io_uring::opcode::ProvideBuffers::new(
                addr,
                CONN_BUF as i32,
                1,
                token as u16,
                id as u16,
            )
            .build()
            .user_data(user_data);
            // SAFETY: the buffer is pool-owned and stays valid until a
            // recv consumes it (the lifecycle in the module docs).
            self.ring.push(user_data, entry)?;
        }
        self.submit_modern_recv(token)
    }

    /// Arm the multishot recv for `token`'s connection.
    fn submit_modern_recv(&mut self, token: u64) -> std::io::Result<()> {
        let (fd, armed, closing) = {
            let c = self.conns.get_mut(&token);
            let Some(c) = c else { return Ok(()) };
            (c.fd, c.recv_armed, c.closing)
        };
        if armed || closing {
            return Ok(());
        }
        let user_data = KIND_TCP_READ | token;
        let conn_msg = self.conns.get(&token).map(|c| &*c.recv_msg as *const libc::msghdr);
        let Some(msg) = conn_msg else { return Ok(()) };
        // SAFETY: `recv_msg` is boxed and lives until the op terminates;
        // only msg_namelen/msg_controllen are read for a stream socket.
        let entry = io_uring::opcode::RecvMsgMulti::new(io_uring::types::Fd(fd), msg, token as u16)
            .build()
            .user_data(user_data);
        self.ring.push(user_data, entry)?;
        if let Some(c) = self.conns.get_mut(&token) {
            c.recv_armed = true;
        }
        Ok(())
    }

    /// Start closing `token`'s connection: stop reads, free the
    /// user-space buffers, and drain the provided group once the recv
    /// op is dead.
    fn close_tcp(&mut self, token: u64, _metrics: &Metrics, _core: usize) -> std::io::Result<()> {
        if self.legacy {
            return self.close_legacy_tcp(token);
        }
        let (fd, recv_armed) = {
            let c = self.conns.get_mut(&token);
            let Some(c) = c else { return Ok(()) };
            if c.closing {
                return Ok(());
            }
            c.closing = true;
            while let Some(id) = c.to_send.pop_front() {
                // A buffer with a SEND_ZC op still in flight (a partial
                // send's tail) stays owned by the pool; its notification
                // recycles it via the closed-connection path.
                if self.pool.zc_settled(id) {
                    self.pool.give_back(id);
                    c.outstanding = c.outstanding.saturating_sub(1);
                }
            }
            (c.fd, c.recv_armed)
        };
        if fd >= 0 {
            // SAFETY: the fd was accepted by this datapath and is closed
            // exactly once, here; in-flight ops complete with -EBADF.
            unsafe {
                libc::close(fd);
            }
            if let Some(c) = self.conns.get_mut(&token) {
                c.fd = -1;
            }
        }
        if !recv_armed {
            self.maybe_drain_group(token)?;
        }
        self.try_finish_close(token)?;
        Ok(())
    }

    /// Submit REMOVE_BUFFERS for a closing connection's group once its
    /// recv is dead, so the group drains (FIFO) and the buffers recycle.
    /// At most one drain is in flight per connection.
    fn maybe_drain_group(&mut self, token: u64) -> std::io::Result<()> {
        let n = {
            let c = self.conns.get_mut(&token);
            let Some(c) = c else { return Ok(()) };
            if !c.closing || c.recv_armed || c.draining || c.provided.is_empty() {
                return Ok(());
            }
            c.draining = true;
            c.provided.len()
        };
        let user_data = KIND_TCP_CLOSE | token;
        let entry = io_uring::opcode::RemoveBuffers::new(n as u16, token as u16)
            .build()
            .user_data(user_data);
        // SAFETY: the entry carries no user buffer.
        self.ring.push(user_data, entry)
    }

    /// Remove the conn state and release the table slot once no kernel
    /// op references the connection anymore.
    fn try_finish_close(&mut self, token: u64) -> std::io::Result<()> {
        let done = {
            let c = self.conns.get_mut(&token);
            let Some(c) = c else { return Ok(()) };
            c.closing && !c.recv_armed && c.provided.is_empty() && c.in_kernel.is_empty()
        };
        if done {
            if let Some(_c) = self.conns.remove(&token) {
                let slot = ConnectionId::from_u64(token).slot() as usize;
                self.conn_table.release_slot(slot);
            }
        }
        Ok(())
    }

    /// Multishot rejected at runtime: tear down the modern state and run
    /// the legacy single-shot path from here on.
    fn downgrade_to_legacy(&mut self) -> std::io::Result<()> {
        self.legacy = true;
        self.accept_multi = false;
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
        .flags(Flags::ASYNC)
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
        .flags(Flags::ASYNC)
        .user_data(user_data);
        self.ring.push(user_data, entry)
    }

    /// Submit an accept on the listener: multishot when the modern path
    /// is active (one SQE, one CQE per connection, MORE until the
    /// listener closes), otherwise single-shot.
    fn submit_accept(&mut self) -> std::io::Result<()> {
        if !self.legacy {
            let entry = io_uring::opcode::AcceptMulti::new(io_uring::types::Fd(self.listen_fd))
                .flags(libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC)
                .build()
                .user_data(KIND_ACCEPT);
            // SAFETY: multishot accept has no user buffer; the accepted
            // fds are owned by the datapath.
            self.ring.push(KIND_ACCEPT, entry)?;
            self.accept_multi = true;
            return Ok(());
        }
        self.accept_len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        // SAFETY: the accept scratch lives for the datapath's lifetime
        // and is untouched until the completion is dispatched.
        let entry = io_uring::opcode::Accept::new(
            io_uring::types::Fd(self.listen_fd),
            (&mut *self.accept_addr as *mut libc::sockaddr_storage).cast::<libc::sockaddr>(),
            &mut self.accept_len as *mut libc::socklen_t,
        )
        .flags(libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC)
        .build()
        .user_data(KIND_ACCEPT);
        self.ring.push(KIND_ACCEPT, entry)
    }

    // -----------------------------------------------------------------
    // Legacy single-shot TCP path (kernel < 6.0)
    // -----------------------------------------------------------------

    /// Submit a read for `token`'s connection (waits in-kernel for data).
    fn submit_legacy_tcp_read(&mut self, token: u64) -> std::io::Result<()> {
        let (fd, buf, armed, closing) = {
            let c = self.conns.get_mut(&token);
            let Some(c) = c else { return Ok(()) };
            (c.fd, c.legacy_buf.as_mut_ptr(), c.recv_armed, c.closing)
        };
        if armed || closing || fd < 0 {
            return Ok(());
        }
        // SAFETY: the connection's boxed buffer is owned by the datapath
        // and untouched between this submission and its completion.
        let user_data = KIND_TCP_READ | token;
        let entry = io_uring::opcode::Read::new(io_uring::types::Fd(fd), buf, CONN_BUF as u32)
            .build()
            .user_data(user_data);
        self.ring.push(user_data, entry)?;
        if let Some(c) = self.conns.get_mut(&token) {
            c.recv_armed = true;
        }
        Ok(())
    }

    fn dispatch_legacy_tcp_recv(
        &mut self,
        token: u64,
        res: std::io::Result<u32>,
        metrics: &Metrics,
        core: usize,
    ) -> std::io::Result<()> {
        {
            let c = self.conns.get_mut(&token);
            let Some(c) = c else { return Ok(()) };
            c.recv_armed = false;
        }
        match res {
            Ok(n) if n > 0 => {
                let n = n as usize;
                metrics.add_packets(core, 1);
                metrics.add_bytes(core, n as u64);
                let slot = ConnectionId::from_u64(token).slot() as usize;
                let hot = &mut self.conn_table.conn_mut(slot).hot;
                hot.seq = hot.seq.wrapping_add(n as u32);
                hot.last_activity = crate::util::now_ticks();
                self.submit_legacy_tcp_write(token, n)?;
            }
            Ok(_) => {
                self.close_tcp(token, metrics, core)?;
            }
            Err(_) => {
                metrics.add_drops(core, 1);
                self.close_tcp(token, metrics, core)?;
            }
        }
        Ok(())
    }

    /// Echo `n` received bytes back to `token`'s connection (legacy).
    fn submit_legacy_tcp_write(&mut self, token: u64, n: usize) -> std::io::Result<()> {
        let (fd, buf) = {
            let c = self.conns.get_mut(&token);
            let Some(c) = c else { return Ok(()) };
            (c.fd, c.legacy_buf.as_ptr())
        };
        // SAFETY: the buffer holds `n` received bytes; the kernel only
        // reads them for the duration of the write op.
        let user_data = KIND_TCP_SEND | token;
        let entry = io_uring::opcode::Write::new(io_uring::types::Fd(fd), buf, n as u32)
            .build()
            .user_data(user_data);
        self.ring.push(user_data, entry)
    }

    fn close_legacy_tcp(&mut self, token: u64) -> std::io::Result<()> {
        if let Some(c) = self.conns.remove(&token) {
            let slot = ConnectionId::from_u64(token).slot() as usize;
            if c.fd >= 0 {
                // SAFETY: the fd was accepted by this datapath and is
                // closed exactly once, here.
                unsafe {
                    libc::close(c.fd);
                }
            }
            self.conn_table.release_slot(slot);
        }
        Ok(())
    }

    /// Submit the single-shot poll for the metrics listener.
    fn submit_poll(&mut self) -> std::io::Result<()> {
        match self.metrics_fd {
            Some(fd) => self.ring.submit_poll(fd, poll_flags(Interest::Readable), KIND_POLL),
            None => Ok(()),
        }
    }

    /// Submit the periodic timeout (keeps the loop waking while idle).
    fn submit_timeout(&mut self) -> std::io::Result<()> {
        // SAFETY: `self.timeout` has a stable address for the datapath's
        // lifetime; the kernel reads it while the op is pending.
        let entry = io_uring::opcode::Timeout::new(&self.timeout as *const io_uring::types::Timespec)
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
            if c.fd >= 0 {
                // SAFETY: accepted fds owned by this datapath, closed once.
                unsafe {
                    libc::close(c.fd);
                }
            }
        }
    }
}

/// A stream-socket msghdr for multishot recvmsg: no name, no iov (the
/// kernel writes the payload into the provided buffer).
fn tcp_recv_msg() -> libc::msghdr {
    libc::msghdr {
        msg_name: std::ptr::null_mut(),
        msg_namelen: 0,
        msg_iov: std::ptr::null_mut(),
        msg_iovlen: 0,
        msg_control: std::ptr::null_mut(),
        msg_controllen: 0,
        msg_flags: 0,
    }
}

/// Read the peer address of `fd` with `getpeername` (multishot accept
/// delivers no address).
fn peer_of(fd: i32) -> std::io::Result<std::net::SocketAddr> {
    let mut ss: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    // SAFETY: getpeername writes a sockaddr and its length into `ss`/`len`.
    let rc = unsafe {
        libc::getpeername(
            fd,
            (&mut ss as *mut libc::sockaddr_storage).cast::<libc::sockaddr>(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(addr_from_storage(&ss))
}

/// Read a `sockaddr_storage` filled by the kernel on an AF_INET socket
/// as a std address (IPv4 only, matching the engine).
fn addr_from_storage(ss: &libc::sockaddr_storage) -> std::net::SocketAddr {
    use std::net::{Ipv4Addr, SocketAddrV4};
    assert_eq!(ss.ss_family as libc::c_int, libc::AF_INET);
    // SAFETY: AF_INET guarantees the kernel wrote a `sockaddr_in` at
    // this address; both structs start with the family field.
    let sin = unsafe { &*(ss as *const libc::sockaddr_storage).cast::<libc::sockaddr_in>() };
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

    /// Set `stop` when the client thread exits, including on panic, so
    /// `datapath.run` cannot wait forever for a flag that never arrives.
    struct StopOnDrop(Arc<AtomicBool>);
    impl Drop for StopOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    #[test]
    fn io_uring_setup_ok() {
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
        assert_eq!(completions[0].1.as_ref().unwrap(), &5);
        assert_eq!(&buf[..5], b"hello");
        Ok(())
    }

    #[test]
    fn io_uring_sqpoll_fallback() {
        let _ = IoUringReactor::new(8, 1);
    }

    #[test]
    fn register_buffers_path() {
        let mut reactor = IoUringReactor::new(8, 0).expect("io_uring setup failed");
        let mut a = [0u8; 64];
        let mut b = [0u8; 64];
        let mut bufs: Vec<&mut [u8]> = vec![&mut a, &mut b];
        let _ = reactor.register_buffers(&mut bufs);
    }

    #[test]
    fn kernel_version_parses() {
        let (major, minor) = kernel_version();
        assert!(major >= 5, "kernel {major}.{minor} too old for io_uring");
    }

    /// Full datapath smoke over loopback: UDP echo and TCP echo through
    /// the ring, then a clean stop. Runs on this thread (the datapath is
    /// single-owner, not `Send`); a client thread drives the I/O.
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
            let _stop = StopOnDrop(stop2);
            let client = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            client.send_to(b"ring udp echo", udp_addr).unwrap();
            let mut buf = [0u8; 64];
            let (n, _) = client.recv_from(&mut buf).unwrap();
            assert_eq!(&buf[..n], b"ring udp echo");

            let mut stream = std::net::TcpStream::connect(tcp_addr).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream.write_all(b"ring tcp echo").unwrap();
            let mut tbuf = [0u8; 64];
            let n = stream.read(&mut tbuf).unwrap();
            assert_eq!(&tbuf[..n], b"ring tcp echo");

            client.send_to(&[0xABu8; 60_000], udp_addr).unwrap();
            let mut jumbo = vec![0u8; 70_000];
            let (n, _) = client.recv_from(&mut jumbo).unwrap();
            assert_eq!(n, 60_000);
            assert!(jumbo[..n].iter().all(|&b| b == 0xAB));
        });

        datapath
            .run(&(move || stop.load(Ordering::Relaxed)), &metrics, 0, &mut None, true)
            .expect("datapath run failed");
        client.join().unwrap();
    }

    /// The TCP stall regression: a client floods a large payload while
    /// the server echoes it through the ring. The single-shot datapath
    /// could only keep one read and one write in flight per connection,
    /// so the echo rate collapsed and the client stalled. The multishot
    /// datapath keeps reads armed and bounds buffering by the pool, so
    /// the full payload must echo back.
    #[test]
    fn datapath_tcp_write_flood_echoes() {
        let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp_addr = tcp.local_addr().unwrap();
        // 4 MiB socket buffers on both ends, like the engine's TCP
        // config: the default 128 KiB windows would throttle the flood
        // to a trickle (the pipeline is window-limited).
        for fd in [tcp.as_raw_fd()] {
            for opt in [libc::SO_RCVBUF, libc::SO_SNDBUF] {
                let v: libc::c_int = 4 << 20;
                // SAFETY: `fd` is a live listener owned by this test;
                // the kernel copies the option value.
                unsafe {
                    libc::setsockopt(
                        fd,
                        libc::SOL_SOCKET,
                        opt,
                        &v as *const libc::c_int as *const libc::c_void,
                        std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                    );
                }
            }
        }

        let mut datapath =
            IoUringDatapath::new(0, udp.as_raw_fd(), tcp.as_raw_fd(), None, 512, 0).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let metrics = Metrics::new(1);
        const TOTAL: usize = 16 * 1024 * 1024; // 16 MiB
        const CHUNK: usize = 256 * 1024;

        let client = std::thread::spawn(move || {
            let _stop = StopOnDrop(stop2);
            // Nonblocking client: interleave send and echo-drain so the
            // server's backpressure (pause reads at the high watermark)
            // never deadlocks the test.
            let mut stream = std::net::TcpStream::connect(tcp_addr).unwrap();
            stream.set_nonblocking(true).unwrap();
            stream.set_nodelay(true).unwrap();
            for opt in [libc::SO_RCVBUF, libc::SO_SNDBUF] {
                let v: libc::c_int = 4 << 20;
                // SAFETY: `stream` is a live socket owned by this test.
                unsafe {
                    libc::setsockopt(
                        stream.as_raw_fd(),
                        libc::SOL_SOCKET,
                        opt,
                        &v as *const libc::c_int as *const libc::c_void,
                        std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                    );
                }
            }
            let payload = vec![0x42u8; CHUNK];
            let mut sent = 0usize;
            let mut echoed = 0usize;
            let mut rbuf = [0u8; 64 * 1024];
            let deadline = std::time::Instant::now() + Duration::from_secs(45);
            let mut idle = 0usize;
            while (sent < TOTAL || echoed < TOTAL) && std::time::Instant::now() < deadline {
                let mut progressed = false;
                // Drain the echo.
                loop {
                    match stream.read(&mut rbuf) {
                        Ok(0) => panic!("server closed mid-flood"),
                        Ok(n) => {
                            echoed += n;
                            progressed = true;
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(e) => panic!("client read failed: {e}"),
                    }
                }
                if sent < TOTAL {
                    let want = (TOTAL - sent).min(payload.len());
                    match stream.write(&payload[..want]) {
                        Ok(n) => {
                            sent += n;
                            progressed = true;
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(e) => panic!("client send failed: {e}"),
                    }
                }
                if !progressed {
                    idle += 1;
                    if idle > 10_000 {
                        std::thread::sleep(Duration::from_millis(1));
                        idle = 0;
                    }
                }
            }
            assert!(
                echoed >= TOTAL,
                "echo incomplete: {echoed}/{TOTAL}"
            );
            assert!(std::time::Instant::now() < deadline, "flood too slow");
        });

        datapath
            .run(&(move || stop.load(Ordering::Relaxed)), &metrics, 0, &mut None, false)
            .expect("datapath run failed");
        client.join().unwrap();
    }

    /// The multishot recv payload parse: the kernel prepends
    /// `io_uring_recvmsg_out`; the payload offset is header + name +
    /// control. Simulated with a synthetic buffer.
    #[test]
    fn recvmsg_out_header_parses() {
        let mut buf = vec![0u8; CONN_BUF];
        let hdr = RecvMsgOut {
            namelen: 0,
            controllen: 0,
            payloadlen: 1234,
            flags: 0,
        };
        // SAFETY: `buf` is writable for the struct size.
        unsafe {
            (buf.as_mut_ptr() as *mut RecvMsgOut).write(hdr);
        }
        buf[16..16 + 1234].fill(0x77);
        // SAFETY: same layout as the kernel writes.
        let h = unsafe { (buf.as_ptr() as *const RecvMsgOut).read_unaligned() };
        let off = std::mem::size_of::<RecvMsgOut>() + h.namelen as usize + h.controllen as usize;
        assert_eq!(off, 16);
        assert_eq!(h.payloadlen as usize, 1234);
        assert!(buf[off..off + h.payloadlen as usize]
            .iter()
            .all(|&b| b == 0x77));
    }
}
