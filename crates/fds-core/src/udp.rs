//! UDP transport (standard [IO], [SIMD]; thesis NT46/NT47 batching):
//! nonblocking sockets with `recvmmsg`/`sendmmsg` batch I/O, UDP_SEGMENT
//! (GSO) and UDP_GRO offloads, optional MSG_ZEROCOPY for large
//! datagrams. The batch ring between recvmmsg and processing is the
//! framework's ring (NT48 invariant).
//!
//! CONTRACT (implementer): implement [`UdpSocket`] on top of libc/rustix
//! with the exact signatures below (the crate compiles with these stubs;
//! replace `todo!()` bodies). Batches reuse preallocated arrays of
//! [`mol::Buffer`]; the hot path must not allocate. Wire the offloads
//! from [`crate::Config`]. Tests: loopback send/recv roundtrip, batch of
//! N datagrams preserves order and content, GSO send when enabled,
//! MSG_TRUNC oversized-datagram detection, truncated/short buffer
//! handling. Mark tests that need offload support with graceful skips
//! when the kernel returns EOPNOTSUPP.

use crate::config::UdpConfig;
use std::net::SocketAddr;

/// A nonblocking UDP socket with batch I/O.
pub struct UdpSocket {
    // CONTRACT: implementer chooses the fd storage (e.g. rustix OwnedFd
    // or std OwnedFd); fields may differ, but the public API below is
    // binding.
    _private: (),
}

/// One receive slot: buffer + sender address + metadata.
pub struct RecvResult {
    pub len: usize,
    pub src: SocketAddr,
    /// True when MSG_TRUNC reported the datagram larger than the buffer.
    pub truncated: bool,
}

impl UdpSocket {
    /// Bind a nonblocking UDP socket (IPv4) to `addr`, applying `cfg`.
    pub fn new(addr: SocketAddr, cfg: &UdpConfig) -> std::io::Result<Self> {
        let _ = (addr, cfg);
        todo!("UdpSocket::new: implemented by fds-core milestone task")
    }

    /// Receive a batch of up to `bufs.len()` datagrams into the given
    /// preallocated buffers. Returns the number of datagrams received
    /// (0 = would block). Callers MUST drain until 0 (drain-to-EAGAIN).
    pub fn recv_batch(
        &self,
        bufs: &mut [mol::Buffer<2048>],
        out: &mut [RecvResult],
    ) -> std::io::Result<usize> {
        let _ = (bufs, out);
        todo!("UdpSocket::recv_batch: implemented by fds-core milestone task")
    }

    /// Send one datagram (single datagram path).
    pub fn send_to(&self, data: &[u8], dst: SocketAddr) -> std::io::Result<usize> {
        let _ = (data, dst);
        todo!("UdpSocket::send_to: implemented by fds-core milestone task")
    }

    /// Send a batch of datagrams (sendmmsg path). Returns the number
    /// sent.
    pub fn send_batch(&self, msgs: &[(&[u8], SocketAddr)]) -> std::io::Result<usize> {
        let _ = msgs;
        todo!("UdpSocket::send_batch: implemented by fds-core milestone task")
    }

    /// The local address.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        todo!("UdpSocket::local_addr: implemented by fds-core milestone task")
    }

    /// The raw fd (for reactor registration).
    pub fn as_raw_fd(&self) -> i32 {
        todo!("UdpSocket::as_raw_fd: implemented by fds-core milestone task")
    }
}
