use super::*;
use crate::alloc_count;

#[test]
fn bench_smoke() {
    let stats = run_inner(1).expect("FDS loopback benchmark must succeed");
    assert!(stats.packets > 0, "bench moved no packets");
}

#[test]
fn bench_large_smoke() {
    let cfg = UdpConfig {
        rcvbuf: 16 << 20,
        sndbuf: 16 << 20,
        ..Default::default()
    };
    let (sb, sp, _) = large_send(60_000, 1, &cfg).unwrap();
    let (rb, rp, _) = large_recv(60_000, 1, &cfg).unwrap();
    assert!(sp > 0 && sb > 0, "large send moved no data");
    assert!(rp > 0 && rb > 0, "large recv moved no data");
}

#[cfg(feature = "sctp")]
#[test]
fn sctp_bench_runs_or_skips() {
    // Without the kernel module this explicitly reports a skip.
    run_sctp(1).expect("bench-sctp must not error");
}

#[test]
fn alloc_counter_observes_allocations() {
    alloc_count::reset();
    let allocation: Vec<u8> = Vec::with_capacity(1024);
    std::hint::black_box(&allocation);
    assert!(alloc_count::count() > 0, "control must observe allocation");
    alloc_count::reset();
    assert_eq!(alloc_count::count(), 0);
}

#[test]
fn udp_echo_datapath_allocates_nothing() {
    // A dedicated thread isolates the per-thread counter from other tests.
    std::thread::spawn(|| {
        let dp = engine_datapath().expect("UDP datapath must be constructible");
        let payload = [0xabu8; DATAGRAM];
        let msgs: Vec<(&[u8], SocketAddr)> = vec![(&payload, dp.peer_addr); BATCH];
        let mut bufs = vec![mol::Buffer::new(); SLOTS];
        let mut out: Vec<RecvResult> = (0..SLOTS)
            .map(|_| RecvResult {
                len: 0,
                src: SocketAddr::from(([0, 0, 0, 0], 0)),
                truncated: false,
            })
            .collect();
        let mut scratch = [0u8; 2048];
        send_chunk(&dp, &msgs, CHUNK).expect("warmup send");
        echo_chunk(&dp, &mut scratch).expect("warmup echo");
        recv_chunk(&dp, &mut bufs, &mut out).expect("warmup recv");
        alloc_count::reset();
        for _ in 0..50 {
            send_chunk(&dp, &msgs, CHUNK).expect("send");
            echo_chunk(&dp, &mut scratch).expect("echo");
            recv_chunk(&dp, &mut bufs, &mut out).expect("recv");
        }
        assert_eq!(alloc_count::count(), 0, "UDP hot loop allocated");
    })
    .join()
    .expect("datapath thread panicked");
}
