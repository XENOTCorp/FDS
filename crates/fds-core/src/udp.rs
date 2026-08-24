//! UDP transport (standard \[IO\], \[SIMD\]; thesis NT46/NT47 batching):
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
use std::cell::UnsafeCell;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// Maximum datagrams per `recvmmsg`/`sendmmsg` call; the batch arrays in
/// [`UdpSocket`] are preallocated to this size in `new`.
const MAX_BATCH: usize = 64;

/// Receive-buffer size that fits ANY IPv4 UDP datagram whole: the wire
/// maximum is 65535 bytes (16-bit IPv4 length field), so 65536 = 1<<16
/// never truncates, not even for loopback GSO/GRO jumbo datagrams. The
/// engine and the large-datagram bench allocate [`mol::Buffer`]s of this
/// size; small buffers are only used by tests exercising MSG_TRUNC.
pub(crate) const MAX_DATAGRAM: usize = 65536;

/// `UDP_SEGMENT` (GSO) socket option, `<linux/udp.h>`. libc 0.2 does not
/// export it for glibc targets, so define it here (103, verified against
/// /usr/include/linux/udp.h).
pub(crate) const UDP_SEGMENT: libc::c_int = 103;

/// `UDP_GRO` socket option, `<linux/udp.h>` (104); see [`UDP_SEGMENT`].
pub(crate) const UDP_GRO: libc::c_int = 104;

/// A nonblocking UDP socket with batch I/O.
///
/// The `recvmmsg`/`sendmmsg` header, iovec and address arrays are
/// preallocated here (allocation-free hot path). They sit behind
/// `UnsafeCell` because the batch methods take `&self`; a socket is
/// owned by exactly one reactor thread at a time, so `recv_batch` and
/// `send_batch` never run concurrently on the same socket.
pub(crate) struct UdpSocket {
    fd: OwnedFd,
    /// Config snapshot (offloads applied in `new`).
    cfg: UdpConfig,
    rx_hdrs: UnsafeCell<Box<[libc::mmsghdr]>>,
    rx_iovs: UnsafeCell<Box<[libc::iovec]>>,
    rx_names: UnsafeCell<Box<[libc::sockaddr_storage]>>,
    tx_hdrs: UnsafeCell<Box<[libc::mmsghdr]>>,
    tx_iovs: UnsafeCell<Box<[libc::iovec]>>,
    tx_names: UnsafeCell<Box<[libc::sockaddr_storage]>>,
}

/// One receive slot: buffer + sender address + metadata.
pub(crate) struct RecvResult {
    pub len: usize,
    pub src: SocketAddr,
    /// True when MSG_TRUNC reported the datagram larger than the buffer.
    pub truncated: bool,
}

impl rustix::fd::AsFd for UdpSocket {
    fn as_fd(&self) -> rustix::fd::BorrowedFd<'_> {
        std::os::fd::AsFd::as_fd(&self.fd)
    }
}

/// A zeroed batch array of [`MAX_BATCH`] entries for the batch scratch
/// space. Zeroed raw pointers are valid values, and every field is
/// rewritten before each syscall, so the kernel never observes the
/// initial zeros.
fn zeroed_array<T>() -> Box<[T]> {
    (0..MAX_BATCH)
        .map(|_| unsafe { std::mem::zeroed() })
        .collect::<Vec<_>>()
        .into_boxed_slice()
}

/// `setsockopt` with a single `int` value.
pub(crate) fn set_int(fd: i32, level: libc::c_int, opt: libc::c_int, val: libc::c_int) -> io::Result<()> {
    // SAFETY: `val` is valid for the duration of the call; the kernel
    // copies the option value before `setsockopt` returns.
    let ret = unsafe {
        libc::setsockopt(
            fd,
            level,
            opt,
            (&val as *const libc::c_int).cast::<libc::c_void>(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Build a `sockaddr_in` from a std address (IPv4 only).
fn sockaddr_in_from(addr: SocketAddr) -> io::Result<libc::sockaddr_in> {
    let v4 = match addr {
        SocketAddr::V4(v4) => v4,
        SocketAddr::V6(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "UdpSocket is IPv4-only; IPv6 address rejected",
            ));
        }
    };
    // SAFETY: a zeroed `sockaddr_in` has no invalid bit patterns (only
    // integers and padding); every field is written below.
    let mut sa: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    sa.sin_family = libc::AF_INET as libc::sa_family_t;
    // `sin_port` and `sin_addr.s_addr` are stored in network byte order;
    // `to_be`/`from_ne_bytes` reproduce the on-wire layout.
    sa.sin_port = v4.port().to_be();
    sa.sin_addr.s_addr = u32::from_ne_bytes(v4.ip().octets());
    Ok(sa)
}

/// Read a `sockaddr_storage` filled by the kernel on an IPv4 socket as
/// a std address.
fn addr_from_storage(ss: &libc::sockaddr_storage) -> SocketAddr {
    match ss.ss_family as libc::c_int {
        libc::AF_INET => {
            // SAFETY: AF_INET guarantees the kernel wrote a `sockaddr_in`
            // at this address; both structs start with the family field
            // and `sockaddr_in` is a prefix of `sockaddr_storage`.
            let sin = unsafe { &*(ss as *const libc::sockaddr_storage).cast::<libc::sockaddr_in>() };
            // The kernel stores address and port in network byte order.
            SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)),
                u16::from_be(sin.sin_port),
            ))
        }
        _ => panic!("IPv4 socket reported a non-AF_INET source address"),
    }
}

impl UdpSocket {
    /// Bind a nonblocking UDP socket (IPv4) to `addr`, applying `cfg`.
    pub(crate) fn new(addr: SocketAddr, cfg: &UdpConfig) -> std::io::Result<Self> {
        // SAFETY: `socket` returns a fresh descriptor (or -1); the flags
        // are plain bitwise constants.
        let fd = unsafe {
            libc::socket(
                libc::AF_INET,
                libc::SOCK_DGRAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                0,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` is a fresh descriptor we own; `OwnedFd` takes
        // ownership and closes it on drop, including on error returns.
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };

        // Socket options BEFORE bind: the kernel admits a socket into a
        // SO_REUSEPORT group only when the option is set prior to bind
        // (man 7 socket), so every worker can bind the same address and
        // the kernel can distribute flows across them.
        set_int(owned.as_raw_fd(), libc::SOL_SOCKET, libc::SO_REUSEADDR, 1)?;
        if cfg.reuseport {
            set_int(owned.as_raw_fd(), libc::SOL_SOCKET, libc::SO_REUSEPORT, 1)?;
        }
        set_int(
            owned.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
            cfg.rcvbuf.min(i32::MAX as usize) as libc::c_int,
        )?;
        set_int(
            owned.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            cfg.sndbuf.min(i32::MAX as usize) as libc::c_int,
        )?;
        if cfg.gso_segment_size > 0 {
            set_int(
                owned.as_raw_fd(),
                libc::SOL_UDP,
                UDP_SEGMENT,
                cfg.gso_segment_size.min(i32::MAX as usize) as libc::c_int,
            )?;
        }
        if cfg.gro {
            set_int(owned.as_raw_fd(), libc::SOL_UDP, UDP_GRO, 1)?;
        }
        if cfg.incoming_cpu {
            // SO_INCOMING_CPU (49): steer packets to the socket's current
            // core; the value is the CPU number the thread runs on.
            let cpu = unsafe { libc::sched_getcpu() };
            if cpu >= 0 {
                set_int(owned.as_raw_fd(), libc::SOL_SOCKET, 49, cpu)?;
            }
        }
        if cfg.zerocopy {
            // SO_ZEROCOPY (60): allow MSG_ZEROCOPY sends (see
            // [`UdpSocket::send_to_zerocopy`]); sendmmsg has no flags
            // argument, so zerocopy sends use the sendmsg path.
            set_int(owned.as_raw_fd(), libc::SOL_SOCKET, 60, 1)?;
        }

        // Bind after the options so SO_REUSEPORT is already set (the
        // kernel requires it before bind for reuseport group admission).
        let sa = sockaddr_in_from(addr)?;
        // SAFETY: `sa` is fully initialized and `bind` copies it into the
        // kernel without retaining the pointer.
        let ret = unsafe {
            libc::bind(
                owned.as_raw_fd(),
                (&sa as *const libc::sockaddr_in).cast::<libc::sockaddr>(),
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            )
        };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(UdpSocket {
            fd: owned,
            cfg: cfg.clone(),
            rx_hdrs: UnsafeCell::new(zeroed_array()),
            rx_iovs: UnsafeCell::new(zeroed_array()),
            rx_names: UnsafeCell::new(zeroed_array()),
            tx_hdrs: UnsafeCell::new(zeroed_array()),
            tx_iovs: UnsafeCell::new(zeroed_array()),
            tx_names: UnsafeCell::new(zeroed_array()),
        })
    }

    /// Receive a batch of up to `bufs.len()` datagrams into the given
    /// preallocated buffers. Returns the number of datagrams received
    /// (0 = would block). Callers MUST drain until 0 (drain-to-EAGAIN).
    /// Generic over the buffer size so the engine can receive jumbo
    /// datagrams whole ([`MAX_DATAGRAM`]) while tests use small buffers
    /// to exercise MSG_TRUNC.
    pub(crate) fn recv_batch<const N: usize>(
        &self,
        bufs: &mut [mol::Buffer<N>],
        out: &mut [RecvResult],
    ) -> std::io::Result<usize> {
        let n = bufs.len().min(out.len()).min(MAX_BATCH);
        if n == 0 {
            return Ok(0);
        }
        // SAFETY: the scratch arrays are exclusively owned by this socket
        // and no batch call runs concurrently (one reactor thread per
        // socket); the derived references do not escape this function.
        let hdrs = unsafe { &mut *self.rx_hdrs.get() };
        let iovs = unsafe { &mut *self.rx_iovs.get() };
        let names = unsafe { &mut *self.rx_names.get() };
        for i in 0..n {
            // SAFETY: `names[i]` is a `sockaddr_storage` sized buffer the
            // kernel fills per datagram; the pointer stays valid for the
            // syscall duration.
            hdrs[i].msg_hdr.msg_name =
                (&mut names[i] as *mut libc::sockaddr_storage).cast::<libc::c_void>();
            hdrs[i].msg_hdr.msg_namelen =
                std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            // SAFETY: `iovs[i]` lives in the preallocated array for the
            // duration of the syscall.
            hdrs[i].msg_hdr.msg_iov = &mut iovs[i];
            hdrs[i].msg_hdr.msg_iovlen = 1;
            hdrs[i].msg_hdr.msg_control = std::ptr::null_mut();
            hdrs[i].msg_hdr.msg_controllen = 0;
            hdrs[i].msg_hdr.msg_flags = 0;
            hdrs[i].msg_len = 0;
            iovs[i].iov_base = bufs[i].as_mut_full_slice().as_mut_ptr().cast::<libc::c_void>();
            iovs[i].iov_len = bufs[i].capacity();
        }
        // SAFETY: `hdrs` points at `n` initialized `mmsghdr` entries with
        // matching iovecs and name buffers; the iovec targets are valid
        // for `N` bytes each (the full buffer capacity) for the duration
        // of the call.
        let ret = unsafe {
            libc::recvmmsg(
                self.fd.as_raw_fd(),
                hdrs.as_mut_ptr(),
                n as libc::c_uint,
                0,
                std::ptr::null_mut(),
            )
        };
        if ret < 0 {
            let err = io::Error::last_os_error();
            return if err.raw_os_error() == Some(libc::EAGAIN) {
                Ok(0)
            } else {
                Err(err)
            };
        }
        let count = ret as usize;
        for i in 0..count {
            let hdr = &hdrs[i];
            let cap = bufs[i].capacity();
            // With MSG_TRUNC the kernel reports the full datagram length
            // (larger than the iov); clamp to the buffer capacity.
            let len = (hdr.msg_len as usize).min(cap);
            // SAFETY: `len <= cap` (the buffer's full capacity), so the
            // length is publishable.
            bufs[i].set_len(len).expect("recv length within capacity");
            out[i] = RecvResult {
                len,
                src: addr_from_storage(&names[i]),
                truncated: hdr.msg_hdr.msg_flags & libc::MSG_TRUNC != 0,
            };
        }
        Ok(count)
    }

    /// Send one datagram (single datagram path).
    pub(crate) fn send_to(&self, data: &[u8], dst: SocketAddr) -> std::io::Result<usize> {
        let sa = sockaddr_in_from(dst)?;
        // SAFETY: `data` is a valid byte slice for the call duration; the
        // kernel copies the payload before `sendto` returns.
        let ret = unsafe {
            libc::sendto(
                self.fd.as_raw_fd(),
                data.as_ptr().cast::<libc::c_void>(),
                data.len(),
                0,
                (&sa as *const libc::sockaddr_in).cast::<libc::sockaddr>(),
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            )
        };
        if ret < 0 {
            let err = io::Error::last_os_error();
            return if err.raw_os_error() == Some(libc::EAGAIN) {
                Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "udp send_to: send buffer full",
                ))
            } else {
                Err(err)
            };
        }
        Ok(ret as usize)
    }

    /// Send one datagram with MSG_ZEROCOPY (kernel-zero-copy when the
    /// NIC supports it). Only valid when the socket was created with
    /// `cfg.zerocopy` (SO_ZEROCOPY set); requires CAP_NET_RAW. Returns
    /// `Err(Unsupported)` when the kernel/NIC cannot do zerocopy.
    pub(crate) fn send_to_zerocopy(&self, data: &[u8], dst: SocketAddr) -> std::io::Result<usize> {
        const MSG_ZEROCOPY: libc::c_int = 0x4000000;
        let sa = sockaddr_in_from(dst)?;
        let mut iov = libc::iovec {
            // SAFETY: the kernel treats the iovec target as read-only;
            // the mutable pointer is required by the struct layout.
            iov_base: data.as_ptr().cast_mut().cast::<libc::c_void>(),
            iov_len: data.len(),
        };
        let mut hdr: libc::msghdr = unsafe { std::mem::zeroed() };
        hdr.msg_name = (&sa as *const libc::sockaddr_in)
            .cast_mut()
            .cast::<libc::c_void>();
        hdr.msg_namelen = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
        hdr.msg_iov = &mut iov;
        hdr.msg_iovlen = 1;
        // SAFETY: `hdr` is fully initialized above and `data` stays valid
        // for the duration of the call (the kernel completes the copy
        // before `sendmsg` returns for non-zerocopy; for zerocopy the
        // caller must keep `data` alive until the completion queue
        // reports — documented at the call site).
        let ret = unsafe {
            libc::sendmsg(self.fd.as_raw_fd(), &hdr, MSG_ZEROCOPY)
        };
        if ret < 0 {
            let err = io::Error::last_os_error();
            return if err.raw_os_error() == Some(libc::EAGAIN) {
                Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "udp send_to_zerocopy: send buffer full",
                ))
            } else {
                Err(err)
            };
        }
        Ok(ret as usize)
    }

    /// Send a batch of datagrams (sendmmsg path). Returns the number
    /// sent (0 = would block on the first message).
    pub(crate) fn send_batch(&self, msgs: &[(&[u8], SocketAddr)]) -> std::io::Result<usize> {
        let n = msgs.len().min(MAX_BATCH);
        if n == 0 {
            return Ok(0);
        }
        // SAFETY: as in `recv_batch`, the send scratch arrays are
        // exclusively owned and never used concurrently.
        let hdrs = unsafe { &mut *self.tx_hdrs.get() };
        let iovs = unsafe { &mut *self.tx_iovs.get() };
        let names = unsafe { &mut *self.tx_names.get() };
        for i in 0..n {
            let (data, dst) = msgs[i];
            let sa = sockaddr_in_from(dst)?;
            // SAFETY: `names[i]` is `sockaddr_storage`-sized (larger than
            // a `sockaddr_in`); the copy writes only the prefix.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    &sa as *const libc::sockaddr_in,
                    (&mut names[i] as *mut libc::sockaddr_storage).cast::<libc::sockaddr_in>(),
                    1,
                );
            }
            let hdr = &mut hdrs[i];
            // SAFETY: `names[i]` holds a fully initialized `sockaddr_in`;
            // the kernel copies it before returning.
            hdr.msg_hdr.msg_name =
                (&mut names[i] as *mut libc::sockaddr_storage).cast::<libc::c_void>();
            hdr.msg_hdr.msg_namelen = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
            // SAFETY: `iovs[i]` lives in the preallocated array for the
            // duration of the syscall.
            hdr.msg_hdr.msg_iov = &mut iovs[i];
            hdr.msg_hdr.msg_iovlen = 1;
            hdr.msg_hdr.msg_control = std::ptr::null_mut();
            hdr.msg_hdr.msg_controllen = 0;
            hdr.msg_hdr.msg_flags = 0;
            hdr.msg_len = 0;
            // The kernel only reads `iov_base` on the send path; the
            // const->mut cast is sound because we never write through it.
            iovs[i].iov_base = (data.as_ptr() as *mut u8).cast::<libc::c_void>();
            iovs[i].iov_len = data.len();
        }
        // SAFETY: `hdrs` points at `n` initialized `mmsghdr` entries with
        // matching iovecs and sockaddrs, all valid for the call duration.
        let ret = unsafe {
            libc::sendmmsg(self.fd.as_raw_fd(), hdrs.as_mut_ptr(), n as libc::c_uint, 0)
        };
        if ret < 0 {
            let err = io::Error::last_os_error();
            return if err.raw_os_error() == Some(libc::EAGAIN) {
                Ok(0)
            } else {
                Err(err)
            };
        }
        Ok(ret as usize)
    }

    /// The local address.
    pub(crate) fn local_addr(&self) -> std::io::Result<SocketAddr> {
        // SAFETY: a zeroed `sockaddr_storage` is a valid destination;
        // the kernel fills it before `getsockname` returns.
        let mut ss: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        // SAFETY: `ss` is valid for `len` bytes; `getsockname` writes the
        // bound address and the actual length into `len`.
        let ret = unsafe {
            libc::getsockname(
                self.fd.as_raw_fd(),
                (&mut ss as *mut libc::sockaddr_storage).cast::<libc::sockaddr>(),
                &mut len,
            )
        };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(addr_from_storage(&ss))
    }

    /// The raw fd (for reactor registration).
    pub(crate) fn as_raw_fd(&self) -> i32 {
        self.fd.as_raw_fd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Loopback test config: SO_REUSEPORT off so two sockets never
    /// share an ephemeral port.
    fn test_cfg() -> UdpConfig {
        UdpConfig {
            reuseport: false,
            ..Default::default()
        }
    }

    fn bind() -> UdpSocket {
        UdpSocket::new("127.0.0.1:0".parse().unwrap(), &test_cfg()).unwrap()
    }

    fn recv_slot() -> RecvResult {
        RecvResult {
            len: 0,
            src: "0.0.0.0:0".parse().unwrap(),
            truncated: false,
        }
    }

    #[test]
    fn udp_loopback_roundtrip() {
        let a = bind();
        let b = bind();
        let baddr = b.local_addr().unwrap();
        let payload = b"hello fds udp";
        assert_eq!(a.send_to(payload, baddr).unwrap(), payload.len());

        let mut bufs: [mol::Buffer<2048>; 1] = std::array::from_fn(|_| mol::Buffer::new());
        let mut out: [RecvResult; 1] = std::array::from_fn(|_| recv_slot());
        let n = b.recv_batch(&mut bufs, &mut out).unwrap();
        assert_eq!(n, 1);
        assert_eq!(bufs[0].as_slice(), payload);
        assert_eq!(out[0].len, payload.len());
        assert_eq!(out[0].src, a.local_addr().unwrap());
        assert!(!out[0].truncated);
    }

    #[test]
    fn udp_batch_order() {
        let a = bind();
        let b = bind();
        let baddr = b.local_addr().unwrap();
        let payloads: Vec<Vec<u8>> = (0..10).map(|i| format!("datagram-{i}").into_bytes()).collect();
        for p in &payloads {
            a.send_to(p, baddr).unwrap();
        }

        let mut bufs: [mol::Buffer<2048>; 16] = std::array::from_fn(|_| mol::Buffer::new());
        let mut out: [RecvResult; 16] = std::array::from_fn(|_| recv_slot());
        let n = b.recv_batch(&mut bufs, &mut out).unwrap();
        assert_eq!(n, payloads.len());
        for (i, p) in payloads.iter().enumerate() {
            assert_eq!(bufs[i].as_slice(), p.as_slice());
            assert_eq!(out[i].len, p.len());
            assert!(!out[i].truncated);
            assert_eq!(out[i].src, a.local_addr().unwrap());
        }
    }

    #[test]
    fn udp_recv_batch_would_block() {
        let s = bind();
        let mut bufs: [mol::Buffer<2048>; 4] = std::array::from_fn(|_| mol::Buffer::new());
        let mut out: [RecvResult; 4] = std::array::from_fn(|_| recv_slot());
        assert_eq!(s.recv_batch(&mut bufs, &mut out).unwrap(), 0);
    }

    #[test]
    fn udp_oversized_truncated() {
        // A plain std sender: a datagram larger than our 2048-byte
        // buffers (loopback MTU ~65535 permits it).
        let sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let b = bind();
        let baddr = b.local_addr().unwrap();
        let payload = vec![0xABu8; 3000];
        assert_eq!(sender.send_to(&payload, baddr).unwrap(), payload.len());

        let mut bufs: [mol::Buffer<2048>; 1] = std::array::from_fn(|_| mol::Buffer::new());
        let mut out: [RecvResult; 1] = std::array::from_fn(|_| recv_slot());
        let n = b.recv_batch(&mut bufs, &mut out).unwrap();
        assert_eq!(n, 1);
        assert!(out[0].truncated);
        assert_eq!(out[0].len, 2048);
        assert_eq!(bufs[0].as_slice(), &payload[..2048]);
    }

    #[test]
    fn udp_gso_gro_flags() {
        let cfg = UdpConfig {
            reuseport: false,
            gso_segment_size: 2048,
            gro: true,
            ..Default::default()
        };
        let s = match UdpSocket::new("127.0.0.1:0".parse().unwrap(), &cfg) {
            Ok(s) => s,
            Err(e) => {
                // Offloads unsupported (e.g. pre-5.0 kernels): skip.
                eprintln!("skipping udp_gso_gro_flags: offload unsupported: {e}");
                return;
            }
        };
        // Payload is a multiple of the segment size, so GSO segmentation
        // is legal; loopback supports GSO.
        let payload = vec![0x5Au8; 8192];
        let dst = s.local_addr().unwrap();
        match s.send_to(&payload, dst) {
            Ok(sent) => assert_eq!(sent, payload.len()),
            Err(e) if matches!(e.raw_os_error(), Some(libc::EOPNOTSUPP) | Some(libc::ENOPROTOOPT)) => {
                eprintln!("skipping udp_gso_gro_flags: send unsupported: {e}");
            }
            Err(e) => panic!("GSO send failed: {e}"),
        }
    }
}
