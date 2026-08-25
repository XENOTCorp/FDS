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

// Two deliberate compile-ahead items remain (each has a test, so this
// list is audited, not a dumping ground):
//   - io_uring_reactor::IoUringReactor::register_buffers — zero-copy
//     buffer registration, unwired until the datapath uses registered
//     buffers (tested: register_buffers_path);
//   - sctp::SctpSocket::get_opt_i32 — test-only option introspection
//     (sctp_nodelay_set asserts the option actually took).
// Remove this allow when either lands or is deleted.
#![allow(dead_code)]

pub mod benchmarks;
mod checksum;
#[cfg(test)]
mod alloc_count;
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
