use fds::{
    api::{Driver, EpollDriver, Interest},
    config::{Config, TcpConfig},
    metrics::{Metrics, MetricsServer},
    reactor::Reactor,
    tcp,
};
use std::{
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::net::{UnixListener, UnixStream},
    },
    time::{Duration, Instant},
};

fn tcp_pair(cfg: &TcpConfig) -> (tcp::TcpStream, std::net::TcpStream, tcp::TcpListener) {
    let listener = tcp::TcpListener::bind(([127, 0, 0, 1], 0).into(), cfg, 8).unwrap();
    let peer = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some((stream, _)) = listener.accept().unwrap() {
            return (stream, peer, listener);
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
}

#[test]
fn vectored_io_returns_one_contiguous_prefix() {
    let (mut stream, mut peer, _listener) = tcp_pair(&TcpConfig::default());
    peer.write_all(b"abcdefghijklmnopq").unwrap();
    let mut driver = EpollDriver::new(4).unwrap();
    driver
        .register(stream.as_raw_fd(), 1, Interest::Readable)
        .unwrap();
    assert_eq!(driver.poll(Some(Duration::from_secs(2))).unwrap(), 1);
    let mut storage = [[0u8; 1]; 17];
    let mut buffers: Vec<&mut [u8]> = storage.iter_mut().map(|b| &mut b[..]).collect();
    assert_eq!(stream.readv(&mut buffers).unwrap(), 16);
    assert_eq!(storage[..16].concat(), b"abcdefghijklmnop");
    assert_eq!(storage[16], [0]);
    assert_eq!(stream.writev(&[&b"x"[..]; 17]).unwrap(), 16);
    let mut received = [0; 16];
    peer.read_exact(&mut received).unwrap();
    assert_eq!(received, [b'x'; 16]);
}

#[test]
fn udp_without_reuseport_binds_exclusively() {
    let cfg = fds::config::UdpConfig {
        reuseport: false,
        ..Default::default()
    };
    let first = fds::udp::UdpSocket::new(([127, 0, 0, 1], 0).into(), &cfg).unwrap();
    let result = fds::udp::UdpSocket::new(first.local_addr().unwrap(), &cfg);
    assert!(matches!(result, Err(ref error) if error.kind() == std::io::ErrorKind::AddrInUse));
}

#[test]
fn defer_accept_is_set_on_listener() {
    let cfg = TcpConfig {
        defer_accept: true,
        ..Default::default()
    };
    let listener = tcp::TcpListener::bind(([127, 0, 0, 1], 0).into(), &cfg, 8).unwrap();
    let mut value: libc::c_int = 0;
    let mut len = std::mem::size_of_val(&value) as libc::socklen_t;
    // SAFETY: both output pointers are initialized and writable.
    assert_eq!(
        unsafe {
            libc::getsockopt(
                listener.as_raw_fd(),
                libc::IPPROTO_TCP,
                libc::TCP_DEFER_ACCEPT,
                (&mut value as *mut libc::c_int).cast(),
                &mut len,
            )
        },
        0
    );
    assert!(value > 0);
}

#[test]
fn reactor_rejects_bad_fds_and_does_not_expose_stale_events() {
    let mut reactor = Reactor::new(8).unwrap();
    assert!(reactor.register(-1, 0, Interest::Readable).is_err());
    assert_eq!(reactor.delivered(usize::MAX).count(), 0);
    let (receiver, mut sender) = UnixStream::pair().unwrap();
    reactor
        .register(receiver.as_raw_fd(), 42, Interest::Readable)
        .unwrap();
    sender.write_all(b"x").unwrap();
    assert_eq!(reactor.poll_busy().unwrap(), 1);
    assert_eq!(reactor.delivered(usize::MAX).count(), 1);
    assert_eq!(reactor.delivered(1).next().unwrap().token, 42);
    assert_eq!(reactor.poll_once().unwrap(), 0);
    assert_eq!(reactor.delivered(usize::MAX).count(), 0);
}

#[test]
fn configuration_rejects_typos_and_invalid_limits() {
    assert!(Config::from_json(r#"{"core":{"threadz":2}}"#).is_err());
    assert!(Config::from_json(r#"{"unknown":{}}"#).is_err());
    for document in [
        r#"{"engine":{"tcp_bind":"not an address"}}"#,
        r#"{"af_xdp":{"ring_size":3}}"#,
        r#"{"af_xdp":{"ring_size":256,"num_frames":1}}"#,
        r#"{"core":{"stack_bytes":1}}"#,
        r#"{"reactor":{"max_events":0}}"#,
    ] {
        assert!(
            Config::from_json(document).unwrap().validate().is_err(),
            "{document}"
        );
    }
    Config::default().validate().unwrap();
}

#[test]
fn metrics_does_not_steal_or_unlink_replacement_endpoints() {
    let path = std::env::temp_dir().join(format!("fds-review-metrics-{}.sock", std::process::id()));
    let mut original = MetricsServer::bind(&path).unwrap();
    assert!(MetricsServer::bind(&path).is_err());
    let mut client = UnixStream::connect(&path).unwrap();
    original.poll_once(&Metrics::new(1)).unwrap();
    let mut report = String::new();
    client.read_to_string(&mut report).unwrap();
    assert!(report.contains("packets.total"));
    std::fs::remove_file(&path).unwrap();
    let replacement = UnixListener::bind(&path).unwrap();
    drop(original);
    assert!(path.exists(), "dropping old server removed replacement");
    drop(replacement);
    std::fs::remove_file(&path).unwrap();
}

/// Opt-in live ring/UMEM setup on an isolated virtual device. No XDP
/// program is installed here: this checks setup and TX-pool ownership,
/// not received traffic or NIC zero-copy performance.
#[cfg(feature = "af-xdp")]
#[test]
fn af_xdp_virtual_device_lifecycle() {
    let Ok(device) = std::env::var("FDS_TEST_XDP_DEVICE") else {
        eprintln!(
            "skipping live AF_XDP setup: set FDS_TEST_XDP_DEVICE in an isolated network namespace"
        );
        return;
    };
    let device = std::ffi::CString::new(device).unwrap();
    // SAFETY: device is a live NUL-terminated interface name.
    let index = unsafe { libc::if_nametoindex(device.as_ptr()) };
    assert_ne!(index, 0, "test device is missing");
    let opts = fds::af_xdp::XskOpenOpts {
        ring_size: 32,
        num_frames: 64,
        zero_copy: false,
        node: None,
    };
    let mut socket = fds::af_xdp::XskSocket::open_with(index as i32, 0, opts)
        .expect("live AF_XDP copy-mode setup");
    assert_eq!(socket.mode(), fds::af_xdp::XdpMode::Copy);
    assert!(socket.recv_frame().is_none());
    assert_eq!(socket.tx_pending(), 0);
    for _ in 0..128 {
        let frame = socket.alloc_tx(256).unwrap();
        socket.frame_mut(&frame).fill(0x5a);
        assert!(socket.frame_mut(&frame).iter().all(|&byte| byte == 0x5a));
        socket.drop_frame(frame);
    }
}

#[cfg(feature = "io-uring")]
#[test]
fn uring_timeout_flags_and_token_reuse() {
    use fds::api::IoUringDriver;
    let mut driver = IoUringDriver::new(16).unwrap();
    let (old_receiver, mut old_sender) = UnixStream::pair().unwrap();
    let (new_receiver, mut new_sender) = UnixStream::pair().unwrap();
    driver
        .register(old_receiver.as_raw_fd(), 0, Interest::ReadableWritable)
        .unwrap();
    assert_eq!(driver.poll(Some(Duration::from_secs(2))).unwrap(), 1);
    assert!(driver.events()[0].writable);
    assert!(
        !driver.events()[0].readable,
        "requested interest is not actual readiness"
    );
    driver
        .modify(old_receiver.as_raw_fd(), 0, Interest::Readable)
        .unwrap();
    let start = Instant::now();
    assert_eq!(driver.poll(Some(Duration::from_millis(25))).unwrap(), 0);
    assert!(start.elapsed() >= Duration::from_millis(20));
    assert!(start.elapsed() < Duration::from_secs(2));
    old_sender.write_all(b"old").unwrap();
    driver.unregister(old_receiver.as_raw_fd()).unwrap();
    driver
        .register(new_receiver.as_raw_fd(), 0, Interest::Readable)
        .unwrap();
    assert_eq!(driver.poll(Some(Duration::from_millis(25))).unwrap(), 0);
    new_sender.write_all(b"new").unwrap();
    assert_eq!(driver.poll(Some(Duration::from_secs(2))).unwrap(), 1);
    assert_eq!(driver.events()[0].token, 0);
    assert!(driver.events()[0].readable);
    assert!(!driver.events()[0].writable);
}
