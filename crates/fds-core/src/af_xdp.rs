//! Experimental AF_XDP path (feature `af-xdp`): raw Ethernet sockets
//! with the XDP umem/ring setup — `socket(AF_XDP, SOCK_RAW, 0)`,
//! `XDP_UMEM_REG`, `XDP_RX_RING`/`XDP_TX_RING` (mmap'd), `XDP_BIND`.
//! EXPERIMENTAL: needs an XDP-attached device at runtime; the module
//! compiles everywhere, tests skip when no device is available. The
//! UAPI constants are declared in-crate (libc does not ship them).
//!
//! CONTRACT (implementer): declare the XDP_* setsockopt constants and
//! `struct xdp_umem_reg` / `struct xdp_mmap_offsets` per
//! `<linux/if_xdp.h>`; implement [`XskSocket`] with the public API below.
//! Tests: socket creation with `AddressFamily::XDP` skips gracefully when
//! unsupported; full umem/ring setup is compile-checked but only run when
//! a device is available (skip by default).

/// An AF_XDP socket (umem + rx/tx rings + bind).
pub struct XskSocket {
    // CONTRACT: implementer owns the socket fd and the mmap'd rings.
    _private: (),
}

impl XskSocket {
    /// Open an AF_XDP socket for `ifindex` on queue `queue_id`.
    pub fn open(ifindex: i32, queue_id: u32) -> std::io::Result<Self> {
        let _ = (ifindex, queue_id);
        todo!("XskSocket::open: implemented by fds-core milestone task")
    }

    /// Receive one frame into `out`; `false` = ring empty.
    pub fn recv_frame(&mut self, out: &mut [u8]) -> bool {
        let _ = out;
        todo!("XskSocket::recv_frame: implemented by fds-core milestone task")
    }

    /// Send one frame; `false` = tx ring full.
    pub fn send_frame(&mut self, data: &[u8]) -> bool {
        let _ = data;
        todo!("XskSocket::send_frame: implemented by fds-core milestone task")
    }

    /// The raw fd.
    pub fn as_raw_fd(&self) -> i32 {
        todo!("XskSocket::as_raw_fd: implemented by fds-core milestone task")
    }
}
