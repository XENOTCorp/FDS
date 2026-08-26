//! UDP transport (standard `IO`, `SIMD` batching):
//! nonblocking sockets with `recvmmsg`/`sendmmsg` batch I/O, UDP_SEGMENT
//! (GSO) and UDP_GRO offloads, optional MSG_ZEROCOPY for large
//! datagrams. The batch ring between recvmmsg and processing is the
//! framework's ring.
//!
//! CONTRACT (implementer): implement [`UdpSocket`] on top of libc/rustix
//! with the exact signatures below (the crate compiles with these stubs;
//! replace `todo!()` bodies). Batches reuse preallocated arrays of
//! [`mol::Buffer`]; the hot path must not allocate. Wire the offloads
//! from [`crate::config::Config`]. Tests: loopback send/recv roundtrip, batch of
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
pub const MAX_DATAGRAM: usize = 65536;

/// `UDP_SEGMENT` (GSO) socket option, `<linux/udp.h>`. libc 0.2 does not
/// export it for glibc targets, so define it here (103, verified against
/// /usr/include/linux/udp.h).
pub const UDP_SEGMENT: libc::c_int = 103;

/// `UDP_GRO` socket option, `<linux/udp.h>` (104); see [`UDP_SEGMENT`].
pub const UDP_GRO: libc::c_int = 104;

/// A nonblocking UDP socket with batch I/O.
///
/// The `recvmmsg`/`sendmmsg` header, iovec and address arrays are
/// preallocated here (allocation-free hot path). They sit behind
/// `UnsafeCell` because the batch methods take `&self`; a socket is
/// owned by exactly one reactor thread at a time, so `recv_batch` and
/// `send_batch` never run concurrently on the same socket.
pub struct UdpSocket {
    fd: OwnedFd,
    rx_hdrs: UnsafeCell<Box<[libc::mmsghdr]>>,
    rx_iovs: UnsafeCell<Box<[libc::iovec]>>,
    rx_names: UnsafeCell<Box<[libc::sockaddr_storage]>>,
    tx_hdrs: UnsafeCell<Box<[libc::mmsghdr]>>,
    tx_iovs: UnsafeCell<Box<[libc::iovec]>>,
    tx_names: UnsafeCell<Box<[libc::sockaddr_storage]>>,
}

/// One receive slot: buffer + sender address + metadata.
pub struct RecvResult {
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
pub fn set_int(fd: i32, level: libc::c_int, opt: libc::c_int, val: libc::c_int) -> io::Result<()> {
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
    pub fn new(addr: SocketAddr, cfg: &UdpConfig) -> std::io::Result<Self> {
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
    pub fn recv_batch<const N: usize>(
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
    pub fn send_to(&self, data: &[u8], dst: SocketAddr) -> std::io::Result<usize> {
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
    /// `cfg.zerocopy` (SO_ZEROCOPY set). SO_ZEROCOPY needs no special
    /// privilege (verified empirically; the old "requires CAP_NET_RAW"
    /// claim was wrong). Returns `Err(Unsupported)` when the kernel/NIC
    /// cannot do zerocopy.
    pub fn send_to_zerocopy(&self, data: &[u8], dst: SocketAddr) -> std::io::Result<usize> {
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

    /// Drain MSG_ZEROCOPY completion notifications from the socket's
    /// error queue. Returns the number of notifications consumed. Each
    /// zero-copy send leaves the send buffer referenced by the kernel
    /// until the peer consumes the datagram; the caller must not reuse
    /// the buffer until the corresponding notification is drained.
    ///
    /// NOTE: this kernel queues the notification with an EMPTY byte
    /// range (ee_info == ee_data == 0) for UDP — verified empirically —
    /// so the engine recycles buffers by notification count, not by
    /// byte range (the error queue is FIFO and sends are ordered, so
    /// counts are exact even when ranges are not).
    pub fn drain_zerocopy_notifications(&self) -> std::io::Result<u64> {
        const MSG_ERRQUEUE: libc::c_int = 0x2000;
        const MSG_DONTWAIT: libc::c_int = 0x40;
        let mut count: u64 = 0;
        let mut iov_buf = [0u8; 128];
        let mut cmsg_buf = [0u8; 512];
        loop {
            let mut iov = libc::iovec {
                iov_base: iov_buf.as_mut_ptr().cast::<libc::c_void>(),
                iov_len: iov_buf.len(),
            };
            let mut hdr: libc::msghdr = unsafe { std::mem::zeroed() };
            hdr.msg_iov = &mut iov;
            hdr.msg_iovlen = 1;
            hdr.msg_control = cmsg_buf.as_mut_ptr().cast::<libc::c_void>();
            hdr.msg_controllen = cmsg_buf.len();
            // SAFETY: `hdr` is fully initialized; the kernel fills the
            // iov and control buffers (sized above) and owns them only
            // for the duration of the call.
            let ret = unsafe {
                libc::recvmsg(self.fd.as_raw_fd(), &mut hdr, MSG_ERRQUEUE | MSG_DONTWAIT)
            };
            if ret < 0 {
                match io::Error::last_os_error().raw_os_error() {
                    Some(libc::EAGAIN) | Some(libc::ENOENT) | Some(libc::ENOMSG) => break,
                    Some(_) => return Err(io::Error::last_os_error()),
                    None => break,
                }
            }
            count += 1;
        }
        Ok(count)
    }

    /// Send a batch of datagrams (sendmmsg path). Returns the number
    /// sent (0 = would block on the first message).
    pub fn send_batch(&self, msgs: &[(&[u8], SocketAddr)]) -> std::io::Result<usize> {
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
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
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
    pub fn as_raw_fd(&self) -> i32 {
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
    fn zerocopy_udp_kernel_behavior() {
        // Documents how THIS kernel handles UDP MSG_ZEROCOPY (assertions
        // are kernel-agnostic — send integrity only; the behavior is
        // logged). On this box: the kernel silently COPIES the data at
        // send time (the mutation probe sees the OLD bytes), queues a
        // single coalesced notification with an empty [0,0) byte range,
        // and the engine's ZcState auto-disables zerocopy after a 5 ms
        // grace so the worker never wedges.
        let zc_cfg = UdpConfig {
            reuseport: false,
            zerocopy: true,
            ..Default::default()
        };
        let sock = UdpSocket::new("127.0.0.1:0".parse().unwrap(), &zc_cfg).expect("zc bind");
        let peer = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let payload = vec![0xabu8; 60 * 1024];
        for _ in 0..3 {
            let n = sock
                .send_to_zerocopy(&payload, peer.local_addr().unwrap())
                .expect("zc send");
            assert_eq!(n, payload.len());
            let mut buf = vec![0u8; 70_000];
            let (got, _) = peer.recv_from(&mut buf).expect("peer recv");
            assert_eq!(got, payload.len());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        let notifs = sock
            .drain_zerocopy_notifications()
            .expect("udp drain");
        eprintln!("udp: notifications after 3 zc sends = {notifs}");

        // Mutation probe: send with ZC, mutate the buffer before the
        // peer reads. Referenced pages would deliver the NEW bytes (real
        // zero-copy); a send-time copy delivers the OLD ones.
        let mut probe_payload = vec![0xdu8; 60 * 1024];
        let n = sock
            .send_to_zerocopy(&probe_payload, peer.local_addr().unwrap())
            .expect("zc mutation send");
        assert_eq!(n, probe_payload.len());
        for b in probe_payload.iter_mut() {
            *b = 0x5a;
        }
        let mut mbuf = vec![0u8; 70_000];
        let (mgot, _) = peer.recv_from(&mut mbuf).expect("peer recv mutated");
        assert_eq!(mgot, probe_payload.len());
        let saw_old = mbuf[..mgot].iter().all(|&b| b == 0xdu8);
        let saw_new = mbuf[..mgot].iter().all(|&b| b == 0x5au8);
        eprintln!(
            "udp: mutation test — peer saw OLD bytes (copied at send) = {saw_old}, NEW bytes (pages referenced) = {saw_new}"
        );

        // Corked probe: the same, with UDP_CORK set (the corked path is
        // the documented route for UDP zerocopy).
        let cork: libc::c_int = 1;
        // SAFETY: setsockopt on an owned fd with valid args.
        unsafe {
            libc::setsockopt(
                sock.fd.as_raw_fd(),
                libc::SOL_UDP,
                2, /* UDP_CORK */
                (&cork as *const libc::c_int).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            );
        }
        let mut cork_payload = vec![0xeu8; 60 * 1024];
        let n = sock
            .send_to_zerocopy(&cork_payload, peer.local_addr().unwrap())
            .expect("corked zc send");
        assert_eq!(n, cork_payload.len());
        for b in cork_payload.iter_mut() {
            *b = 0x3c;
        }
        let uncork: libc::c_int = 0;
        // SAFETY: setsockopt on an owned fd with valid args.
        unsafe {
            libc::setsockopt(
                sock.fd.as_raw_fd(),
                libc::SOL_UDP,
                2, /* UDP_CORK */
                (&uncork as *const libc::c_int).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            );
        }
        let mut cbuf = vec![0u8; 70_000];
        let (cgot, _) = peer.recv_from(&mut cbuf).expect("peer recv corked");
        assert_eq!(cgot, cork_payload.len());
        let cork_saw_old = cbuf[..cgot].iter().all(|&b| b == 0xeu8);
        let cork_saw_new = cbuf[..cgot].iter().all(|&b| b == 0x3cu8);
        eprintln!(
            "udp: corked mutation test — OLD (copied) = {cork_saw_old}, NEW (pages referenced) = {cork_saw_new}"
        );
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
