//! FDS transport engine: a nonblocking, edge-triggered, busy-polling,
//! batched, zero-allocation TCP/UDP/SCTP dataplane on the Mol framework
//! (thesis NT34–NT36 reactor-as-trace, NT46–NT47 batching, NT48 rings;
//! standard policies [IO], [SIMD], [CONC], [SEC], [OBS], [ALLOC]).
//!
//! This is a BINARY package with no public API: every module is
//! crate-private and the `fds` binary is the product. Runtime
//! configuration comes from `config.json` (see [`config`]); the adaptive
//! build tooling is sub-project 3.
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

// Interim: modules are implemented before the engine-loop wiring lands
// (integration milestone), so crate items are not all reachable from
// `main` yet. Remove this allow when the loop references every module.
#![allow(dead_code)]

mod af_xdp;
mod checksum;
mod config;
mod conn;
mod io_uring_reactor;
mod metrics;
mod parse;
mod reactor;
mod sctp;
mod tcp;
mod udp;

use config::Config;

/// The engine's per-core runtime context: the preallocated bundles
/// (rings, buffers, counters) threaded through every reactor step.
/// Transport modules extend it with their own preallocated state.
#[derive(Default)]
pub(crate) struct Ctx {
    /// Total packets processed (padded counter, no false sharing).
    pub packets: mol::PaddedCounter,
    /// Total bytes across all packets.
    pub bytes: mol::PaddedCounter,
    /// Dropped packets (full rings, checksum failures, truncated datagrams).
    pub drops: mol::PaddedCounter,
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("config.json"));

    let cfg = match std::fs::metadata(&path) {
        Ok(_) => Config::from_file(&path).unwrap_or_else(|e| {
            eprintln!("fds: bad config {}: {e}", path.display());
            std::process::exit(1);
        }),
        Err(_) => {
            let cfg = Config::default();
            eprintln!(
                "fds: no config at {} — using defaults (epoll busy-poll)",
                path.display()
            );
            cfg
        }
    };

    signals::install();
    eprintln!("fds: engine starting (pid {}); Ctrl-C to stop", std::process::id());

    // The engine loop lands with the transports (integration milestone);
    // this skeleton idles until interrupted so the binary is runnable.
    let _ = cfg;
    while !signals::interrupted() {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    eprintln!("fds: shutting down");
}

/// Async-signal-safe Ctrl-C handling (no dependencies): a signal handler
/// that only stores to an atomic; the main loop polls it.
mod signals {
    use std::sync::atomic::{AtomicBool, Ordering};

    static INTERRUPTED: AtomicBool = AtomicBool::new(false);

    extern "C" fn on_sigint(_: libc::c_int) {
        INTERRUPTED.store(true, Ordering::Relaxed);
    }

    /// Install the SIGINT handler (idempotent).
    pub fn install() {
        // SAFETY: the handler does only an atomic store, which is
        // async-signal-safe; libc::signal is safe to call once at startup.
        unsafe {
            libc::signal(libc::SIGINT, on_sigint as *const () as libc::sighandler_t);
        }
    }

    pub fn interrupted() -> bool {
        INTERRUPTED.load(Ordering::Relaxed)
    }
}
