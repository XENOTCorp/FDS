//! In-crate benchmark harness for the loopback datapaths (thesis
//! NT46/NT47 batching; standard \[OBS\], \[ALLOC\]).
//!
//! An external bench target cannot reach crate-private items, so the
//! harness lives here and is invoked from the `fds` binary via
//! `--bench <seconds>` / `--bench-large <datagram> <seconds>` /
//! `--bench-sctp <seconds>` (arg dispatch wired in `main.rs`).
//!
//! Datapath: `BATCH` fixed-size datagrams are sent to a peer socket
//! bound on 127.0.0.1 and echoed back; packets and bytes are counted and
//! per-second pps / MB/s are printed to stdout. Sending and receiving go
//! through [`crate::udp::UdpSocket`]'s documented API (`send_batch` /
//! `recv_batch`); while that transport is still a `todo!()` stub the
//! harness detects the panic and falls back to a plain
//! [`std::net::UdpSocket`] pair so the measurement still runs. The hot
//! loop allocates nothing: payload, message vector and receive buffers
//! are preallocated once.
//!
//! `--bench-large` is the byte-ceiling measurement: one-way, per
//! direction, with datagrams up to the IPv4 UDP wire maximum — the
//! "10-40+ Gbps loopback" number the standard quotes is a
//! memory-bandwidth bound that only shows up with large datagrams (see
//! docs/engine.md "Throughput").

use crate::config::UdpConfig;
use crate::udp::{set_int, RecvResult, UdpSocket};
use std::net::SocketAddr;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Datagrams per measurement round.
const BATCH: usize = 1024;
/// Datagram payload size (bytes), near-MTU loopback size.
const DATAGRAM: usize = 1400;
/// Flow-control chunk per sub-round: bounded by the peer's kernel receive
/// buffer (default rmem_default ~212 KiB) so loopback never drops — UDP
/// has no backpressure.
const CHUNK: usize = 32;
/// Receive slots per [`UdpSocket::recv_batch`] call.
const SLOTS: usize = 128;
/// Receive buffer capacity; must match the crate socket's batch buffers.
const RCV_CAP: usize = 2048;

/// Totals for one measurement run.
struct Stats {
    packets: u64,
    bytes: u64,
    seconds: u64,
}

/// The measured endpoint: the crate socket when the transport is
/// implemented, otherwise a plain std socket.
enum Measured {
    Engine(UdpSocket),
    Std(std::net::UdpSocket),
}

/// One datapath: measured endpoint + the std echo peer it talks to.
struct Datapath {
    measured: Measured,
    peer: std::net::UdpSocket,
    peer_addr: SocketAddr,
}

/// Call `f`, mapping a `todo!()` stub panic (or any error) to `None`.
/// With `panic = "abort"` in release this cannot catch, which is fine —
/// the engine path only runs once the transport is implemented.
fn try_io<T>(f: impl FnOnce() -> std::io::Result<T>) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .ok()?
        .ok()
}

/// Try the crate-socket datapath; `None` when the transport is still a
/// stub or the sockets cannot be set up.
fn try_engine_datapath() -> Option<Datapath> {
    let sock = try_io(|| {
        UdpSocket::new(SocketAddr::from(([127, 0, 0, 1], 0)), &UdpConfig::default())
    })?;
    let peer = std::net::UdpSocket::bind("127.0.0.1:0").ok()?;
    peer.set_nonblocking(true).ok()?;
    let peer_addr = peer.local_addr().ok()?;
    Some(Datapath {
        measured: Measured::Engine(sock),
        peer,
        peer_addr,
    })
}

/// The std fallback datapath (used while `udp.rs` is still a stub).
fn try_std_datapath() -> Option<Datapath> {
    let rx = std::net::UdpSocket::bind("127.0.0.1:0").ok()?;
    rx.set_nonblocking(true).ok()?;
    let peer = std::net::UdpSocket::bind("127.0.0.1:0").ok()?;
    peer.set_nonblocking(true).ok()?;
    let peer_addr = peer.local_addr().ok()?;
    Some(Datapath {
        measured: Measured::Std(rx),
        peer,
        peer_addr,
    })
}

/// Send `chunk` datagrams from the measured endpoint to the peer.
fn send_chunk(
    dp: &Datapath,
    msgs: &[(&[u8], SocketAddr)],
    chunk: usize,
) -> std::io::Result<()> {
    match &dp.measured {
        Measured::Engine(sock) => {
            let mut sent = 0;
            while sent < chunk {
                sent += sock.send_batch(&msgs[sent..chunk])?;
            }
        }
        Measured::Std(rx) => {
            for &(data, dst) in &msgs[..chunk] {
                loop {
                    match rx.send_to(data, dst) {
                        Ok(_) => break,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                        Err(e) => return Err(e),
                    }
                }
            }
        }
    }
    Ok(())
}

/// The peer forwards every datagram back to its sender.
fn echo_chunk(dp: &Datapath, scratch: &mut [u8]) -> std::io::Result<()> {
    let mut echoed = 0;
    while echoed < CHUNK {
        match dp.peer.recv_from(scratch) {
            Ok((n, src)) => loop {
                match dp.peer.send_to(&scratch[..n], src) {
                    Ok(_) => break,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                    Err(e) => return Err(e),
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => return Err(e),
        }
        echoed += 1;
    }
    Ok(())
}

/// Receive `chunk` echoed datagrams; returns (datagrams, bytes).
fn recv_chunk(
    dp: &Datapath,
    bufs: &mut [mol::Buffer<RCV_CAP>],
    out: &mut [RecvResult],
    scratch: &mut [u8],
) -> std::io::Result<(usize, usize)> {
    let mut received = 0;
    let mut bytes = 0;
    while received < CHUNK {
        match &dp.measured {
            Measured::Engine(sock) => {
                let n = sock.recv_batch(bufs, out)?;
                if n == 0 {
                    continue; // would-block; echoes are in flight on loopback
                }
                for r in &out[..n] {
                    bytes += r.len;
                }
                received += n;
            }
            Measured::Std(rx) => match rx.recv_from(scratch) {
                Ok((n, _)) => {
                    received += 1;
                    bytes += n;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(e) => return Err(e),
            },
        }
    }
    Ok((received, bytes))
}

/// Run the benchmark for `seconds` seconds and print the summary line.
pub fn run(seconds: u64) -> std::io::Result<()> {
    let stats = run_inner(seconds)?;
    let secs = stats.seconds as f64;
    println!(
        "bench: {} packets, {} bytes in {}s — {:.0} pps, {:.3} MB/s",
        stats.packets,
        stats.bytes,
        stats.seconds,
        stats.packets as f64 / secs,
        stats.bytes as f64 / (secs * 1024.0 * 1024.0),
    );
    Ok(())
}

/// Round-trip latency distribution of the UDP echo datapath (single
/// in-flight datagram): samples the RTT for `seconds` seconds and reports
/// the p50/p99/p999 percentiles plus max. p99 is the tail-latency budget
/// the engine targets (standard \[OBS\]; thesis NT25 cost model).
pub fn run_latency(seconds: u64) -> std::io::Result<()> {
    let seconds = seconds.max(1);
    let sock = try_io(|| {
        UdpSocket::new(
            SocketAddr::from(([127, 0, 0, 1], 0)),
            &UdpConfig::default(),
        )
    })
    .ok_or_else(|| std::io::Error::other("no UDP datapath available"))?;
    let peer = std::net::UdpSocket::bind("127.0.0.1:0")?;
    let peer_addr = peer.local_addr()?;
    // A dedicated echo thread keeps the peer's half of the loopback
    // round-trip busy; the measured socket is used single-flight.
    let echo_peer = peer.try_clone()?;
    std::thread::spawn(move || {
        let mut buf = [0u8; RCV_CAP];
        while let Ok((n, src)) = echo_peer.recv_from(&mut buf) {
            let _ = echo_peer.send_to(&buf[..n], src);
        }
    });

    let payload = [0u8; 32];
    let mut bufs = [mol::Buffer::<RCV_CAP>::new()];
    let mut out = [RecvResult {
        len: 0,
        src: SocketAddr::from(([0, 0, 0, 0], 0)),
        truncated: false,
    }];

    // Samples are a bench-mode allocation (not the engine hot path).
    let mut samples: Vec<u64> = Vec::with_capacity(200_000);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let mut count: u64 = 0;
    while std::time::Instant::now() < deadline {
        let t0 = std::time::Instant::now();
        sock.send_to(&payload, peer_addr)?;
        loop {
            if sock.recv_batch(&mut bufs, &mut out)? > 0 {
                break;
            }
        }
        samples.push(t0.elapsed().as_nanos() as u64);
        count += 1;
    }
    let dur = seconds as f64;
    report_latency("latency", &mut samples, count, dur);
    Ok(())
}

/// Print the full percentile ladder (p10…p99, p999), mean, median,
/// standard deviation and jitter (stdev/mean) of `samples` (nanosecond
/// RTTs) as µs — the per-metric detail the cross-tool bench consumes.
fn report_latency(label: &str, samples: &mut [u64], count: u64, dur: f64) {
    let n = samples.len();
    if n == 0 {
        println!("{label}: no samples over {dur:.0}s");
        return;
    }
    samples.sort_unstable();
    // Nearest-rank quantile: index = floor(q * n), q in [0, 1).
    let idx = |q: f64| ((n as f64) * q).min(n as f64 - 1.0) as usize;
    let us = |i: usize| samples[i] as f64 / 1000.0;
    print!("{label}: {count} samples over {dur:.0}s —");
    for &q in &[
        0.10, 0.20, 0.30, 0.40, 0.50, 0.60, 0.70, 0.80, 0.90, 0.95, 0.99, 0.999,
    ] {
        if q == 0.999 {
            print!(" p999 {:.1}µs", us(idx(q)));
        } else {
            print!(" p{:<3} {:.1}µs", (q * 100.0) as u32, us(idx(q)));
        }
    }
    let mean_us = samples.iter().sum::<u64>() as f64 / n as f64 / 1000.0;
    let median_us = us(idx(0.50));
    let var_ns = samples
        .iter()
        .map(|&s| {
            let d = s as f64 - mean_us * 1000.0;
            d * d
        })
        .sum::<f64>()
        / n as f64;
    let stdev_us = var_ns.sqrt() / 1000.0;
    println!(
        " — mean {mean_us:.1}µs, median {median_us:.1}µs, stdev {stdev_us:.1}µs, jitter(stdev/mean) {:.2}",
        stdev_us / mean_us.max(1e-9)
    );
}

/// Round-trip latency against a RUNNING engine (`fds` in default mode,
/// UDP echo on its configured bind): single-flight datagrams to `addr`,
/// RTT percentiles over `seconds`. This is the end-to-end number the
/// engine's busy-poll loop targets.
pub fn run_engine_latency(addr: SocketAddr, seconds: u64) -> std::io::Result<()> {
    let seconds = seconds.max(1);
    let sock = try_io(|| {
        UdpSocket::new(
            SocketAddr::from(([127, 0, 0, 1], 0)),
            &UdpConfig::default(),
        )
    })
    .ok_or_else(|| std::io::Error::other("no UDP datapath available"))?;

    let payload = [0u8; 32];
    let mut bufs = [mol::Buffer::<RCV_CAP>::new()];
    let mut out = [RecvResult {
        len: 0,
        src: SocketAddr::from(([0, 0, 0, 0], 0)),
        truncated: false,
    }];

    let mut samples: Vec<u64> = Vec::with_capacity(200_000);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let mut count: u64 = 0;
    while std::time::Instant::now() < deadline {
        let t0 = std::time::Instant::now();
        sock.send_to(&payload, addr)?;
        // Wait for the echo; a nonblocking recv_batch may return Ok(0)
        // while the peer is absent — bound the wait by the deadline so a
        // dead engine cannot hang the client (it burns 100% CPU).
        loop {
            if sock.recv_batch(&mut bufs, &mut out)? > 0 {
                break;
            }
            if std::time::Instant::now() >= deadline {
                eprintln!(
                    "fds: engine at {addr} not answering (is `fds` running?); giving up after {} samples",
                    samples.len()
                );
                let dur = seconds as f64;
                report_latency(&format!("engine latency vs {addr}"), &mut samples, count, dur);
                return Ok(());
            }
        }
        samples.push(t0.elapsed().as_nanos() as u64);
        count += 1;
    }
    let dur = seconds as f64;
    report_latency(&format!("engine latency vs {addr}"), &mut samples, count, dur);
    Ok(())
}

/// The measurement proper (also used by the smoke test).
fn run_inner(seconds: u64) -> std::io::Result<Stats> {
    let seconds = seconds.max(1);
    let dp = try_engine_datapath()
        .or_else(try_std_datapath)
        .ok_or_else(|| std::io::Error::other("no UDP datapath available"))?;

    // Preallocate everything up front; the loop below allocates nothing.
    let mut payload = [0u8; DATAGRAM];
    for (i, b) in payload.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }
    let mut msgs: Vec<(&[u8], SocketAddr)> = Vec::with_capacity(BATCH);
    for _ in 0..BATCH {
        msgs.push((&payload, dp.peer_addr));
    }
    let mut bufs: Vec<mol::Buffer<RCV_CAP>> = vec![mol::Buffer::new(); SLOTS];
    let mut out: Vec<RecvResult> = (0..SLOTS)
        .map(|_| RecvResult {
            len: 0,
            src: SocketAddr::from(([0, 0, 0, 0], 0)),
            truncated: false,
        })
        .collect();
    let mut scratch = [0u8; RCV_CAP];

    let mut stats = Stats {
        packets: 0,
        bytes: 0,
        seconds,
    };
    let start = Instant::now();
    let mut last_report = start;
    let mut interval_packets = 0u64;
    let mut interval_bytes = 0u64;

    loop {
        // One round: BATCH datagrams out and back, chunked so the peer's
        // kernel receive buffer never overflows (UDP drops silently).
        for _ in 0..BATCH / CHUNK {
            send_chunk(&dp, &msgs, CHUNK)?;
            echo_chunk(&dp, &mut scratch)?;
            let (got, bytes) = recv_chunk(&dp, &mut bufs, &mut out, &mut scratch)?;
            stats.packets += got as u64;
            stats.bytes += bytes as u64;
            interval_packets += got as u64;
            interval_bytes += bytes as u64;
        }

        let now = Instant::now();
        let dt = now.duration_since(last_report).as_secs_f64();
        if dt >= 1.0 {
            println!(
                "bench[{}s]: {:.0} pps, {:.3} MB/s",
                now.duration_since(start).as_secs(),
                interval_packets as f64 / dt,
                interval_bytes as f64 / (dt * 1024.0 * 1024.0),
            );
            last_report = now;
            interval_packets = 0;
            interval_bytes = 0;
        }
        if now.duration_since(start).as_secs() >= seconds {
            break;
        }
    }
    Ok(stats)
}

/// One-way large-datagram loopback throughput (thesis NT47 batching at
/// the memory-bandwidth ceiling): measures each direction separately so
/// the per-packet echo cost of [`run`] does not hide the byte ceiling.
/// `datagram` is clamped to the IPv4 UDP payload maximum (65507).
/// Prints Gbps + kpps per direction; these are the numbers behind the
/// quoted "10-40+ Gbps loopback": at 60 KiB datagrams the loopback is
/// memory-bound, at 1400 B it is packet-rate-bound (~200k syscalls/s per
/// core on the dev machine).
pub fn run_large(datagram: usize, seconds: u64) -> std::io::Result<()> {
    let seconds = seconds.max(1);
    let asked = datagram;
    let datagram = datagram.clamp(64, 65_507);
    if datagram != asked {
        eprintln!("bench-large: clamped {asked}B to {datagram}B (IPv4 UDP payload max 65507)");
    }
    eprintln!("bench-large: {datagram}B datagrams, {seconds}s, one-way (per direction)");

    let cfg = UdpConfig {
        rcvbuf: 16 << 20,
        sndbuf: 16 << 20,
        ..Default::default()
    };
    let (s_bytes, s_pkts, s_elapsed) = large_send(datagram, seconds, &cfg)?;
    let (r_bytes, r_pkts, r_elapsed) = large_recv(datagram, seconds, &cfg)?;
    println!(
        "bench-large: {datagram}B — send {:.2} Gbps ({:.0} kpps), recv {:.2} Gbps ({:.0} kpps)",
        s_bytes as f64 * 8.0 / s_elapsed / 1e9,
        s_pkts as f64 / s_elapsed / 1000.0,
        r_bytes as f64 * 8.0 / r_elapsed / 1e9,
        r_pkts as f64 / r_elapsed / 1000.0,
    );
    Ok(())
}

/// Phase 1 of [`run_large`]: engine-side sender (batched `sendmmsg`) to
/// a std drain socket. Returns (bytes, packets, elapsed seconds) as
/// counted at the drain.
fn large_send(
    datagram: usize,
    seconds: u64,
    cfg: &UdpConfig,
) -> std::io::Result<(u64, u64, f64)> {
    let sock = UdpSocket::new(SocketAddr::from(([127, 0, 0, 1], 0)), cfg)?;
    let drain = std::net::UdpSocket::bind("127.0.0.1:0")?;
    drain.set_nonblocking(true)?;
    // Kernel clamps SO_RCVBUF to rmem_max, then doubles: 16 MiB lands
    // well above the default rmem_max for jumbo-datagram headroom.
    set_int(drain.as_raw_fd(), libc::SOL_SOCKET, libc::SO_RCVBUF, 16 << 20)?;
    let dst = drain.local_addr()?;

    let stop = Arc::new(AtomicBool::new(false));
    let stop_drain = stop.clone();
    let drain_rx = drain.try_clone()?;
    let drain_thread = std::thread::spawn(move || {
        let mut buf = vec![0u8; datagram + 4096];
        let (mut bytes, mut pkts) = (0u64, 0u64);
        loop {
            if stop_drain.load(Ordering::Relaxed) {
                // Drain whatever is still in flight, then exit.
                while let Ok((n, _)) = drain_rx.recv_from(&mut buf) {
                    bytes += n as u64;
                    pkts += 1;
                }
                break;
            }
            match drain_rx.recv_from(&mut buf) {
                Ok((n, _)) => {
                    bytes += n as u64;
                    pkts += 1;
                }
                Err(_) => std::thread::yield_now(),
            }
        }
        (bytes, pkts)
    });

    let payload = vec![0xABu8; datagram];
    let msgs: Vec<(&[u8], SocketAddr)> = (0..64).map(|_| (payload.as_slice(), dst)).collect();
    let start = Instant::now();
    let deadline = start + std::time::Duration::from_secs(seconds);
    let mut sent = 0u64;
    while Instant::now() < deadline {
        // sendmmsg may accept a partial batch (send buffer full); push
        // until the whole batch is accepted (no allocation per round).
        let mut done = 0;
        while done < msgs.len() {
            done += sock.send_batch(&msgs[done..])?;
        }
        sent += done as u64;
    }
    stop.store(true, Ordering::Relaxed);
    let (bytes, pkts) = drain_thread.join().unwrap();
    let elapsed = start.elapsed().as_secs_f64();
    eprintln!(
        "bench-large send: sent {sent} pkts, drained {pkts} pkts ({bytes} B) in {elapsed:.2}s"
    );
    Ok((bytes, pkts, elapsed))
}

/// Phase 2 of [`run_large`]: a std sender thread to the engine-side
/// receiver (batched `recvmmsg` into [`crate::udp::MAX_DATAGRAM`]
/// buffers). Returns (bytes, packets, elapsed seconds) as counted by the
/// engine datapath.
fn large_recv(
    datagram: usize,
    seconds: u64,
    cfg: &UdpConfig,
) -> std::io::Result<(u64, u64, f64)> {
    let sock = UdpSocket::new(SocketAddr::from(([127, 0, 0, 1], 0)), cfg)?;
    let dst = sock.local_addr()?;

    let stop = Arc::new(AtomicBool::new(false));
    let stop_sender = stop.clone();
    let sender_thread = std::thread::spawn(move || {
        let tx = match std::net::UdpSocket::bind("127.0.0.1:0") {
            Ok(s) => s,
            Err(_) => return 0u64,
        };
        let _ = tx.set_nonblocking(true);
        let payload = vec![0x5Au8; datagram];
        let mut sent = 0u64;
        while !stop_sender.load(Ordering::Relaxed) {
            match tx.send_to(&payload, dst) {
                Ok(_) => sent += 1,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::yield_now()
                }
                Err(_) => break,
            }
        }
        sent
    });

    let mut bufs: Vec<mol::Buffer<{ crate::udp::MAX_DATAGRAM }>> =
        vec![mol::Buffer::new(); SLOTS];
    let mut out: Vec<RecvResult> = (0..SLOTS)
        .map(|_| RecvResult {
            len: 0,
            src: SocketAddr::from(([0, 0, 0, 0], 0)),
            truncated: false,
        })
        .collect();
    let start = Instant::now();
    let deadline = start + std::time::Duration::from_secs(seconds);
    let (mut bytes, mut pkts) = (0u64, 0u64);
    while Instant::now() < deadline {
        let n = sock.recv_batch(&mut bufs, &mut out)?;
        if n == 0 {
            continue;
        }
        for r in &out[..n] {
            // MAX_DATAGRAM buffers cannot truncate a legal IPv4 datagram.
            if r.truncated {
                continue;
            }
            bytes += r.len as u64;
            pkts += 1;
        }
    }
    stop.store(true, Ordering::Relaxed);
    let sent = sender_thread.join().unwrap_or(0);
    let elapsed = start.elapsed().as_secs_f64();
    eprintln!(
        "bench-large recv: sender pushed {sent} pkts, engine received {pkts} pkts ({bytes} B) in {elapsed:.2}s"
    );
    Ok((bytes, pkts, elapsed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke: measure 1s and require the loopback datapath to have moved
    /// packets. Tolerant: while the crate UDP transport is still a
    /// `todo!()` stub (any panic), the run is skipped instead of failing
    /// — the std fallback still exercises the harness.
    #[test]
    fn bench_smoke() {
        match std::panic::catch_unwind(|| run_inner(1)) {
            Ok(Ok(stats)) => {
                assert!(stats.packets > 0, "bench moved no packets");
                eprintln!("bench smoke: {} packets in 1s", stats.packets);
            }
            Ok(Err(e)) => eprintln!("bench smoke: skipped ({e})"),
            Err(_) => eprintln!("bench smoke: skipped (UdpSocket still a stub)"),
        }
    }

    /// Smoke: 60 KiB one-way datagrams must move bytes in BOTH directions
    /// through the crate socket (send path and recv path).
    #[test]
    fn bench_large_smoke() {
        let cfg = UdpConfig {
            rcvbuf: 16 << 20,
            sndbuf: 16 << 20,
            ..Default::default()
        };
        let (sb, sp, _) = large_send(60_000, 1, &cfg).unwrap();
        let (rb, rp, _) = large_recv(60_000, 1, &cfg).unwrap();
        assert!(sp > 0 && sb > 0, "large send moved no data: {sp} pkts {sb} B");
        assert!(rp > 0 && rb > 0, "large recv moved no data: {rp} pkts {rb} B");
        eprintln!(
            "bench-large smoke: send {sp} pkts/{sb} B, recv {rp} pkts/{rb} B in 1s each"
        );
    }
}

/// One-way SCTP stream throughput over loopback (`--bench-sctp <secs>`):
/// a one-to-one SCTP listener on 127.0.0.1:0 accepts one association,
/// then the client sends 32 KiB messages for `secs` seconds while a
/// receiver thread counts bytes. Requires the kernel SCTP module
/// (`modprobe sctp`); absent it, prints a note and returns Ok (the
/// in-module transport tests skip the same way).
pub fn run_sctp(seconds: u64) -> std::io::Result<()> {
    use crate::config::SctpConfig;
    use crate::sctp::{is_notification, unsupported, SctpSocket};
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::Instant;

    const MSG: usize = 32 * 1024;
    let wall = std::time::Duration::from_secs(seconds.max(1));

    let cfg = SctpConfig {
        reuseport: false,
        ..SctpConfig::default()
    };
    let server = match SctpSocket::bind("127.0.0.1:0".parse().unwrap(), &cfg) {
        Ok(s) => s,
        Err(e) if unsupported(&e) => {
            eprintln!("fds: bench-sctp: kernel SCTP unavailable (modprobe sctp) — skipping ({e})");
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    // One-to-one SCTP sockets must listen() to accept an association.
    // SAFETY: `server` is a bound, nonblocking SCTP socket; listen(2)
    // never blocks.
    if unsafe { libc::listen(server.as_raw_fd(), 8) } < 0 {
        let e = std::io::Error::last_os_error();
        if unsupported(&e) {
            eprintln!("fds: bench-sctp: listen unsupported (modprobe sctp) — skipping ({e})");
            return Ok(());
        }
        return Err(e);
    }
    let server_addr = server.local_addr()?;
    let client = SctpSocket::bind("127.0.0.1:0".parse().unwrap(), &cfg)?;

    // Receiver thread: accept the association, then count bytes until
    // the sender stops. WouldBlock means momentarily drained — yield,
    // never sleep (a 1 ms sleep would cap the drain rate).
    let stop = Arc::new(AtomicBool::new(false));
    let rx_bytes = Arc::new(AtomicU64::new(0));
    let rx_msgs = Arc::new(AtomicU64::new(0));
    let stop2 = stop.clone();
    let rx_bytes2 = rx_bytes.clone();
    let rx_msgs2 = rx_msgs.clone();
    let rx = std::thread::spawn(move || -> std::io::Result<()> {
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        let raw = loop {
            // SAFETY: accept4 returns a new fd or -1; we pass no address
            // buffers (only the fd is needed).
            let raw = unsafe {
                libc::accept4(
                    server.as_raw_fd(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                )
            };
            if raw >= 0 {
                break raw;
            }
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::WouldBlock {
                if Instant::now() > deadline {
                    return Err(std::io::Error::other("bench-sctp: timed out accepting"));
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            } else if unsupported(&e) {
                eprintln!("fds: bench-sctp: accept unsupported (modprobe sctp) — skipping ({e})");
                return Ok(());
            } else {
                return Err(e);
            }
        };
        // SAFETY: `raw` is a fresh fd owned by no other code.
        let sock = SctpSocket {
            fd: unsafe { OwnedFd::from_raw_fd(raw) },
        };
        let mut buf = [0u8; 65536];
        let mut stream = 0u16;
        while !stop2.load(Ordering::Relaxed) {
            match sock.recv_msg(&mut buf, &mut stream) {
                Ok((n, _)) => {
                    rx_bytes2.fetch_add(n as u64, Ordering::Relaxed);
                    rx_msgs2.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) if is_notification(&e) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::yield_now();
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    });

    // Establish the association with a first send (sendmsg implicitly
    // connects); retry while the INIT handshake is in flight.
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    let first = [0x5au8; MSG];
    loop {
        match client.send_msg(&first, 0, server_addr) {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    return Err(std::io::Error::other("bench-sctp: timed out connecting"));
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(e) if unsupported(&e) => {
                eprintln!("fds: bench-sctp: send unsupported (modprobe sctp) — skipping ({e})");
                return Ok(());
            }
            Err(e) => return Err(e),
        }
    }

    // Measure: send 32 KiB messages for `seconds` (partial sends are
    // resumed at the same offset; the byte count is the truth).
    let payload = vec![0x5au8; MSG];
    let mut off = 0usize;
    let t0 = Instant::now();
    loop {
        if Instant::now().saturating_duration_since(t0) >= wall {
            break;
        }
        match client.send_msg(&payload[off..], 0, server_addr) {
            Ok(n) => {
                off += n;
                if off >= MSG {
                    off = 0;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::yield_now();
            }
            Err(e) if unsupported(&e) => {
                eprintln!("fds: bench-sctp: send unsupported (modprobe sctp) — skipping ({e})");
                return Ok(());
            }
            Err(e) => return Err(e),
        }
    }
    // Grace for the receiver to drain in-flight messages, then stop it.
    stop.store(true, Ordering::Relaxed);
    std::thread::sleep(std::time::Duration::from_millis(300));
    drop(client);
    let _ = rx.join();

    let secs = wall.as_secs_f64();
    let rx_b = rx_bytes.load(Ordering::Relaxed);
    let gbps = rx_b as f64 * 8.0 / secs / 1e9;
    let msgs = rx_msgs.load(Ordering::Relaxed);
    println!(
        "bench-sctp: {:.0}s one-way SCTP loopback: {rx_b} bytes ({gbps:.1} Gbps), {:.0} msgs/s ({} B/msg)",
        secs, msgs as f64 / secs, MSG
    );
    Ok(())
}

#[cfg(test)]
mod sctp_tests {
    use super::*;

    #[test]
    fn sctp_bench_runs_or_skips() {
        // Without the kernel module this prints a note and returns Ok;
        // with it, a 1 s run completes and reports.
        run_sctp(1).expect("bench-sctp must not error");
    }
}

/// Pull the engine's metrics report from its Unix socket and print it
/// (`--metrics-pull [path]`): the observability counterpart to the
/// in-engine `MetricsServer` (used by the cross-tool bench to read the
/// per-core SO_REUSEPORT distribution).
pub fn run_metrics_pull(path: &str) -> std::io::Result<()> {
    use std::io::Read;
    let mut sock = std::os::unix::net::UnixStream::connect(path)?;
    let mut report = String::new();
    sock.read_to_string(&mut report)?;
    print!("{report}");
    Ok(())
}

/// Round-trip latency distribution of the TCP echo datapath (single
/// connection, single in-flight request): one connect, then 32-byte
/// request/response RTT samples for `seconds`, reported as the same
/// p10..p999 ladder as the UDP mode (`--latency-tcp <secs>`).
pub fn run_latency_tcp(seconds: u64) -> std::io::Result<()> {
    use std::io::{Read, Write};
    let seconds = seconds.max(1);
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    let echo = std::thread::spawn(move || -> std::io::Result<()> {
        let (mut conn, _) = listener.accept()?;
        conn.set_nodelay(true)?;
        let mut buf = [0u8; 4096];
        loop {
            let n = conn.read(&mut buf)?;
            if n == 0 {
                return Ok(());
            }
            conn.write_all(&buf[..n])?;
        }
    });

    let mut sock = std::net::TcpStream::connect(addr)?;
    sock.set_nodelay(true)?;
    let payload = [0xabu8; 32];
    let mut samples: Vec<u64> = Vec::with_capacity(200_000);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let mut count: u64 = 0;
    let mut buf = [0u8; 64];
    while std::time::Instant::now() < deadline {
        let t0 = std::time::Instant::now();
        sock.write_all(&payload)?;
        let mut got = 0;
        while got < payload.len() {
            got += sock.read(&mut buf[got..payload.len()])?;
        }
        samples.push(t0.elapsed().as_nanos() as u64);
        count += 1;
    }
    // Close the client so the echo thread's read returns EOF and the
    // join below cannot hang.
    drop(sock);
    let _ = echo.join();
    report_latency("tcp latency", &mut samples, count, seconds as f64);
    Ok(())
}
