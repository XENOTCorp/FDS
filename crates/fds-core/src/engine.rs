//! The built-in engine loop: the minimal runnable dataplane that binds
//! the reactor, transports, connection state, counters, and metrics
//! together (thesis ch. 10 reactor-as-trace; standard [IO], [ALLOC]).
//!
//! Default mode is a UDP + TCP echo server on the addresses from
//! [`crate::config::EngineConfig`]. The transports are the real code
//! (recvmmsg batches, edge-triggered drain, hot/cold connection state);
//! application protocol logic is meant to replace the echo handlers.
//! Limitation (documented): on an echo-write `WouldBlock` the engine
//! counts a drop and keeps draining the read side to EAGAIN — a
//! per-connection send ring (the spec's design) is the production
//! replacement.

use crate::config::Config;
use crate::conn::{ConnTable, Connection, ConnectionId};
use crate::metrics::Metrics;
use crate::reactor::{Interest, Reactor};
use crate::signals;
use crate::Ctx;
use std::net::SocketAddr;

/// Token layout: reserved high tokens for the UDP socket and TCP
/// listener; connection tokens are packed [`ConnectionId`]s (core in the
/// high half, slot in the low half — core 0 here).
const TOKEN_UDP: u64 = u64::MAX - 1;
const TOKEN_TCP_LISTENER: u64 = u64::MAX;
/// Connection table capacity (preallocated slots).
const CONN_CAP: usize = 1024;

/// Run the engine until SIGINT.
pub(crate) fn run(cfg: &Config) -> std::io::Result<()> {
    signals::install();

    let udp_addr: SocketAddr = parse_addr(&cfg.engine.udp_bind, "127.0.0.1:7777");
    let tcp_addr: SocketAddr = parse_addr(&cfg.engine.tcp_bind, "127.0.0.1:7778");

    let udp_sock = crate::udp::UdpSocket::new(udp_addr, &cfg.udp)?;
    let tcp_listener = crate::tcp::TcpListener::bind(tcp_addr, &cfg.tcp)?;

    startup_probes(cfg);

    let mut reactor = Reactor::new(cfg.reactor.max_events)?;
    reactor.register(&udp_sock, TOKEN_UDP, Interest::Readable)?;
    reactor.register(&tcp_listener, TOKEN_TCP_LISTENER, Interest::Readable)?;

    // Preallocated connection table + active-stream map (the map allocates
    // only at connection setup/teardown, never per packet).
    let conns: ConnTable<CONN_CAP> = ConnTable::new();
    for i in 0..CONN_CAP {
        conns.initialize(i, Connection::new("0.0.0.0:0".parse().unwrap(), 0));
    }
    let mut streams: std::collections::HashMap<u64, crate::tcp::TcpStream> =
        std::collections::HashMap::new();

    let metrics_path = if cfg.metrics.socket_path.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(&cfg.metrics.socket_path))
    };
    let mut metrics_server = match &metrics_path {
        Some(p) => Some(crate::metrics::MetricsServer::bind(p)?),
        None => None,
    };
    let metrics = Metrics::new(1);

    let mut ctx = Ctx::default();
    // Preallocated receive batch (hot path allocates nothing).
    let mut rx_bufs: Vec<mol::Buffer<2048>> = vec![mol::Buffer::new(); 64];
    let mut rx_out: Vec<crate::udp::RecvResult> = (0..64)
        .map(|_| crate::udp::RecvResult {
            len: 0,
            src: "0.0.0.0:0".parse().unwrap(),
            truncated: false,
        })
        .collect();

    eprintln!(
        "fds: engine up — udp echo on {udp_addr}, tcp echo on {tcp_addr} (Ctrl-C to stop)"
    );

    while !signals::interrupted() {
        let n = reactor.poll_busy()?;
        // Copy events into a stack buffer so handlers can take &mut
        // reactor (no allocation, no held borrow).
        let mut evbuf = [crate::reactor::EpollEvent {
            token: 0,
            readable: false,
            writable: false,
            hang_up: false,
            error: false,
        }; 256];
        let m = reactor.copy_events(n, &mut evbuf);
        for ev in evbuf.iter().take(m) {
            if ev.error {
                ctx.drops
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                continue;
            }
            match ev.token {
                TOKEN_UDP => drain_udp(&udp_sock, &mut rx_bufs, &mut rx_out, &mut ctx)?,
                TOKEN_TCP_LISTENER => {
                    drain_accept(&tcp_listener, &mut reactor, &conns, &mut streams)?;
                }
                tok => {
                    drain_tcp(tok, &mut reactor, &conns, &mut streams, &mut ctx)?;
                }
            }
        }
        if let Some(s) = &mut metrics_server {
            metrics.set_totals(
                ctx.packets.load(std::sync::atomic::Ordering::Relaxed),
                ctx.bytes.load(std::sync::atomic::Ordering::Relaxed),
                ctx.drops.load(std::sync::atomic::Ordering::Relaxed),
            );
            let _ = s.poll_once(&metrics);
        }
    }

    eprintln!(
        "fds: engine stopped ({} packets, {} bytes, {} drops)",
        ctx.packets.load(std::sync::atomic::Ordering::Relaxed),
        ctx.bytes.load(std::sync::atomic::Ordering::Relaxed),
        ctx.drops.load(std::sync::atomic::Ordering::Relaxed),
    );
    Ok(())
}

fn parse_addr(s: &str, fallback: &str) -> SocketAddr {
    s.parse().unwrap_or_else(|_| {
        eprintln!("fds: bad bind address {s:?} — using {fallback}");
        fallback.parse().unwrap()
    })
}

/// Presence probes for the optional transports (feature-gated; absence is
/// reported, never fatal).
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
        match crate::af_xdp::XskSocket::open(1, 0) {
            Ok(_) => eprintln!("fds: af_xdp available on ifindex 1"),
            Err(e) => eprintln!("fds: af_xdp unavailable (no XDP device?): {e}"),
        }
    }
}

/// Drain the UDP socket to EAGAIN, echoing every datagram back to its
/// source via one `sendmmsg` batch per receive batch. No allocation.
fn drain_udp(
    udp: &crate::udp::UdpSocket,
    bufs: &mut [mol::Buffer<2048>],
    out: &mut [crate::udp::RecvResult],
    ctx: &mut Ctx,
) -> std::io::Result<()> {
    use std::sync::atomic::Ordering::Relaxed;
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
                ctx.drops.fetch_add(1, Relaxed);
                continue;
            }
            ctx.packets.fetch_add(1, Relaxed);
            ctx.bytes.fetch_add(r.len as u64, Relaxed);
            echo[m] = (&bufs[idx].as_slice()[..r.len], r.src);
            m += 1;
        }
        if m > 0 {
            let _ = udp.send_batch(&echo[..m]);
        }
    }
    Ok(())
}

/// Accept connections until EAGAIN; register each with the reactor and
/// store its stream keyed by its [`ConnectionId`] token.
fn drain_accept(
    listener: &crate::tcp::TcpListener,
    reactor: &mut Reactor,
    conns: &ConnTable<CONN_CAP>,
    streams: &mut std::collections::HashMap<u64, crate::tcp::TcpStream>,
) -> std::io::Result<()> {
    loop {
        match listener.accept()? {
            None => break,
            Some((stream, peer)) => {
                let slot = match conns.try_acquire() {
                    Some(mut slot) => {
                        let idx = slot.index();
                        slot.conn_mut().cold.peer = peer;
                        idx
                    }
                    None => {
                        eprintln!("fds: connection table full; dropping peer {peer}");
                        continue;
                    }
                };
                let token = ConnectionId::new(0, slot as u32).as_u64();
                reactor.register(&stream, token, Interest::Readable)?;
                streams.insert(token, stream);
            }
        }
    }
    Ok(())
}

/// Drain one TCP connection: echo received bytes back until EAGAIN.
/// `WouldBlock` during the echo write counts a drop and discards the
/// remainder of the read burst (see module docs).
fn drain_tcp(
    token: u64,
    reactor: &mut Reactor,
    conns: &ConnTable<CONN_CAP>,
    streams: &mut std::collections::HashMap<u64, crate::tcp::TcpStream>,
    ctx: &mut Ctx,
) -> std::io::Result<()> {
    use std::sync::atomic::Ordering::Relaxed;
    let slot = ConnectionId::from_u64(token).slot() as usize;
    let close = {
        let stream = match streams.get_mut(&token) {
            Some(s) => s,
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
                    ctx.packets.fetch_add(1, Relaxed);
                    ctx.bytes.fetch_add(n as u64, Relaxed);
                    match stream.write_all(&buf[..n]) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            ctx.drops.fetch_add(1, Relaxed);
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
        if let Some(stream) = streams.remove(&token) {
            let _ = reactor.unregister(&stream);
            conns.release_slot(slot);
        }
    }
    Ok(())
}
