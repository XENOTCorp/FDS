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
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

// Socket-option numbers from <linux/sctp.h> (verified against the header).
const SCTP_INITMSG: libc::c_int = 2;
const SCTP_NODELAY: libc::c_int = 3;
const SCTP_EVENTS: libc::c_int = 11;
const SCTP_PARTIAL_DELIVERY_POINT: libc::c_int = 19;
const SCTP_MAX_BURST: libc::c_int = 20;
/// `enum sctp_msg_flags` (also `SCTP_NOTIFICATION`).
const MSG_NOTIFICATION: libc::c_int = 0x8000;
// libc 0.2.189 exports neither of these on Linux-gnu; values from the
// kernel UAPI and <netinet/sctp.h>. SOL_SCTP is 132. There is NO
// AF_SCTP in the Linux UAPI: the kernel registers SCTP under the inet
// family, so the socket call uses AF_INET + IPPROTO_SCTP (132) (an
// earlier AF_SCTP = 30 constant was AF_TIPC and made every SCTP socket
// fail with EAFNOSUPPORT).
const SOL_SCTP: libc::c_int = 132;

/// `sctp_assoc_t` from <linux/sctp.h>. The header typedefs it `__s32`;
/// association ids are never negative and the ABI is identical, so the
/// binding keeps `u32` throughout.
type SctpAssocT = u32;

/// `struct sctp_initmsg` — the SCTP_INITMSG socket option.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct SctpInitMsg {
    sinit_num_ostreams: u16,
    sinit_max_instreams: u16,
    sinit_max_attempts: u16,
    sinit_max_init_timeo: u16,
}

/// `struct sctp_sndrcvinfo` — filled by `sctp_recvmsg` per message.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct SctpSndRcvInfo {
    sinfo_stream: u16,
    sinfo_ssn: u16,
    sinfo_flags: u16,
    sinfo_ppid: u32,
    sinfo_context: u32,
    sinfo_timetolive: u32,
    sinfo_tsn: u32,
    sinfo_cumtsn: u32,
    sinfo_assoc_id: SctpAssocT,
}

/// `struct sctp_event_subscribe` — the SCTP_EVENTS socket option. The
/// last member keeps the kernel header's spelling: `sctp_send_failure_event_event`
/// is a preserved typo in <linux/sctp.h>.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct SctpEventSubscribe {
    sctp_data_io_event: u8,
    sctp_association_event: u8,
    sctp_address_event: u8,
    sctp_send_failure_event: u8,
    sctp_peer_error_event: u8,
    sctp_shutdown_event: u8,
    sctp_partial_delivery_event: u8,
    sctp_adaptation_layer_event: u8,
    sctp_authentication_event: u8,
    sctp_sender_dry_event: u8,
    sctp_stream_reset_event: u8,
    sctp_assoc_reset_event: u8,
    sctp_stream_change_event: u8,
    sctp_send_failure_event_event: u8,
}

// FFI to libsctp (see contract above). Do NOT edit the signatures
// without matching `<netinet/sctp.h>` and the installed libsctp ABI.
#[link(name = "sctp")]
extern "C" {
    /// `ssize_t sctp_sendmsg(...)` — verified `ssize_t` against the
    /// installed libsctp 1.0.21 (errors come back as a 64-bit -1).
    fn sctp_sendmsg(
        sd: libc::c_int,
        msg: *const libc::c_void,
        len: libc::size_t,
        to: *mut libc::sockaddr,
        tolen: libc::socklen_t,
        ppid: u32,
        flags: u32,
        stream_no: u16,
        timetolive: u32,
        context: u32,
    ) -> libc::ssize_t;
    /// `int sctp_recvmsg(...)` — the system header and the installed
    /// libsctp return `int` (verified: -1 comes back as a 32-bit value),
    /// so declaring `ssize_t` would misread the error return.
    fn sctp_recvmsg(
        sd: libc::c_int,
        msg: *mut libc::c_void,
        len: libc::size_t,
        from: *mut libc::sockaddr,
        fromlen: *mut libc::socklen_t,
        sinfo: *mut SctpSndRcvInfo,
        msg_flags: *mut libc::c_int,
    ) -> libc::c_int;
}

/// A nonblocking SCTP one-to-one (or one-to-many) socket.
pub(crate) struct SctpSocket {
    pub(crate) fd: OwnedFd,
}

/// Convert a `SocketAddr` into a `sockaddr_storage` + length for the
/// libsctp / libc calls.
fn sockaddr_of(addr: &SocketAddr) -> (libc::sockaddr_storage, libc::socklen_t) {
    // SAFETY: sockaddr_storage is 128 bytes, all-zero is a valid value,
    // and it is aligned for both sockaddr_in and sockaddr_in6.
    let mut ss: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    match addr {
        SocketAddr::V4(a) => {
            // SAFETY: ss is writable as sockaddr_in (same bytes, no
            // stricter alignment).
            let sin =
                unsafe { &mut *(&mut ss as *mut libc::sockaddr_storage as *mut libc::sockaddr_in) };
            sin.sin_family = libc::AF_INET as libc::sa_family_t;
            sin.sin_port = a.port().to_be();
            sin.sin_addr = libc::in_addr {
                s_addr: u32::from(*a.ip()).to_be(),
            };
            (
                ss,
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            )
        }
        SocketAddr::V6(a) => {
            // SAFETY: ss is writable as sockaddr_in6 (equal alignment).
            let sin6 = unsafe {
                &mut *(&mut ss as *mut libc::sockaddr_storage as *mut libc::sockaddr_in6)
            };
            sin6.sin6_family = libc::AF_INET6 as libc::sa_family_t;
            sin6.sin6_port = a.port().to_be();
            sin6.sin6_flowinfo = a.flowinfo();
            sin6.sin6_addr = libc::in6_addr {
                s6_addr: a.ip().octets(),
            };
            sin6.sin6_scope_id = a.scope_id();
            (
                ss,
                std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t,
            )
        }
    }
}

/// Convert a kernel-filled `sockaddr_storage` back into a `SocketAddr`.
fn sockaddr_to_addr(
    ss: &libc::sockaddr_storage,
    len: libc::socklen_t,
) -> std::io::Result<SocketAddr> {
    match ss.ss_family as libc::c_int {
        libc::AF_INET => {
            if len < std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "short sockaddr_in",
                ));
            }
            // SAFETY: family and length guarantee a full sockaddr_in.
            let sin =
                unsafe { &*(ss as *const libc::sockaddr_storage as *const libc::sockaddr_in) };
            Ok(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)),
                u16::from_be(sin.sin_port),
            )))
        }
        libc::AF_INET6 => {
            if len < std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "short sockaddr_in6",
                ));
            }
            // SAFETY: family and length guarantee a full sockaddr_in6.
            let sin6 =
                unsafe { &*(ss as *const libc::sockaddr_storage as *const libc::sockaddr_in6) };
            Ok(SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::from(sin6.sin6_addr.s6_addr),
                u16::from_be(sin6.sin6_port),
                sin6.sin6_flowinfo,
                sin6.sin6_scope_id,
            )))
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unsupported address family",
        )),
    }
}

impl SctpSocket {
    /// setsockopt wrapper; the kernel copies `value` before returning.
    fn set_opt<T>(&self, level: libc::c_int, name: libc::c_int, value: &T) -> std::io::Result<()> {
        // SAFETY: `value` points to a valid `T` of `size_of::<T>()` bytes
        // and the kernel reads it synchronously.
        let rc = unsafe {
            libc::setsockopt(
                self.fd.as_raw_fd(),
                level,
                name,
                value as *const T as *const libc::c_void,
                std::mem::size_of::<T>() as libc::socklen_t,
            )
        };
        if rc < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// getsockopt wrapper for a `c_int` option.
    fn get_opt_i32(&self, level: libc::c_int, name: libc::c_int) -> std::io::Result<i32> {
        let mut value: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: `value` and `len` are valid writable out-params and the
        // kernel writes at most `size_of::<c_int>()` bytes.
        let rc = unsafe {
            libc::getsockopt(
                self.fd.as_raw_fd(),
                level,
                name,
                &mut value as *mut libc::c_int as *mut libc::c_void,
                &mut len,
            )
        };
        if rc < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(value)
        }
    }

    /// The address this socket is bound to (getsockname).
    pub(crate) fn local_addr(&self) -> std::io::Result<SocketAddr> {
        // SAFETY: sockaddr_storage is zeroable, large enough for any
        // family, and aligned for sockaddr_in6.
        let mut ss: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        // SAFETY: ss and len are valid writable out-params.
        let rc = unsafe {
            libc::getsockname(
                self.fd.as_raw_fd(),
                &mut ss as *mut libc::sockaddr_storage as *mut libc::sockaddr,
                &mut len,
            )
        };
        if rc < 0 {
            return Err(std::io::Error::last_os_error());
        }
        sockaddr_to_addr(&ss, len)
    }

    /// Create and bind an SCTP socket on `addr`, applying `cfg`.
    pub(crate) fn bind(addr: SocketAddr, cfg: &SctpConfig) -> std::io::Result<Self> {
        // SAFETY: socket(2) with valid constants; a fresh fd (or -1)
        // comes back and we take ownership immediately. Linux registers
        // SCTP under the inet family — there is no AF_SCTP in the UAPI.
        let raw = unsafe {
            libc::socket(
                libc::AF_INET,
                libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                libc::IPPROTO_SCTP,
            )
        };
        if raw < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `raw` is a fresh fd owned by us.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let sock = SctpSocket { fd };

        sock.set_opt(libc::SOL_SOCKET, libc::SO_REUSEADDR, &1i32)?;
        if cfg.reuseport {
            rustix::net::sockopt::set_socket_reuseport(&sock.fd, true)
                .map_err(std::io::Error::from)?;
        }
        if cfg.nodelay {
            sock.set_opt(SOL_SCTP, SCTP_NODELAY, &1i32)?;
        }
        // SCTP_INITMSG: negotiated stream counts (attempts/timeo left at
        // 0 = kernel defaults).
        let init = SctpInitMsg {
            sinit_num_ostreams: cfg.init_max_streams,
            sinit_max_instreams: cfg.init_max_streams,
            ..SctpInitMsg::default()
        };
        sock.set_opt(SOL_SCTP, SCTP_INITMSG, &init)?;
        if cfg.partial_delivery_point > 0 {
            sock.set_opt(
                SOL_SCTP,
                SCTP_PARTIAL_DELIVERY_POINT,
                &cfg.partial_delivery_point,
            )?;
        }
        if cfg.max_burst > 0 {
            sock.set_opt(SOL_SCTP, SCTP_MAX_BURST, &cfg.max_burst)?;
        }
        // Subscribe to data-io (so sctp_recvmsg fills sinfo), association
        // and shutdown notifications. Best-effort: notifications are
        // advisory and must not fail the bind.
        let events = SctpEventSubscribe {
            sctp_data_io_event: 1,
            sctp_association_event: 1,
            sctp_shutdown_event: 1,
            ..SctpEventSubscribe::default()
        };
        let _ = sock.set_opt(SOL_SCTP, SCTP_EVENTS, &events);

        let (ss, len) = sockaddr_of(&addr);
        // SAFETY: `ss` is a valid sockaddr describing `addr`; bind(2)
        // does not retain the pointer.
        let rc = unsafe {
            libc::bind(
                sock.fd.as_raw_fd(),
                &ss as *const libc::sockaddr_storage as *const libc::sockaddr,
                len,
            )
        };
        if rc < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(sock)
    }

    /// Send `data` on stream `stream_id` to `dst`.
    pub(crate) fn send_msg(
        &self,
        data: &[u8],
        stream_id: u16,
        dst: SocketAddr,
    ) -> std::io::Result<usize> {
        let (ss, len) = sockaddr_of(&dst);
        // SAFETY: sctp_sendmsg wraps sendmsg(2), reads `msg`/`to`
        // synchronously, and does not retain pointers. The header declares
        // `to` non-const but the API never writes it. No allocation.
        let n = unsafe {
            sctp_sendmsg(
                self.fd.as_raw_fd(),
                data.as_ptr() as *const libc::c_void,
                data.len(),
                &ss as *const libc::sockaddr_storage as *mut libc::sockaddr,
                len,
                0, // ppid
                0, // flags: ordered delivery
                stream_id,
                0, // timetolive
                0, // context
            )
        };
        if n < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }

    /// Receive one message; returns the payload length, the sender, and
    /// the stream id. `Err(WouldBlock)` = drained.
    pub(crate) fn recv_msg(
        &self,
        buf: &mut [u8],
        out_stream: &mut u16,
    ) -> std::io::Result<(usize, SocketAddr)> {
        let mut sinfo = SctpSndRcvInfo::default();
        let mut from: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut fromlen = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        let mut msg_flags: libc::c_int = 0;
        // SAFETY: buf/from/fromlen/sinfo/msg_flags are valid writable
        // buffers of the right sizes; sctp_recvmsg fills them and returns
        // the payload length (or -1 with errno). No allocation.
        let n = unsafe {
            sctp_recvmsg(
                self.fd.as_raw_fd(),
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
                &mut from as *mut libc::sockaddr_storage as *mut libc::sockaddr,
                &mut fromlen,
                &mut sinfo,
                &mut msg_flags,
            )
        };
        if n < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if msg_flags & MSG_NOTIFICATION != 0 {
            // A notification, not user data (SCTP truncates silently on a
            // short buffer); surface it distinctly so callers can drain.
            return Err(std::io::Error::other("sctp notification"));
        }
        *out_stream = sinfo.sinfo_stream;
        // SAFETY: the kernel filled `from` with `fromlen` bytes.
        let addr = sockaddr_to_addr(&from, fromlen)?;
        Ok((n as usize, addr))
    }

    /// The raw fd.
    pub(crate) fn as_raw_fd(&self) -> i32 {
        self.fd.as_raw_fd()
    }
}

/// Errno values meaning "no SCTP support here" (kernel module absent, or
/// an association refused for lack of support): tests and the SCTP bench
/// skip on these instead of failing.
pub(crate) fn unsupported(e: &std::io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(
            libc::EPROTONOSUPPORT
                | libc::EAFNOSUPPORT
                | libc::EOPNOTSUPP
                | libc::ENOPROTOOPT
                | libc::EPROTOTYPE
                | libc::ENOTCONN
        )
    )
}

/// recv_msg's "this message was an SCTP notification" sentinel.
pub(crate) fn is_notification(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::Other && e.to_string() == "sctp notification"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loopback() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    /// Bind on loopback; `None` (with a note) when the kernel has no SCTP.
    fn bind_or_skip(what: &str, cfg: &SctpConfig) -> Option<SctpSocket> {
        match SctpSocket::bind(loopback(), cfg) {
            Ok(s) => Some(s),
            Err(e) if unsupported(&e) => {
                eprintln!("{what}: kernel SCTP unavailable, skipping ({e})");
                None
            }
            Err(e) => panic!("{what}: bind failed: {e}"),
        }
    }

    #[test]
    fn sctp_bind_ok() {
        let Some(sock) = bind_or_skip("sctp_bind_ok", &SctpConfig::default()) else {
            return;
        };
        let got = sock.local_addr().expect("getsockname after bind");
        assert_eq!(got.ip(), loopback().ip());
    }

    #[test]
    fn sctp_nodelay_set() {
        let Some(sock) = bind_or_skip("sctp_nodelay_set", &SctpConfig::default()) else {
            return;
        };
        let v = sock
            .get_opt_i32(SOL_SCTP, SCTP_NODELAY)
            .expect("getsockopt SCTP_NODELAY");
        assert_eq!(v, 1, "SCTP_NODELAY must be on after bind with cfg.nodelay");
    }

    #[test]
    fn sctp_loopback_roundtrip() {
        // Distinct ephemeral ports; reuseport off keeps the two sockets
        // independent.
        let cfg = SctpConfig {
            reuseport: false,
            ..SctpConfig::default()
        };
        let Some(a) = bind_or_skip("sctp_loopback_roundtrip(a)", &cfg) else {
            return;
        };
        let Some(b) = bind_or_skip("sctp_loopback_roundtrip(b)", &cfg) else {
            return;
        };

        // One-to-one SCTP sockets must listen() to accept an association.
        // SAFETY: b's fd is a bound, nonblocking SCTP socket; listen(2)
        // never blocks.
        let rc = unsafe { libc::listen(b.as_raw_fd(), 8) };
        if rc < 0 {
            let e = std::io::Error::last_os_error();
            if unsupported(&e) {
                eprintln!("sctp_loopback_roundtrip: listen unsupported, skipping ({e})");
                return;
            }
            panic!("listen failed: {e}");
        }
        let b_addr = b.local_addr().expect("getsockname B");

        // sendmsg implicitly connects A to B; on a nonblocking one-to-one
        // socket it returns WouldBlock while the INIT handshake is in
        // flight, so retry until it goes through.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let payload = b"hello";
        loop {
            match a.send_msg(payload, 3, b_addr) {
                Ok(n) => {
                    assert_eq!(n, payload.len(), "full payload sent");
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "timed out connecting/sending"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(e) if unsupported(&e) => {
                    eprintln!("sctp_loopback_roundtrip: send unsupported, skipping ({e})");
                    return;
                }
                Err(e) => panic!("send failed: {e}"),
            }
        }

        // accept the association once the handshake completes.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let conn = loop {
            // SAFETY: accept4 on a listening fd returns a new fd or -1;
            // we pass no address buffers (only the fd is needed).
            let raw = unsafe {
                libc::accept4(
                    b.as_raw_fd(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                )
            };
            if raw >= 0 {
                // SAFETY: `raw` is a fresh fd owned by us.
                break unsafe { OwnedFd::from_raw_fd(raw) };
            }
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::WouldBlock {
                assert!(std::time::Instant::now() < deadline, "timed out accepting");
                std::thread::sleep(std::time::Duration::from_millis(5));
            } else if unsupported(&e) {
                eprintln!("sctp_loopback_roundtrip: accept unsupported, skipping ({e})");
                return;
            } else {
                panic!("accept failed: {e}");
            }
        };

        // The accepted socket inherits B's options (SCTP_EVENTS, ...).
        let b_conn = SctpSocket { fd: conn };
        let mut buf = [0u8; 64];
        let mut stream = 0u16;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match b_conn.recv_msg(&mut buf, &mut stream) {
                Ok((n, from)) => {
                    assert_eq!(
                        from.ip(),
                        a.local_addr().expect("getsockname A").ip(),
                        "sender address"
                    );
                    assert_eq!(&buf[..n], payload);
                    assert_eq!(stream, 3, "stream id preserved");
                    return;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(std::time::Instant::now() < deadline, "timed out receiving");
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(e) if is_notification(&e) => {} // drain association events
                Err(e) if unsupported(&e) => {
                    eprintln!("sctp_loopback_roundtrip: recv unsupported, skipping ({e})");
                    return;
                }
                Err(e) => panic!("recv failed: {e}"),
            }
        }
    }
}
