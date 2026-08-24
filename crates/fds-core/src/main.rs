//! FDS transport engine: a nonblocking, edge-triggered, busy-polling,
//! batched, zero-allocation TCP/UDP/SCTP dataplane on the Mol framework
//! (thesis NT34–NT36 reactor-as-trace, NT46–NT47 batching, NT48 rings;
//! standard policies \[IO\], \[SIMD\], \[CONC\], \[SEC\], \[OBS\], \[ALLOC\]).
//!
//! This is a BINARY package with no public API: every module is
//! crate-private and the `fds` binary is the product. Runtime
//! configuration comes from `config.json` (see [`config`]); the adaptive
//! build tooling is sub-project 3.
//!
//! Usage: `fds [config.json]` runs the engine; `fds --bench <secs>` runs
//! the UDP loopback benchmark; `fds --bench-large <datagram> <secs>` runs
//! the one-way large-datagram byte-ceiling benchmark; `fds --fuzz
//! <iters>` runs the parser fuzz harness.
//!
//! Architecture: per-core [`reactor::Reactor`] instances poll epoll
//! edge-triggered with a drain-to-EAGAIN discipline; transports
//! ([`udp`], [`tcp`], [`sctp`]) batch with recvmmsg/sendmmsg, readv/writev
//! and the sctp equivalents; connection state lives in [`conn`] with
//! hot/cold cache-line separation; parser/checksum atoms are pure
//! molecules ([`parse`], [`checksum`]); observability is lock-free
//! per-core counters pulled over a Unix socket ([`metrics`]).
//!
//! Experimental reactor paths: io_uring SQPOLL ([`io_uring_reactor`],
//! feature `io-uring`) and AF_XDP ([`af_xdp`], feature `af-xdp`).

// The per-core multi-protocol loop has landed (epoll busy-poll with the
// syscall transports, and the io_uring completion-driven datapath with
// RECVMSG/SENDMSG/ACCEPT/READ/WRITE through the ring; AF_XDP's
// process_frame pipeline is wired and unit-tested). What remains unwired
// — and is intentionally compiled ahead of the wiring, per the standard
// — is the SCTP engine path, the zero-copy transport ops (MSG_ZEROCOPY,
// registered buffers, splice_from_fd), the io_uring transport-op
// helpers (submit_read/submit_write), and the cold-state fields those
// transports consume. Remove this allow as each path lands.
#![allow(dead_code)]

mod af_xdp;
mod bench;
mod checksum;
mod config;
mod conn;
mod engine;
mod fuzz;
mod io_uring_reactor;
mod metrics;
mod parse;
mod reactor;
mod sctp;
mod tcp;
mod udp;

use config::Config;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("--bench") => {
            let secs = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2);
            bench::run(secs)
        }
        Some("--bench-large") => {
            let datagram = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(60_000);
            let secs = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3);
            bench::run_large(datagram, secs)
        }
        Some("--latency") => {
            let secs = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2);
            bench::run_latency(secs)
        }
        Some("--latency-against") => {
            let addr: std::net::SocketAddr = args
                .get(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| {
                    eprintln!("fds: --latency-against <addr> [secs] — using 127.0.0.1:7777");
                    "127.0.0.1:7777".parse().unwrap()
                });
            let secs = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2);
            bench::run_engine_latency(addr, secs)
        }
        Some("--fuzz") => {
            let iters = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1_000_000);
            fuzz::run(iters);
            Ok(())
        }
        _ => {
            let path = args
                .first()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("config.json"));
            let cfg = load_config(&path);
            engine::run(&cfg)
        }
    };
    if let Err(e) = code {
        eprintln!("fds: {e}");
        std::process::exit(1);
    }
}

/// Async-signal-safe Ctrl-C handling (no dependencies): a signal handler
/// that only stores to an atomic; the engine loop polls it.
mod signals {
    use std::sync::atomic::{AtomicBool, Ordering};

    static INTERRUPTED: AtomicBool = AtomicBool::new(false);

    extern "C" fn on_sigint(_: libc::c_int) {
        INTERRUPTED.store(true, Ordering::Relaxed);
    }

    /// Install the SIGINT handler (idempotent).
    pub(crate) fn install() {
        // SAFETY: the handler does only an atomic store, which is
        // async-signal-safe; libc::signal is safe to call once at startup.
        unsafe {
            libc::signal(libc::SIGINT, on_sigint as *const () as libc::sighandler_t);
        }
    }

    pub(crate) fn interrupted() -> bool {
        INTERRUPTED.load(Ordering::Relaxed)
    }
}

/// Load `config.json`, falling back to defaults with a note.
fn load_config(path: &std::path::Path) -> Config {
    match std::fs::metadata(path) {
        Ok(_) => match Config::from_file(path) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("fds: bad config {}: {e}", path.display());
                std::process::exit(1);
            }
        },
        Err(_) => {
            let cfg = Config::default();
            eprintln!(
                "fds: no config at {} — using defaults (epoll busy-poll, udp 127.0.0.1:7777)",
                path.display()
            );
            cfg
        }
    }
}
