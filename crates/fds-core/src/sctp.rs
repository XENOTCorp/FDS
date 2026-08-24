//! SCTP transport (feature `sctp`, links libsctp): `sctp_recvmsg` /
//! `sctp_sendmsg` with preallocated ancillary control buffers,
//! SCTP_NODELAY, SCTP_EVENTS association notifications, SCTP_INITMSG
//! stream configuration, SCTP_PARTIAL_DELIVERY_POINT, SCTP_MAX_BURST,
//! SCTP_PEELOFF for per-association dedicated sockets, and `sctp_bindx`
//! multi-homing.
//!
//! CONTRACT (implementer): declare the FFI exactly against
//! `<netinet/sctp.h>` (Linux, libsctp): `sctp_bindx`, `sctp_connectx`,
//! `sctp_peeloff`, `sctp_recvmsg`, `sctp_sendmsg`; the structs
//! `sctp_assoc_t`, `sctp_sndrcvinfo`, `sctp_initmsg`, `sctp_event_subscribe`,
//! `sctp_setprim`, and the constants (SCTP_NODELAY, SCTP_EVENTS,
//! SCTP_INITMSG, SCTP_PARTIAL_DELIVERY_POINT, SCTP_MAX_BURST, SCTP_PEELOFF,
//! SCTP_BINDX_ADD_ADDR, ...) from that header. The #[link(name = "sctp")]
//! attribute goes on the extern block. Public API below is binding; the
//! crate compiles with these stubs. Tests: bind/connect over loopback
//! (skipped gracefully with an eprintln when `socket(AF_SCTP, ...)` fails
//! — kernel SCTP module absent), send/recv roundtrip with stream ids,
//! SCTP_NODELAY option set, peeloff exercised if the kernel supports it.

use crate::config::SctpConfig;
use std::net::SocketAddr;

// FFI to libsctp (see contract above). Do NOT edit the signatures
// without matching `<netinet/sctp.h>`.
#[link(name = "sctp")]
extern "C" {
    // CONTRACT: declare sctp_bindx / sctp_connectx / sctp_peeloff /
    // sctp_recvmsg / sctp_sendmsg here, with the exact libc types.
}

/// A nonblocking SCTP one-to-one (or one-to-many) socket.
pub(crate) struct SctpSocket {
    // CONTRACT: implementer chooses fd storage; public API is binding.
    _private: (),
}

impl SctpSocket {
    /// Create and bind an SCTP socket on `addr`, applying `cfg`.
    pub(crate) fn bind(addr: SocketAddr, cfg: &SctpConfig) -> std::io::Result<Self> {
        let _ = (addr, cfg);
        todo!("SctpSocket::bind: implemented by fds-core milestone task")
    }

    /// Send `data` on stream `stream_id` to `dst`.
    pub(crate) fn send_msg(
        &self,
        data: &[u8],
        stream_id: u16,
        dst: SocketAddr,
    ) -> std::io::Result<usize> {
        let _ = (data, stream_id, dst);
        todo!("SctpSocket::send_msg: implemented by fds-core milestone task")
    }

    /// Receive one message; returns the payload length, the sender, and
    /// the stream id. `Err(WouldBlock)` = drained.
    pub(crate) fn recv_msg(&self, buf: &mut [u8], out_stream: &mut u16) -> std::io::Result<(usize, SocketAddr)> {
        let _ = (buf, out_stream);
        todo!("SctpSocket::recv_msg: implemented by fds-core milestone task")
    }

    /// Peel off the association with the given id into its own socket.
    pub(crate) fn peeloff(&self, assoc_id: u32) -> std::io::Result<Self> {
        let _ = assoc_id;
        todo!("SctpSocket::peeloff: implemented by fds-core milestone task")
    }

    /// Add a local address (multi-homing) via `sctp_bindx`.
    pub(crate) fn add_local_addr(&self, addr: SocketAddr) -> std::io::Result<()> {
        let _ = addr;
        todo!("SctpSocket::add_local_addr: implemented by fds-core milestone task")
    }

    /// The raw fd.
    pub(crate) fn as_raw_fd(&self) -> i32 {
        todo!("SctpSocket::as_raw_fd: implemented by fds-core milestone task")
    }
}
