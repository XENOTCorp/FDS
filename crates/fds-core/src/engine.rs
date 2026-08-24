//! The built-in engine loop: the minimal runnable dataplane that binds
//! the poller, transports, connection state, counters, and metrics
//! together (thesis ch. 10 reactor-as-trace; standard \[IO\], \[ALLOC\]).
//!
//! Default mode is a UDP + TCP echo server on the addresses from
//! [`crate::config::EngineConfig`], run by **one worker thread per
//! logical CPU** (config `core.threads`; default 0 = auto, which is 2x
//! the physical core count on hyperthreaded machines). Each worker owns
//! its readiness/datapath (epoll busy-poll by default, the io_uring
//! completion-driven datapath as the `io-uring` strategy), its own
//! SO_REUSEPORT socket pair, connection table, receive batch and
//! per-core counters — the kernel steers traffic across workers, so the
//! loop scales with the core count. The transports are the real code
//! (recvmmsg batches, edge-triggered drain, hot/cold connection state);
//! application protocol logic is meant to replace the echo handlers.
//! Limitation (documented): on an echo-write `WouldBlock` the engine
//! counts a drop and keeps draining the read side to EAGAIN — a
//! per-connection send ring (the spec's design) is the production
//! replacement.

use crate::config::Config;
use crate::conn::{ConnTable, Connection, ConnectionId, CONN_CAP};
use crate::metrics::Metrics;
use crate::reactor::{Interest, Reactor};
use crate::signals;
use crate::util::{now_ticks, pin_to_core};
use std::net::SocketAddr;
use std::sync::Arc;

/// Token layout: reserved high tokens for the UDP socket, TCP listener
/// and metrics listener; connection tokens are packed [`ConnectionId`]s
/// (core in the high half, slot in the low half — core is the worker
/// index here).
const TOKEN_UDP: u64 = u64::MAX - 1;
const TOKEN_TCP_LISTENER: u64 = u64::MAX;
const TOKEN_METRICS: u64 = u64::MAX - 2;

/// Run the engine until SIGINT.
pub fn run(cfg: &Config) -> std::io::Result<()> {
    signals::install();
    startup_probes(cfg);
    run_until(cfg, Arc::new(signals::interrupted))
}

/// The engine proper: spawn one worker per logical CPU, plus an optional
/// AF_XDP forwarding thread, and join them when `stop` turns true. The
/// binary passes [`signals::interrupted`]; tests pass their own flag.
fn run_until(
    cfg: &Config,
    stop: Arc<dyn Fn() -> bool + Send + Sync>,
) -> std::io::Result<()> {
    let threads = worker_count(&cfg.core);
    let metrics = Arc::new(Metrics::new(threads));
    eprintln!(
        "fds: starting {threads} workers (core.threads={}, pin_cores={})",
        cfg.core.threads, cfg.core.pin_cores
    );

    // Each worker owns its poller, sockets, conn table and counters;
    // nothing is shared except the metrics bundle (per-core slots) and
    // the stop flag.
    let mut workers = Vec::with_capacity(threads);
    for id in 0..threads {
        let cfg = cfg.clone();
        let metrics = metrics.clone();
        let stop = stop.clone();
        workers.push(
            std::thread::Builder::new()
                .name(format!("fds-worker-{id}"))
                .stack_size(cfg.core.stack_bytes)
                .spawn(move || worker_main(id, &cfg, &metrics, &*stop))
                .map_err(|e| std::io::Error::other(format!("spawn worker {id}: {e}")))?,
        );
    }

    // Experimental AF_XDP: when a device is configured and opens, a
    // dedicated thread forwards frames on that queue (kernel bypass);
    // otherwise this is a no-op and the engine runs on the kernel
    // datapath.
    let xdp = spawn_xdp_thread(cfg, &stop);

    for w in workers {
        // The outer `?` propagates a worker panic; the inner one
        // propagates the worker's own error (e.g. a bind failure) instead
        // of letting `thread::spawn` drop it silently.
        let result = w
            .join()
            .map_err(|_| std::io::Error::other("worker panicked"))?;
        result?;
    }
    if let Some(x) = xdp {
        let forwarded = x.join().unwrap_or(0);
        eprintln!("fds: af_xdp thread stopped ({forwarded} frames forwarded)");
    }
    let (p, b, d) = metrics.totals();
    eprintln!("fds: engine stopped ({p} packets, {b} bytes, {d} drops)");
    Ok(())
}

/// Worker thread count: the configured value, or one per logical CPU
/// (2x the physical core count on hyperthreaded machines).
fn worker_count(core: &crate::config::CoreConfig) -> usize {
    if core.threads > 0 {
        core.threads
    } else {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    }
}

/// One worker: pin to its CPU, bind its SO_REUSEPORT socket pair, then
/// run the configured strategy's loop until `stop`.
fn worker_main(
    id: usize,
    cfg: &Config,
    metrics: &Metrics,
    stop: &(dyn Fn() -> bool + Send + Sync),
) -> std::io::Result<()> {
    if cfg.core.pin_cores {
        match pin_to_core(id) {
            Ok(()) => eprintln!("fds: worker {id} pinned to cpu {id}"),
            Err(e) => eprintln!("fds: worker {id} pinning unavailable ({e}), unpinned"),
        }
    }

    let udp_addr: SocketAddr = parse_addr(&cfg.engine.udp_bind, "127.0.0.1:7777");
    let tcp_addr: SocketAddr = parse_addr(&cfg.engine.tcp_bind, "127.0.0.1:7778");

    // SO_REUSEPORT (config default) lets every worker bind the same
    // address; the kernel steers datagrams/connections across workers.
    let udp_sock = crate::udp::UdpSocket::new(udp_addr, &cfg.udp)?;
    let tcp_listener = crate::tcp::TcpListener::bind(tcp_addr, &cfg.tcp, libc::SOMAXCONN)?;

    // The metrics pull endpoint lives on worker 0 only; it aggregates
    // the per-core counters of every worker.
    let metrics_path = if cfg.metrics.socket_path.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(&cfg.metrics.socket_path))
    };
    let mut metrics_server = match &metrics_path {
        Some(p) => Some(crate::metrics::MetricsServer::bind(p)?),
        None => None,
    };

    if id == 0 {
        eprintln!(
            "fds: engine up — udp echo on {udp_addr}, tcp echo on {tcp_addr} (Ctrl-C to stop)"
        );
    }

    match cfg.reactor.strategy {
        crate::config::ReactorStrategy::EpollBusyPoll => worker_epoll_loop(
            id,
            cfg,
            &udp_sock,
            &tcp_listener,
            &mut metrics_server,
            metrics,
            stop,
        ),
        crate::config::ReactorStrategy::IoUring => {
            #[cfg(feature = "io-uring")]
            {
                worker_io_uring_loop(
                    id,
                    cfg,
                    &udp_sock,
                    &tcp_listener,
                    &mut metrics_server,
                    metrics,
                    stop,
                )
            }
            #[cfg(not(feature = "io-uring"))]
            {
                eprintln!("fds: worker {id}: io-uring feature disabled; using epoll");
                worker_epoll_loop(
                    id,
                    cfg,
                    &udp_sock,
                    &tcp_listener,
                    &mut metrics_server,
                    metrics,
                    stop,
                )
            }
        }
    }
}

/// The default strategy: epoll edge-triggered busy-poll readiness with
/// the syscall transports (recvmmsg/sendmmsg, readv/writev).
fn worker_epoll_loop(
    id: usize,
    cfg: &Config,
    udp_sock: &crate::udp::UdpSocket,
    tcp_listener: &crate::tcp::TcpListener,
    metrics_server: &mut Option<crate::metrics::MetricsServer>,
    metrics: &Metrics,
    stop: &(dyn Fn() -> bool + Send + Sync),
) -> std::io::Result<()> {
    let mut reactor = Reactor::new(cfg.reactor.max_events)?;
    reactor.register(udp_sock.as_raw_fd(), TOKEN_UDP, Interest::Readable)?;
    reactor.register(tcp_listener.as_raw_fd(), TOKEN_TCP_LISTENER, Interest::Readable)?;
    if let Some(s) = metrics_server {
        reactor.register(s.as_raw_fd(), TOKEN_METRICS, Interest::Readable)?;
    }

    // Per-worker connection table + active-stream map (the map allocates
    // only at connection setup/teardown, never per packet). The
    // `ConnectionSlot` is HELD for the connection's lifetime: dropping
    // it releases the table slot, so releasing it again at close would
    // double-release (a free-list ring spin).
    let conns: ConnTable<CONN_CAP> = ConnTable::new();
    for i in 0..CONN_CAP {
        conns.initialize(i, Connection::new("0.0.0.0:0".parse().unwrap(), 0));
    }
    let mut streams: std::collections::HashMap<
        u64,
        (crate::tcp::TcpStream, crate::conn::ConnectionSlot<'_, CONN_CAP>),
    > = std::collections::HashMap::new();

    // Preallocated receive batch (hot path allocates nothing). Buffers
    // are sized to the IPv4 UDP wire maximum (65535), so ANY datagram —
    // including loopback GSO/GRO jumbo — is received whole, never
    // truncated: 64 x 64 KiB = 4 MiB, allocated once per worker. The
    // MSG_ZEROCOPY path keeps its own doubled set (the kernel references
    // send pages until the error-queue notification, so a set cannot be
    // reused until then); rx_bufs stays as the auto-disable fallback.
    let mut rx_bufs: Vec<mol::Buffer<{ crate::udp::MAX_DATAGRAM }>> = vec![mol::Buffer::new(); 64];
    let mut rx_out: Vec<crate::udp::RecvResult> = (0..64)
        .map(|_| crate::udp::RecvResult {
            len: 0,
            src: "0.0.0.0:0".parse().unwrap(),
            truncated: false,
        })
        .collect();
    let mut zc: Option<ZcState> = if cfg.udp.zerocopy {
        Some(ZcState::new())
    } else {
        None
    };

    let timeout_ms = if cfg.reactor.busy_poll {
        0
    } else {
        cfg.reactor.timeout_ms.max(0)
    };
    let timeout = rustix::event::Timespec {
        tv_sec: (timeout_ms / 1000) as i64,
        tv_nsec: ((timeout_ms % 1000) as i64) * 1_000_000,
    };
    // Sized to max_events so a full batch is always copied out in one go.
    let mut evbuf = vec![crate::reactor::EpollEvent::default(); cfg.reactor.max_events.max(1)];

    while !stop() {
        let n = reactor.poll_timeout(Some(&timeout))?;
        if n == 0 {
            continue;
        }
        let m = reactor.copy_events(n, &mut evbuf);
        for ev in evbuf.iter().take(m) {
            if ev.error {
                metrics.add_drops(id, 1);
                continue;
            }
            match ev.token {
                TOKEN_UDP => {
                    if let Some(z) = &mut zc {
                        if z.disabled {
                            drain_udp(udp_sock, &mut rx_bufs, &mut rx_out, metrics, id)?;
                        } else {
                            drain_udp_zc(udp_sock, z, metrics, id)?;
                        }
                    } else {
                        drain_udp(udp_sock, &mut rx_bufs, &mut rx_out, metrics, id)?;
                    }
                }
                TOKEN_TCP_LISTENER => {
                    drain_accept(tcp_listener, &mut reactor, &conns, &mut streams, id)?;
                }
                TOKEN_METRICS => {
                    // Drain all pending metric requests (edge-triggered:
                    // serve until none remain). Best-effort: a bad client
                    // never kills the engine.
                    if let Some(s) = metrics_server {
                        while let Ok(true) = s.poll_once(metrics) {}
                    }
                }
                tok => drain_tcp(tok, &mut reactor, &conns, &mut streams, metrics, id)?,
            }
        }
    }
    Ok(())
}

/// The `io-uring` strategy: the completion-driven datapath
/// ([`crate::io_uring_reactor::IoUringDatapath`]) — UDP/TCP echo runs
/// through the ring (RECVMSG/SENDMSG/ACCEPT/READ/WRITE) instead of the
/// syscall transports.
#[cfg(feature = "io-uring")]
fn worker_io_uring_loop(
    id: usize,
    cfg: &Config,
    udp_sock: &crate::udp::UdpSocket,
    tcp_listener: &crate::tcp::TcpListener,
    metrics_server: &mut Option<crate::metrics::MetricsServer>,
    metrics: &Metrics,
    stop: &(dyn Fn() -> bool + Send + Sync),
) -> std::io::Result<()> {
    let mut datapath = crate::io_uring_reactor::IoUringDatapath::new(
        id,
        udp_sock.as_raw_fd(),
        tcp_listener.as_raw_fd(),
        metrics_server.as_ref().map(|s| s.as_raw_fd()),
        cfg.reactor.io_uring_entries,
        cfg.reactor.io_uring_sq_thread,
    )?;
    eprintln!(
        "fds: worker {id}: io_uring datapath ({} entries, sq_thread {})",
        cfg.reactor.io_uring_entries, cfg.reactor.io_uring_sq_thread
    );
    datapath.run(
        stop,
        metrics,
        id,
        metrics_server,
        cfg.reactor.busy_poll,
    )
}

fn parse_addr(s: &str, fallback: &str) -> SocketAddr {
    s.parse().unwrap_or_else(|_| {
        eprintln!("fds: bad bind address {s:?} — using {fallback}");
        fallback.parse().unwrap()
    })
}

/// Presence probes for the optional transports (feature-gated; absence is
/// reported, never fatal).
#[allow(unused_variables)]
fn startup_probes(cfg: &Config) {
    #[cfg(feature = "sctp")]
    {
        match crate::sctp::SctpSocket::bind("127.0.0.1:0".parse().unwrap(), &cfg.sctp) {
            Ok(_) => eprintln!("fds: sctp available"),
            Err(e) => eprintln!("fds: sctp unavailable (kernel module absent?): {e}"),
        }
    }
    #[cfg(feature = "io-uring")]
    {
        match crate::io_uring_reactor::IoUringReactor::new(8, 0) {
            Ok(_) => eprintln!("fds: io_uring available (experimental path compiled)"),
            Err(e) => eprintln!("fds: io_uring unavailable: {e}"),
        }
    }
    #[cfg(feature = "af-xdp")]
    {
        if !cfg.af_xdp.device.is_empty() {
            eprintln!(
                "fds: af_xdp configured for device {} (queue {})",
                cfg.af_xdp.device, cfg.af_xdp.queue
            );
        }
    }
}

/// SAFETY wrapper: the [`crate::af_xdp::XskSocket`] is created in
/// `spawn_xdp_thread` and moved into the forwarding thread immediately;
/// no other thread ever touches it. Its raw ring/umem pointers are only
/// dereferenced in that thread, which also runs `Drop`.
struct SendXsk(crate::af_xdp::XskSocket);

// SAFETY: single-owner transfer — the socket is only ever accessed from
// the forwarding thread that owns it (see the struct doc).
unsafe impl Send for SendXsk {}

impl SendXsk {
    fn recv_frame(&mut self, out: &mut [u8]) -> Option<usize> {
        self.0.recv_frame(out)
    }
    fn send_frame(&mut self, data: &[u8]) -> bool {
        self.0.send_frame(data)
    }
}

/// Experimental AF_XDP forwarding thread (feature `af-xdp`): bind
/// `cfg.af_xdp.device` queue and forward frames rx->tx until `stop`.
/// Device-gated: absent a configured device, or when the socket cannot
/// open (no XDP-capable driver, no CAP_NET_RAW), the engine logs and
/// runs on the kernel datapath only.
#[cfg(feature = "af-xdp")]
fn spawn_xdp_thread(
    cfg: &Config,
    stop: &Arc<dyn Fn() -> bool + Send + Sync>,
) -> Option<std::thread::JoinHandle<u64>> {
    if cfg.af_xdp.device.is_empty() {
        return None;
    }
    use std::ffi::CString;
    let Ok(dev) = CString::new(cfg.af_xdp.device.as_str()) else {
        eprintln!("fds: af_xdp device name contains a NUL byte");
        return None;
    };
    // SAFETY: `dev` is a valid NUL-terminated C string for the call.
    let ifindex = unsafe { libc::if_nametoindex(dev.as_ptr()) };
    if ifindex == 0 {
        eprintln!(
            "fds: af_xdp device {} not found; kernel datapath only",
            cfg.af_xdp.device
        );
        return None;
    }
    let queue = cfg.af_xdp.queue;
    let stop = stop.clone();
    match crate::af_xdp::XskSocket::open(ifindex as i32, queue) {
        Ok(xsk) => {
            eprintln!(
                "fds: af_xdp bound {} queue {queue} (ifindex {ifindex}); forwarding frames",
                cfg.af_xdp.device
            );
            let mut guarded: SendXsk = SendXsk(xsk);
            Some(
                match std::thread::Builder::new()
                    .name("fds-af-xdp".to_string())
                    .stack_size(cfg.core.stack_bytes)
                    .spawn(move || {
                        let mut buf = [0u8; 4096];
                        let mut forwarded = 0u64;
                        let mut dropped = 0u64;
                        while !stop() {
                            // Method calls on the whole `SendXsk` wrapper
                            // (not the inner field): the move closure then
                            // captures the wrapper, whose unsafe `Send`
                            // impl is in effect. Capturing `guarded.0`
                            // directly would capture the raw-pointer
                            // `XskSocket` field and fail to send.
                            //
                            // recv_frame consumes the RX frame (returning
                            // it to the fill ring); the datapath validates
                            // + rewrites it for echo, and only Echoed
                            // frames are transmitted.
                            if let Some(n) = guarded.recv_frame(&mut buf) {
                                match crate::af_xdp::process_frame(&mut buf[..n]) {
                                    crate::af_xdp::FrameAction::Echo => {
                                        if guarded.send_frame(&buf[..n]) {
                                            forwarded += 1;
                                        }
                                    }
                                    crate::af_xdp::FrameAction::Drop => dropped += 1,
                                }
                            }
                        }
                        eprintln!(
                            "fds: af_xdp thread stopped ({forwarded} forwarded, {dropped} dropped)"
                        );
                        forwarded
                    }) {
                    Ok(h) => h,
                    Err(e) => {
                        eprintln!("fds: af_xdp thread spawn failed: {e}");
                        return None;
                    }
                },
            )
        }
        Err(e) => {
            eprintln!(
                "fds: af_xdp device {} unavailable ({e}); kernel datapath only",
                cfg.af_xdp.device
            );
            None
        }
    }
}

#[cfg(not(feature = "af-xdp"))]
fn spawn_xdp_thread(
    cfg: &Config,
    _stop: &Arc<dyn Fn() -> bool + Send + Sync>,
) -> Option<std::thread::JoinHandle<u64>> {
    if !cfg.af_xdp.device.is_empty() {
        eprintln!("fds: af-xdp feature disabled; ignoring af_xdp.device");
    }
    None
}

/// Drain the UDP socket to EAGAIN, echoing every datagram back to its
/// source via one `sendmmsg` batch per receive batch. No allocation.
fn drain_udp(
    udp: &crate::udp::UdpSocket,
    bufs: &mut [mol::Buffer<{ crate::udp::MAX_DATAGRAM }>],
    out: &mut [crate::udp::RecvResult],
    metrics: &Metrics,
    core: usize,
) -> std::io::Result<()> {
    loop {
        let n = udp.recv_batch(bufs, out)?;
        if n == 0 {
            break;
        }
        // Echo batch: (payload slice, source) pairs referencing the
        // receive buffers; sendmmsg copies before returning.
        let mut echo: [(&[u8], SocketAddr); 64] = [(&[], "0.0.0.0:0".parse().unwrap()); 64];
        let mut m = 0;
        for (idx, r) in out.iter().take(n).enumerate() {
            if r.truncated {
                metrics.add_drops(core, 1);
                continue;
            }
            metrics.add_packets(core, 1);
            metrics.add_bytes(core, r.len as u64);
            echo[m] = (&bufs[idx].as_slice()[..r.len], r.src);
            m += 1;
        }
        if m > 0 {
            let _ = udp.send_batch(&echo[..m]);
        }
    }
    Ok(())
}

/// Smallest datagram that pays for the MSG_ZEROCOPY per-datagram
/// sendmsg syscall (below it, the batched sendmmsg copy is cheaper).
const ZC_MIN_DATAGRAM: usize = 4096;

/// MSG_ZEROCOPY echo state: two receive-buffer sets alternate. The
/// kernel references (does not copy) send pages, so a set whose
/// zero-copy sends are in flight cannot be reused until their
/// error-queue notifications are drained. This kernel reports empty
/// byte ranges on UDP ZC notifications (verified empirically), so
/// recycling is by notification COUNT: the error queue is FIFO and
/// sends are ordered, so cumulative counts are exact.
struct ZcState {
    bufs: [Vec<mol::Buffer<{ crate::udp::MAX_DATAGRAM }>>; 2],
    out: [Vec<crate::udp::RecvResult>; 2],
    /// Per set: (cumulative ZC sends before this set, ZC sends from
    /// this set) — `None` = reusable.
    in_flight: [Option<(u64, u64)>; 2],
    /// Cumulative ZC notifications drained from the error queue.
    acked_notifs: u64,
    /// Cumulative ZC datagrams sent (across both sets).
    sent_total: u64,
    /// The set the next recv targets (prefer alternating).
    next: usize,
    /// Set when notifications stop arriving (this kernel silently
    /// copies UDP MSG_ZEROCOPY sends, so none ever come); the worker
    /// then falls back to the copy path so it never wedges.
    disabled: bool,
    /// When both sets have been in flight without a free one.
    stall_start: Option<std::time::Instant>,
}

impl ZcState {
    fn new() -> Self {
        let mk_out = || {
            (0..64)
                .map(|_| crate::udp::RecvResult {
                    len: 0,
                    src: "0.0.0.0:0".parse().unwrap(),
                    truncated: false,
                })
                .collect()
        };
        Self {
            bufs: [vec![mol::Buffer::new(); 64], vec![mol::Buffer::new(); 64]],
            out: [mk_out(), mk_out()],
            in_flight: [None, None],
            acked_notifs: 0,
            sent_total: 0,
            next: 0,
            disabled: false,
            stall_start: None,
        }
    }

    /// Drain the error queue and free any set whose sends have all been
    /// notified.
    fn recycle(&mut self, udp: &crate::udp::UdpSocket) -> std::io::Result<()> {
        self.acked_notifs += udp.drain_zerocopy_notifications()?;
        for slot in self.in_flight.iter_mut() {
            if let Some((before, count)) = *slot {
                if self.acked_notifs >= before + count {
                    *slot = None;
                }
            }
        }
        Ok(())
    }
}

/// `drain_udp` variant for `cfg.udp.zerocopy`: large datagrams are
/// echoed with MSG_ZEROCOPY (the send buffer pages are referenced, not
/// copied), small ones through the batched copy path. A set is only
/// reused after its in-flight sends are notified, so receive and send
/// stay safe without allocating on the datapath.
fn drain_udp_zc(
    udp: &crate::udp::UdpSocket,
    zc: &mut ZcState,
    metrics: &Metrics,
    core: usize,
) -> std::io::Result<()> {
    zc.recycle(udp)?;
    loop {
        let set = if zc.in_flight[zc.next].is_none() {
            zc.next
        } else if zc.in_flight[1 - zc.next].is_none() {
            1 - zc.next
        } else {
            // Both sets in flight: wait for notifications. Kernels that
            // silently COPY UDP MSG_ZEROCOPY sends (this kernel does —
            // verified: pages never referenced, no notifications) would
            // wedge the worker forever; after a short grace, disable ZC
            // for this worker and fall back to the copy path.
            zc.recycle(udp)?;
            if zc.in_flight[zc.next].is_some() && zc.in_flight[1 - zc.next].is_some() {
                if zc.stall_start.is_none() {
                    zc.stall_start = Some(std::time::Instant::now());
                }
                if zc.stall_start.unwrap().elapsed() >= std::time::Duration::from_millis(5) {
                    eprintln!(
                        "fds: worker {core}: udp zerocopy sends not completing (kernel copies \
                         silently?) — disabling zerocopy for this worker"
                    );
                    zc.disabled = true;
                    break;
                }
            } else {
                zc.stall_start = None;
            }
            continue;
        };
        let n = udp.recv_batch(&mut zc.bufs[set], &mut zc.out[set])?;
        if n == 0 {
            break;
        }
        let mut copy_msgs: [(&[u8], SocketAddr); 64] = [(&[], "0.0.0.0:0".parse().unwrap()); 64];
        let mut cm = 0;
        let mut zc_sent: u64 = 0;
        for (idx, r) in zc.out[set].iter().take(n).enumerate() {
            if r.truncated {
                metrics.add_drops(core, 1);
                continue;
            }
            metrics.add_packets(core, 1);
            metrics.add_bytes(core, r.len as u64);
            let payload = &zc.bufs[set][idx].as_slice()[..r.len];
            if r.len >= ZC_MIN_DATAGRAM {
                match udp.send_to_zerocopy(payload, r.src) {
                    Ok(sent) => {
                        zc_sent += 1;
                        zc.sent_total += 1;
                        debug_assert_eq!(sent, r.len);
                    }
                    // Fall back to the copy path; the buffer is safe to
                    // hand to sendmmsg because it copies before returning.
                    Err(_) => {
                        copy_msgs[cm] = (payload, r.src);
                        cm += 1;
                    }
                }
            } else {
                copy_msgs[cm] = (payload, r.src);
                cm += 1;
            }
        }
        if cm > 0 {
            let _ = udp.send_batch(&copy_msgs[..cm]);
        }
        zc.in_flight[set] = if zc_sent > 0 {
            Some((zc.sent_total - zc_sent, zc_sent))
        } else {
            None
        };
        zc.next = 1 - set;
    }
    Ok(())
}

/// Accept connections until EAGAIN; register each with the worker's
/// reactor and store its stream + slot guard keyed by its
/// [`ConnectionId`] token. The guard is held for the connection's
/// lifetime (dropping it releases the slot exactly once, at close).
fn drain_accept<'a>(
    listener: &crate::tcp::TcpListener,
    reactor: &mut Reactor,
    conns: &'a ConnTable<CONN_CAP>,
    streams: &mut std::collections::HashMap<
        u64,
        (crate::tcp::TcpStream, crate::conn::ConnectionSlot<'a, CONN_CAP>),
    >,
    core: usize,
) -> std::io::Result<()> {
    loop {
        match listener.accept()? {
            None => break,
            Some((stream, peer)) => {
                match conns.try_acquire() {
                    Some(mut slot) => {
                        let idx = slot.index();
                        slot.conn_mut().cold.peer = peer;
                        let token = ConnectionId::new(core as u32, idx as u32).as_u64();
                        reactor.register(stream.as_raw_fd(), token, Interest::Readable)?;
                        streams.insert(token, (stream, slot));
                    }
                    None => {
                        eprintln!("fds: connection table full; dropping peer {peer}");
                    }
                }
            }
        }
    }
    Ok(())
}

/// Drain one TCP connection: echo received bytes back until EAGAIN.
/// `WouldBlock` during the echo write counts a drop and discards the
/// remainder of the read burst (see module docs).
fn drain_tcp<'a>(
    token: u64,
    reactor: &mut Reactor,
    conns: &'a ConnTable<CONN_CAP>,
    streams: &mut std::collections::HashMap<
        u64,
        (crate::tcp::TcpStream, crate::conn::ConnectionSlot<'a, CONN_CAP>),
    >,
    metrics: &Metrics,
    core: usize,
) -> std::io::Result<()> {
    let slot = ConnectionId::from_u64(token).slot() as usize;
    let close = {
        let stream = match streams.get_mut(&token) {
            Some((s, _)) => s,
            None => return Ok(()),
        };
        let mut close = false;
        loop {
            let mut buf = [0u8; 8192];
            match stream.readv(&mut [&mut buf]) {
                Ok(0) => {
                    close = true;
                    break;
                }
                Ok(n) => {
                    metrics.add_packets(core, 1);
                    metrics.add_bytes(core, n as u64);
                    // Hot state: sequence + activity on every step (the
                    // per-connection hot/cold split in action).
                    let hot = &mut conns.conn_mut(slot).hot;
                    hot.seq = hot.seq.wrapping_add(n as u32);
                    hot.last_activity = now_ticks();
                    match stream.write_all(&buf[..n]) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            metrics.add_drops(core, 1);
                            continue; // discard the rest of the burst
                        }
                        Err(_) => {
                            close = true;
                            break;
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    close = true;
                    break;
                }
            }
        }
        close
    };
    if close {
        // Removing the (stream, slot) tuple drops the slot guard, which
        // releases the table slot exactly once — never call
        // `release_slot` here (that would double-release).
        if let Some((stream, _slot)) = streams.remove(&token) {
            let _ = reactor.unregister(stream.as_raw_fd());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    /// Multi-worker engine smoke: run `run_until` with 2 workers on
    /// ephemeral-ish loopback ports, verify UDP echo (SO_REUSEPORT
    /// distributes across workers) and a TCP echo round trip, then stop.
    #[test]
    fn engine_multithread_echo() {
        let mut cfg = Config::default();
        cfg.core.pin_cores = false;
        cfg.core.threads = 2;
        cfg.core.stack_bytes = 1 << 20;
        cfg.engine.udp_bind = "127.0.0.1:19001".to_string();
        cfg.engine.tcp_bind = "127.0.0.1:19002".to_string();
        cfg.metrics.socket_path = String::new(); // disabled in tests

        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let handle = std::thread::spawn(move || {
            run_until(&cfg, Arc::new(move || stop2.load(Ordering::Relaxed)))
        });

        // UDP: wait until the engine echoes (up to 3s), then exercise
        // both workers with a burst.
        let client = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let payload = b"engine-multithread";
        let mut buf = [0u8; 256];
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut echoed = 0u64;
        while Instant::now() < deadline && echoed == 0 {
            let _ = client.send_to(payload, "127.0.0.1:19001");
            if let Ok((n, _)) = client.recv_from(&mut buf) {
                assert_eq!(&buf[..n], payload, "echo content mismatch");
                echoed += 1;
            }
        }
        assert!(echoed >= 1, "engine never echoed a UDP datagram");
        for _ in 0..20 {
            client.send_to(payload, "127.0.0.1:19001").unwrap();
            let (n, _) = client.recv_from(&mut buf).unwrap();
            assert_eq!(&buf[..n], payload);
        }

        // TCP: one round trip.
        let mut tcp = std::net::TcpStream::connect("127.0.0.1:19002").unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        tcp.write_all(b"hello tcp worker").unwrap();
        let mut tbuf = [0u8; 64];
        let n = tcp.read(&mut tbuf).unwrap();
        assert_eq!(&tbuf[..n], b"hello tcp worker");

        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap().expect("engine run_until failed");
    }

    /// Same smoke on the `io-uring` strategy: the completion-driven
    /// datapath must serve the same UDP + TCP echo over loopback.
    #[test]
    fn engine_io_uring_echo() {
        let mut cfg = Config::default();
        cfg.reactor.strategy = crate::config::ReactorStrategy::IoUring;
        cfg.core.pin_cores = false;
        cfg.core.threads = 2;
        cfg.core.stack_bytes = 1 << 20;
        cfg.engine.udp_bind = "127.0.0.1:19011".to_string();
        cfg.engine.tcp_bind = "127.0.0.1:19012".to_string();
        cfg.metrics.socket_path = String::new();

        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let handle = std::thread::spawn(move || {
            run_until(&cfg, Arc::new(move || stop2.load(Ordering::Relaxed)))
        });

        let client = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let payload = b"engine-io-uring";
        let mut buf = [0u8; 256];
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut echoed = 0u64;
        while Instant::now() < deadline && echoed == 0 {
            let _ = client.send_to(payload, "127.0.0.1:19011");
            if let Ok((n, _)) = client.recv_from(&mut buf) {
                assert_eq!(&buf[..n], payload);
                echoed += 1;
            }
        }
        assert!(echoed >= 1, "io_uring engine never echoed a UDP datagram");

        let mut tcp = std::net::TcpStream::connect("127.0.0.1:19012").unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        tcp.write_all(b"hello io_uring tcp").unwrap();
        let mut tbuf = [0u8; 64];
        let n = tcp.read(&mut tbuf).unwrap();
        assert_eq!(&tbuf[..n], b"hello io_uring tcp");
        // Closing the client surfaces the EOF path (slot release).
        drop(tcp);
        std::thread::sleep(Duration::from_millis(200));

        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap().expect("engine run_until failed");
    }
}
