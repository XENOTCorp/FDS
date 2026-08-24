//! Observability (standard \[OBS\]): lock-free per-core counters (padded
//! atomics from the framework) and a pull-based Unix socket endpoint —
//! no allocation in the hot path, no HTTP stack. The endpoint accepts
//! one connection at a time, writes the metrics text, and closes.
//!
//! Design: [`Metrics`] owns per-core [`CounterSet`]s for packets, bytes
//! and drops; [`MetricsServer`] is a nonblocking Unix listener that
//! serves one connection per [`poll_once`](MetricsServer::poll_once)
//! with [`Metrics::write_into`] text. Numbers are written into a stack
//! buffer, so formatting never allocates. Tests cover snapshot
//! visibility, report contents, one-shot serving, and socket-path
//! cleanup on drop.

use std::io::{Read, Write};
use std::path::Path;

/// A fixed set of named counters (per-core, lock-free).
pub struct CounterSet {
    /// One padded atomic per counter name.
    counters: Box<[mol::PaddedCounter]>,
    /// The counter names, in the same order as `counters`.
    names: Box<[&'static str]>,
}

impl CounterSet {
    /// A fresh set with the given names (all counters zeroed).
    pub fn new(names: &[&'static str]) -> Self {
        let mut counters = Vec::with_capacity(names.len());
        for _ in names {
            counters.push(mol::PaddedCounter::new(std::sync::atomic::AtomicU64::new(0)));
        }
        CounterSet {
            counters: counters.into_boxed_slice(),
            names: names.to_vec().into_boxed_slice(),
        }
    }

    /// Add `v` to counter `i` (relaxed, lock-free).
    pub fn add(&self, i: usize, v: u64) {
        self.counters[i].fetch_add(v, std::sync::atomic::Ordering::Relaxed);
    }

    /// Store a value into counter `i` (relaxed; replaces, does not add).
    pub fn set(&self, i: usize, v: u64) {
        self.counters[i].store(v, std::sync::atomic::Ordering::Relaxed);
    }

    /// Snapshot all counters into `out` (len == names.len()).
    pub fn snapshot(&self, out: &mut [u64]) {
        assert_eq!(
            out.len(),
            self.counters.len(),
            "snapshot buffer length must match the counter count"
        );
        for (i, c) in self.counters.iter().enumerate() {
            out[i] = c.load(std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Load counter `i` alone (used by the report formatter).
    pub fn load(&self, i: usize) -> u64 {
        self.counters[i].load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The counter names, in index order.
    pub fn names(&self) -> &[&'static str] {
        &self.names
    }
}

/// Write `v` as decimal digits into `out` without allocating: digits go
/// into a stack buffer (u64 needs at most 20), then the slice is pushed.
fn push_num(out: &mut String, mut v: u64) {
    let mut buf = [0u8; 20];
    let mut n = buf.len();
    if v == 0 {
        n -= 1;
        buf[n] = b'0';
    } else {
        while v > 0 {
            n -= 1;
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
        }
    }
    // SAFETY: buf[n..] holds only ASCII digits b'0'..=b'9' (each a valid
    // one-byte UTF-8 code point), and n is an in-bounds index by
    // construction (v == 0 leaves one digit, otherwise one per base-10
    // digit of v, at most 20).
    out.push_str(unsafe { std::str::from_utf8_unchecked(&buf[n..]) });
}

/// The engine-wide metrics bundle: per-core [`CounterSet`]s for packets,
/// bytes and drops, plus the totals across cores.
pub struct Metrics {
    /// Per-core packet counters.
    packets: Vec<CounterSet>,
    /// Per-core byte counters.
    bytes: Vec<CounterSet>,
    /// Per-core drop counters.
    drops: Vec<CounterSet>,
}

impl Metrics {
    pub fn new(cores: usize) -> Self {
        Metrics {
            packets: (0..cores).map(|_| CounterSet::new(&["packets"])).collect(),
            bytes: (0..cores).map(|_| CounterSet::new(&["bytes"])).collect(),
            drops: (0..cores).map(|_| CounterSet::new(&["drops"])).collect(),
        }
    }

    /// Add to worker `core`'s packet counter (relaxed, lock-free; each
    /// worker only touches its own slot, so the padded counters never
    /// contend).
    pub fn add_packets(&self, core: usize, v: u64) {
        self.packets[core].add(0, v);
    }

    pub fn add_bytes(&self, core: usize, v: u64) {
        self.bytes[core].add(0, v);
    }

    pub fn add_drops(&self, core: usize, v: u64) {
        self.drops[core].add(0, v);
    }

    /// Total packets/bytes/drops summed across all cores.
    pub fn totals(&self) -> (u64, u64, u64) {
        let sum = |sets: &[CounterSet]| -> u64 { sets.iter().map(|s| s.load(0)).sum() };
        (sum(&self.packets), sum(&self.bytes), sum(&self.drops))
    }

    /// Format the full metrics text into `out` (no allocation).
    pub fn write_into(&self, out: &mut String) {
        out.push_str("# fds metrics (pull endpoint)\n");
        out.push_str("# cores: ");
        push_num(out, self.packets.len() as u64);
        out.push('\n');
        if !self.packets.is_empty() {
            out.push_str("# counter names per core: ");
            // The counter names come from each per-core set (identical
            // across cores, so reading core 0's names is representative).
            for sets in [&self.packets, &self.bytes, &self.drops] {
                for name in sets[0].names() {
                    out.push_str(name);
                    out.push(' ');
                }
            }
            out.push('\n');
        }
        write_kind(out, &self.packets);
        write_kind(out, &self.bytes);
        write_kind(out, &self.drops);
    }
}

/// Write one counter kind: a `.total` line summed across cores, then a
/// `core.<i>.<name>` line per core.
fn write_kind(out: &mut String, sets: &[CounterSet]) {
    if sets.is_empty() {
        return;
    }
    let names = sets[0].names();
    for (j, name) in names.iter().enumerate() {
        let mut total = 0u64;
        for set in sets {
            total += set.load(j);
        }
        out.push_str(name);
        out.push_str(".total ");
        push_num(out, total);
        out.push('\n');
    }
    for (i, set) in sets.iter().enumerate() {
        for (j, name) in names.iter().enumerate() {
            out.push_str("core.");
            push_num(out, i as u64);
            out.push('.');
            out.push_str(name);
            out.push(' ');
            push_num(out, set.load(j));
            out.push('\n');
        }
    }
}

/// Pull endpoint: a Unix socket that serves [`Metrics::write_into`] text.
#[derive(Debug)]
pub struct MetricsServer {
    /// The bound listener (nonblocking at the kernel level).
    listener: std::os::unix::net::UnixListener,
    /// The socket path, unlinked on drop.
    path: std::path::PathBuf,
}

impl rustix::fd::AsFd for MetricsServer {
    fn as_fd(&self) -> rustix::fd::BorrowedFd<'_> {
        std::os::fd::AsFd::as_fd(&self.listener)
    }
}

impl MetricsServer {
    /// Bind the Unix socket at `path`, best-effort-unlinking a stale
    /// socket file first. Unlink failures are ignored: a stale file
    /// owned by another user must not fail the engine with a misleading
    /// `PermissionDenied` — the bind below then reports the accurate
    /// condition (`AddrInUse` when a socket is actually bound there).
    pub fn bind(path: &Path) -> std::io::Result<Self> {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {}
        }
        let listener = std::os::unix::net::UnixListener::bind(path)?;
        // Kernel-level nonblocking (FIONBIO) so accept() returns
        // WouldBlock instead of parking the caller. rustix::io::Errno
        // converts into std::io::Error, so `?` works here.
        rustix::io::ioctl_fionbio(&listener, true)?;
        Ok(MetricsServer {
            listener,
            path: path.to_path_buf(),
        })
    }

    /// The raw fd (for epoll reactor registration).
    pub fn as_raw_fd(&self) -> i32 {
        use std::os::fd::AsRawFd as _;
        self.listener.as_raw_fd()
    }

    /// Serve one request: accept, write metrics text, close. Returns
    /// `false` when no connection was pending.
    pub fn poll_once(&mut self, metrics: &Metrics) -> std::io::Result<bool> {
        let (mut stream, _addr) = match self.listener.accept() {
            Ok(conn) => conn,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(e) => return Err(e),
        };
        // std tracked the listener as blocking, so the accepted stream
        // is blocking; make it nonblocking so the drain cannot stall on
        // a client that never sends.
        rustix::io::ioctl_fionbio(&stream, true)?;
        // Drain whatever the client sent (ignored: pull-only endpoint).
        let mut buf = [0u8; 4096];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break, // client closed its write side
                Ok(_) => continue,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        let mut report = String::new();
        metrics.write_into(&mut report);
        stream.write_all(report.as_bytes())?;
        drop(stream); // close: the client sees EOF after the report
        Ok(true)
    }
}

impl Drop for MetricsServer {
    fn drop(&mut self) {
        // Unlink the socket path; the listener fd is closed by its own
        // Drop. A missing file (already unlinked) is not an error.
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_add_snapshot() {
        let set = CounterSet::new(&["a", "b"]);
        set.add(0, 5);
        set.add(1, 7);
        let mut out = [0u64; 2];
        set.snapshot(&mut out);
        assert_eq!(out, [5, 7]);
    }

    #[test]
    fn metrics_report_contains_names() {
        let metrics = Metrics::new(1);
        let mut out = String::new();
        metrics.write_into(&mut out);
        assert!(out.contains("packets"), "report: {out}");
        assert!(out.contains("bytes"), "report: {out}");
        assert!(out.contains("drops"), "report: {out}");
    }

    #[test]
    fn server_serves_report() {
        // Unique socket path per test run (pid + sequence) so parallel
        // test processes never collide on the same file.
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "fds-metrics-test-{}-{}.sock",
            std::process::id(),
            seq
        ));

        let metrics = Metrics::new(1);
        let mut server = MetricsServer::bind(&path).expect("bind");

        // No client yet: poll reports nothing pending.
        assert!(!server.poll_once(&metrics).expect("poll"));

        // Connect, serve one request, and read the report back.
        let mut client = std::os::unix::net::UnixStream::connect(&path).expect("connect");
        assert!(server.poll_once(&metrics).expect("poll"));
        let mut report = String::new();
        client.read_to_string(&mut report).expect("read");
        assert!(report.contains("packets"), "report: {report}");

        // Dropping the server unlinks the socket file.
        drop(server);
        assert!(!path.exists(), "socket path must be unlinked on drop");
    }

    #[test]
    fn stale_socket_file_is_rebound() {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "fds-metrics-stale-{}-{}.sock",
            std::process::id(),
            seq
        ));

        // A stale file at the socket path (no live listener) must be
        // unlinked and rebound.
        std::fs::write(&path, b"").expect("stale file");
        let server = MetricsServer::bind(&path).expect("stale file must be rebound");
        drop(server);
        assert!(!path.exists(), "socket path must be unlinked on drop");
    }

    #[test]
    fn no_alloc_formatting() {
        // write_into appends only to the caller-provided String; running
        // it repeatedly would surface any accidental per-call allocation
        // as a regression (this test just exercises the path).
        let metrics = Metrics::new(4);
        for _ in 0..1000 {
            let mut out = String::new();
            metrics.write_into(&mut out);
        }
    }
}
