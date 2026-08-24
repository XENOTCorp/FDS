//! FDS transport engine: a nonblocking, edge-triggered, busy-polling,
//! batched, zero-allocation TCP/UDP/SCTP dataplane on the Mol framework
//! (thesis NT34–NT36 reactor-as-trace, NT46–NT47 batching, NT48 rings;
//! standard policies [IO], [SIMD], [CONC], [SEC], [OBS], [ALLOC]).
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
//!
//! Runtime configuration comes from `config.json` (see [`config`]); the
//! adaptive build tooling is sub-project 3.

pub mod checksum;
pub mod config;
pub mod conn;
pub mod metrics;
pub mod parse;
pub mod reactor;
pub mod tcp;
pub mod udp;

#[cfg(feature = "sctp")]
pub mod sctp;

#[cfg(feature = "io-uring")]
pub mod io_uring_reactor;

#[cfg(feature = "af-xdp")]
pub mod af_xdp;

pub use checksum::{ip_checksum, sctp_checksum, tcp_checksum, udp_checksum};
pub use config::Config;
pub use conn::{ColdState, ConnTable, Connection, ConnectionId, HotState};
pub use reactor::{EpollEvent, Interest, Reactor};

/// The engine's version of the per-core runtime context: the preallocated
/// bundles (rings, buffers, counters) threaded through every reactor step.
/// Transport modules extend it with their own preallocated state.
#[derive(Default)]
pub struct Ctx {
    /// Total packets processed (padded counter, no false sharing).
    pub packets: mol::PaddedCounter,
    /// Total bytes across all packets.
    pub bytes: mol::PaddedCounter,
    /// Dropped packets (full rings, checksum failures, truncated datagrams).
    pub drops: mol::PaddedCounter,
}
