//! TCP transport (standard [IO], [SEC]; thesis NT38 minimal state):
//! nonblocking `accept4` listeners, `readv`/`writev` scatter-gather for
//! partial reads/writes, `sendfile`/`splice` zero-copy for file-backed
//! responses (valid-fd discipline — no double-close), and the option set
//! from [`crate::Config`] (NODELAY default on, QUICKACK, DEFER_ACCEPT,
//! FASTOPEN config-gated with the spoofing caveat documented, CORK
//! opt-in). Connection state uses [`crate::conn`] hot/cold halves.
//!
//! CONTRACT (implementer): implement [`TcpListener`] and [`TcpStream`]
//! with the exact public API below (the crate compiles with these stubs;
//! replace `todo!()` bodies). Hot path must not allocate. Tests:
//! loopback accept/connect, echo roundtrip over partial reads (write in
//! small chunks), NODELAY/QUICKACK set on the accepted stream, splice of
//! a temp file to the stream, drain-to-EAGAIN on the reader.

use crate::config::TcpConfig;
use std::net::SocketAddr;

/// A nonblocking TCP listener with `accept4(..., SOCK_NONBLOCK)`.
pub struct TcpListener {
    // CONTRACT: implementer chooses fd storage; public API is binding.
    _private: (),
}

/// A nonblocking TCP connection.
pub struct TcpStream {
    // CONTRACT: implementer chooses fd storage; public API is binding.
    _private: (),
}

impl TcpListener {
    /// Bind + listen on `addr`, applying `cfg`.
    pub fn bind(addr: SocketAddr, cfg: &TcpConfig) -> std::io::Result<Self> {
        let _ = (addr, cfg);
        todo!("TcpListener::bind: implemented by fds-core milestone task")
    }

    /// Accept one connection; returns `None` when the accept queue is
    /// empty (EAGAIN).
    pub fn accept(&self) -> std::io::Result<Option<(TcpStream, SocketAddr)>> {
        todo!("TcpListener::accept: implemented by fds-core milestone task")
    }

    /// The raw fd (for reactor registration).
    pub fn as_raw_fd(&self) -> i32 {
        todo!("TcpListener::as_raw_fd: implemented by fds-core milestone task")
    }
}

impl TcpStream {
    /// Read into the buffer, returning bytes read (0 = EOF, Err WouldBlock
    /// means drained — callers treat WouldBlock as drain-to-EAGAIN).
    pub fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let _ = buf;
        todo!("TcpStream::read: implemented by fds-core milestone task")
    }

    /// Scatter-gather read into `bufs`.
    pub fn readv(&mut self, bufs: &mut [&mut [u8]]) -> std::io::Result<usize> {
        let _ = bufs;
        todo!("TcpStream::readv: implemented by fds-core milestone task")
    }

    /// Write all of `data` (handles partial writes internally).
    pub fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        let _ = data;
        todo!("TcpStream::write_all: implemented by fds-core milestone task")
    }

    /// Gathered write.
    pub fn writev(&mut self, bufs: &[&[u8]]) -> std::io::Result<usize> {
        let _ = bufs;
        todo!("TcpStream::writev: implemented by fds-core milestone task")
    }

    /// Zero-copy splice: send `len` bytes from the seekable fd `src_fd`
    /// (e.g. an open file) into this socket. Returns bytes spliced.
    pub fn splice_from_fd(&mut self, src_fd: i32, len: usize) -> std::io::Result<usize> {
        let _ = (src_fd, len);
        todo!("TcpStream::splice_from_fd: implemented by fds-core milestone task")
    }

    /// The peer address.
    pub fn peer_addr(&self) -> std::io::Result<SocketAddr> {
        todo!("TcpStream::peer_addr: implemented by fds-core milestone task")
    }

    /// The raw fd (for reactor registration).
    pub fn as_raw_fd(&self) -> i32 {
        todo!("TcpStream::as_raw_fd: implemented by fds-core milestone task")
    }
}
