//! Observability (standard [OBS]): lock-free per-core counters (padded
//! atomics from the framework) and a pull-based Unix socket endpoint —
//! no allocation in the hot path, no HTTP stack. The endpoint accepts
//! one connection at a time, writes the metrics text, and closes.
//!
//! CONTRACT (implementer): implement [`Metrics`] (the counter bundle),
//! [`CounterSet`] (fixed set of named u64 counters on padded atomics),
//! and [`MetricsServer`] (Unix listener). `MetricsServer` must format
//! numbers without heap allocation (write digits into a stack buffer).
//! Tests: counter increments visible in the snapshot, server accept →
//! read → parse the text (contains the counter names), listener cleanup
//! (socket path unlinked on drop).

use std::path::Path;

/// A fixed set of named counters (per-core, lock-free).
pub struct CounterSet {
    // CONTRACT: implementer stores e.g. a small array of
    // mol::PaddedCounter with a static name table.
    _private: (),
}

impl CounterSet {
    /// A fresh set with the given names.
    pub fn new(names: &[&'static str]) -> Self {
        let _ = names;
        todo!("CounterSet::new: implemented by fds-core milestone task")
    }

    /// Add `v` to counter `i`.
    pub fn add(&self, i: usize, v: u64) {
        let _ = (i, v);
        todo!("CounterSet::add: implemented by fds-core milestone task")
    }

    /// Snapshot all counters into `out` (len == names.len()).
    pub fn snapshot(&self, out: &mut [u64]) {
        let _ = out;
        todo!("CounterSet::snapshot: implemented by fds-core milestone task")
    }
}

/// The engine-wide metrics bundle.
pub struct Metrics {
    // CONTRACT: implementer bundles CounterSets (packets, bytes, drops
    // per core).
    _private: (),
}

impl Metrics {
    pub fn new(cores: usize) -> Self {
        let _ = cores;
        todo!("Metrics::new: implemented by fds-core milestone task")
    }

    /// Format the full metrics text into `out` (no allocation).
    pub fn write_into(&self, out: &mut String) {
        let _ = out;
        todo!("Metrics::write_into: implemented by fds-core milestone task")
    }
}

/// Pull endpoint: a Unix socket that serves [`Metrics::write_into`] text.
pub struct MetricsServer {
    // CONTRACT: implementer owns the listener fd + socket path.
    _private: (),
}

impl MetricsServer {
    /// Bind the Unix socket at `path` (unlinks a stale file first).
    pub fn bind(path: &Path) -> std::io::Result<Self> {
        let _ = path;
        todo!("MetricsServer::bind: implemented by fds-core milestone task")
    }

    /// Serve one request: accept, write metrics text, close. Returns
    /// `false` when no connection was pending.
    pub fn poll_once(&mut self, metrics: &Metrics) -> std::io::Result<bool> {
        let _ = metrics;
        todo!("MetricsServer::poll_once: implemented by fds-core milestone task")
    }
}

impl Drop for MetricsServer {
    fn drop(&mut self) {
        // CONTRACT: unlink the socket path.
    }
}
