//! FDS transport engine — library API.
//!
//! A nonblocking, edge-triggered, busy-polling, batched, zero-allocation
//! TCP/UDP/SCTP dataplane on the Mol framework (thesis NT34–NT36
//! reactor-as-trace, NT46–NT47 batching, NT48 rings; standard policies
//! \[IO\], \[SIMD\], \[CONC\], \[SEC\], \[OBS\], \[ALLOC\]).
//!
//! # Structure
//!
//! - [`reactor`] — one edge-triggered epoll instance with a
//!   drain-to-EAGAIN busy-poll discipline (the readiness source).
//! - [`tcp`], [`udp`] — nonblocking transports with the option set from
//!   [`config`] applied before bind (SO_REUSEPORT admission), batch I/O,
//!   and zero-copy helpers.
//! - [`conn`] — per-core preallocated connection tables with hot/cold
//!   cache-line separation and packed [`conn::ConnectionId`] tokens
//!   (thesis NT53).
//! - [`config`] — the runtime configuration surface (single repo-root
//!   `config.json` plus `FDS_*` env overrides).
//! - [`metrics`] — lock-free per-core counters pulled over a Unix socket.
//! - [`util`] — thread pinning and coarse monotonic ticks.
//! - [`engine`] — the built-in echo engine (the reference loop wiring
//!   workers, transports, tables, and counters together; applications
//!   build their own loops on the primitives above).
//!
//! Experimental reactor paths stay internal: the io_uring
//! completion-driven datapath (`io_uring_reactor`, feature `io-uring`)
//! and the AF_XDP frame pipeline (`af_xdp`, feature `af-xdp`), both used
//! by [`engine`]. [`benchmarks`] and [`fuzz`] are the CLI tooling that
//! powers the `fds` binary's `--bench*`/`--fuzz` modes.
//!
//! The `fds` binary (`src/main.rs`) is a thin CLI over this library.

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

pub mod benchmarks;
mod checksum;
pub mod config;
pub mod conn;
pub mod engine;
pub mod fuzz;
mod io_uring_reactor;
pub mod metrics;
mod parse;
pub mod reactor;
mod sctp;
mod signals;
pub mod tcp;
pub mod udp;
pub mod util;

mod af_xdp;
