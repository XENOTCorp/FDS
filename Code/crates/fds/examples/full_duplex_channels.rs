//! Full-duplex parallel channels on the fds primitives.
//!
//! A custom protocol server built directly on the reactor and the TCP
//! transport (no engine loop), plus a client that opens N parallel TCP
//! connections, each carrying independent bidirectional traffic.
//! Full duplex means each connection sends and receives at the same
//! time; parallel means N connections at once.
//!
//! ```sh
//! cargo run --release -p fds --example full_duplex_channels
//! ```
//!
//! The server listens on 127.0.0.1:7820 and echoes every byte back on
//! the connection that sent it. The client opens 8 channels, streams
//! 8 KiB frames into each while reading the echoes back concurrently,
//! and prints per-channel and aggregate throughput.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use fds::config::TcpConfig;
use fds::reactor::{Interest, Reactor};
use fds::tcp::{TcpListener, TcpStream as FdsTcp};

const BIND: &str = "127.0.0.1:7820";
const CHANNELS: usize = 8;
const FRAME: usize = 8 * 1024;
const DURATION: Duration = Duration::from_secs(5);
const RX_CAP: usize = 64 * 1024;

/// One accepted channel: the stream, a preallocated receive buffer, a
/// pending echo buffer (retains capacity between frames: no steady-state
/// allocation), and the write offset.
struct Chan {
    stream: FdsTcp,
    rx: Box<[u8; RX_CAP]>,
    tx: Vec<u8>,
    off: usize,
}

fn would_block(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::WouldBlock
}

/// Drain the channel until EAGAIN, writing each chunk as it is read
/// (the pending buffer stays bounded by one receive burst). Returns
/// false when the peer closed or errored.
fn drain(chan: &mut Chan, reactor: &Reactor, token: u64) -> bool {
    loop {
        match chan.stream.read(&mut chan.rx[..]) {
            Ok(0) => return false,
            Ok(n) => {
                chan.tx.extend_from_slice(&chan.rx[..n]);
                if !flush(chan, reactor, token) {
                    return false;
                }
                if chan.off < chan.tx.len() {
                    return true; // write backlog: stop reading
                }
            }
            Err(e) if would_block(&e) => break,
            Err(_) => return false,
        }
    }
    true
}

/// Write the pending buffer; on EAGAIN keep the remainder and wait for
/// writability. Returns false on error, true when the backlog is gone
/// or still pending.
fn flush(chan: &mut Chan, reactor: &Reactor, token: u64) -> bool {
    while chan.off < chan.tx.len() {
        match chan.stream.writev(&[&chan.tx[chan.off..]]) {
            Ok(n) => chan.off += n,
            Err(e) if would_block(&e) => break,
            Err(_) => return false,
        }
    }
    if chan.off < chan.tx.len() {
        // Write backlog: stop reading until it drains (backpressure:
        // the pending buffer stays bounded by one receive burst).
        let _ = reactor.modify(chan.stream.as_raw_fd(), token, Interest::Writable);
        return true;
    }
    chan.tx.clear();
    chan.off = 0;
    let _ = reactor.modify(chan.stream.as_raw_fd(), token, Interest::Readable);
    true
}

/// The server: a reactor loop over the listener and the channels.
fn server(addr: SocketAddr) {
    let mut reactor = Reactor::new(64).expect("reactor");
    let listener = TcpListener::bind(addr, &TcpConfig::default(), 128).expect("bind");
    reactor
        .register(listener.as_raw_fd(), 0, Interest::Readable)
        .expect("register listener");
    let mut chans: Vec<Option<Chan>> = Vec::with_capacity(16);

    loop {
        let n = reactor.poll_once().expect("poll");
        if n == 0 {
            continue;
        }
        for ev in reactor.delivered(n) {
            if ev.error {
                // EPOLLERR/EPOLLHUP: drop the channel, keep the listener.
                if ev.token != 0 {
                    if let Some(chan) = chans[ev.token as usize - 1].take() {
                        let _ = reactor.unregister(chan.stream.as_raw_fd());
                    }
                }
                continue;
            }
            if ev.token == 0 {
                // Listener ready: accept until EAGAIN.
                while let Ok(Some((stream, _))) = listener.accept() {
                    let slot = chans
                        .iter()
                        .position(Option::is_none)
                        .unwrap_or_else(|| {
                            chans.push(None);
                            chans.len() - 1
                        });
                    let token = slot as u64 + 1;
                    reactor
                        .register(stream.as_raw_fd(), token, Interest::Readable)
                        .expect("register stream");
                    let mut chan = Chan {
                        stream,
                        rx: Box::new([0u8; RX_CAP]),
                        tx: Vec::with_capacity(FRAME),
                        off: 0,
                    };
                    // Edge-triggered accept race: the peer may have
                    // written before registration, and ET never fires for
                    // data pending at EPOLL_CTL_ADD. Drain once now.
                    let open = drain(&mut chan, &reactor, token);
                    chans[slot] = Some(chan);
                    if !open {
                        let _ = reactor.unregister(chans[slot].as_ref().unwrap().stream.as_raw_fd());
                        chans[slot] = None;
                    }
                }
            } else if let Some(chan) = chans[ev.token as usize - 1].as_mut() {
                let open = if ev.readable {
                    drain(chan, &reactor, ev.token)
                } else {
                    true
                };
                let open = open && if ev.writable { flush(chan, &reactor, ev.token) } else { open };
                if !open {
                    let _ = reactor.unregister(chan.stream.as_raw_fd());
                    chans[ev.token as usize - 1] = None;
                }
            }
        }
    }
}

/// One client channel: full duplex on a single connection, implemented
/// by cloning the socket so one thread writes frames while another
/// reads the echoes back. Returns the bytes echoed.
fn channel(addr: SocketAddr, id: usize) -> u64 {
    let mut s = TcpStream::connect(addr).expect("connect");
    s.set_nodelay(true).ok();
    let mut r = s.try_clone().expect("clone");
    let frame = vec![b'c'; FRAME];

    let writer = thread::spawn(move || {
        let mut sent: u64 = 0;
        let deadline = Instant::now() + DURATION;
        while Instant::now() < deadline {
            sent += s.write(&frame).unwrap_or(0) as u64;
        }
        sent
    });
    let deadline = Instant::now() + DURATION;
    r.set_read_timeout(Some(Duration::from_millis(100))).ok();
    let mut recv: u64 = 0;
    let mut buf = vec![0u8; FRAME];
    while Instant::now() < deadline {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => recv += n as u64,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
    drop(r); // close this end: the server sees EOF and releases the slot
    let sent = writer.join().expect("writer");
    let mib = recv as f64 / DURATION.as_secs_f64() / 1e6;
    eprintln!("channel {id}: sent {sent} B, echoed {recv} B ({mib:.1} MiB/s)");
    recv
}

fn main() {
    let addr: SocketAddr = BIND.parse().expect("bind address");
    thread::spawn(move || server(addr));
    thread::sleep(Duration::from_millis(200));

    let start = Instant::now();
    let mut handles = Vec::with_capacity(CHANNELS);
    for id in 0..CHANNELS {
        handles.push(thread::spawn(move || channel(addr, id)));
    }
    let mut total: u64 = 0;
    for h in handles {
        total += h.join().expect("channel");
    }
    let mib = total as f64 / start.elapsed().as_secs_f64() / 1e6;
    eprintln!(
        "aggregate: {total} B echoed in {:?} ({mib:.1} MiB/s over {CHANNELS} full-duplex channels)",
        start.elapsed()
    );
}
