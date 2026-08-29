//! FDS transport library.
//!
//! A nonblocking, edge-triggered, batched, zero-allocation TCP/UDP/SCTP
//! dataplane on the Mol framework (reactor as a trace, batching, rings;
//! standard policies [IO], [SIMD], [CONC], [SEC], [OBS], [ALLOC]).
//!
//! # Structure
//!
//! - [`api`]: stable Driver/callback and AsyncRead/AsyncWrite surface
//!   for programs that adopt FDS.
//! - [`reactor`]: one edge-triggered epoll instance with a
//!   drain-to-EAGAIN discipline (the readiness source).
//! - [`tcp`], [`udp`]: nonblocking IPv4/IPv6/dual-stack transports with
//!   the option set from [`config`] applied before bind (SO_REUSEPORT
//!   admission), batch I/O, and zero-copy helpers.
//! - [`sctp`]: SCTP transport (feature `sctp`).
//! - [`conn`]: per-core preallocated connection tables with hot/cold
//!   cache-line separation and packed [`conn::ConnectionId`] tokens.
//! - [`config`]: the runtime configuration surface (`config.json` plus
//!   `FDS_*` env overrides).
//! - [`metrics`]: lock-free per-core counters pulled over a Unix socket.
//! - [`util`]: thread pinning and coarse monotonic ticks.
//! - [`parse`]: bounds-safe IPv4/IPv6/UDP/TCP header parsers.
//! - [`ustack`]: userspace TCP (RACK, software TSO, loss recovery).
//!
//! Optional reactor paths: the io_uring completion-driven datapath
//! (`io_uring_reactor`, feature `io-uring`; registered buffers,
//! multishot recv/accept, SEND_ZC) and the AF_XDP zero-copy frame
//! pipeline (`af_xdp`, feature `af-xdp`; native `XDP_ZEROCOPY`,
//! NUMA-local umem). [`fuzz`] is the deterministic parser and checksum
//! harness used by the `fds` binary.
//!
//! The echo engine and CLI live in the `fds-engine` package (binary `fds`).

pub mod config;
pub mod conn;
pub mod fuzz;
pub mod metrics;
pub mod parse;
pub mod reactor;
pub mod tcp;
pub mod udp;
pub mod ustack;
pub mod util;

mod checksum;

pub mod sctp;

#[cfg(feature = "io-uring")]
pub mod io_uring_reactor;

#[cfg(feature = "af-xdp")]
pub mod af_xdp;
pub mod api;
