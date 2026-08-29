//! Runtime configuration (`config.json`), per the standard: the config
//! file is the sole runtime configuration source; defaults are the
//! baseline and environment variables override individual fields
//! (`FDS_<SECTION>_<KEY>`, e.g. `FDS_REACTOR_BUSY_POLL=1`).
//!
//! The file is the single `Code/config.json` (one file, no layering;
//! sub-project 3 ruling), consumed at startup by the engine. The adaptive
//! *build-time* tooling (sub-project 3) lives in `build/`: `build.sh`
//! derives codegen flags from the hardware, and `fds-detect` regenerates
//! `config.json` and `config/config.schema.json` (which validates this
//! file) from the same field table as this module's serde model.

use serde::{Deserialize, Serialize};

/// Full engine configuration. Every field has a default; `config.json`
/// fields are optional and override the defaults.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub core: CoreConfig,
    pub reactor: ReactorConfig,
    pub udp: UdpConfig,
    pub tcp: TcpConfig,
    pub sctp: SctpConfig,
    pub metrics: MetricsConfig,
    pub zero_copy: ZeroCopyConfig,
    pub engine: EngineConfig,
    /// AF_XDP device binding. Empty `device` keeps the kernel socket path.
    pub af_xdp: AfXdpConfig,
}

/// Application-level binds for the built-in engine loop (UDP/TCP echo).
/// The engine is the minimal runnable dataplane; real applications wire
/// their own handlers around the same reactor/transport primitives.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineConfig {
    /// UDP echo bind address ("ip:port").
    pub udp_bind: String,
    /// TCP echo bind address ("ip:port").
    pub tcp_bind: String,
    /// Run the userspace TCP stack (RACK, TSO, loss recovery) on the
    /// AF_XDP datapath instead of the UDP frame echo.
    pub userspace_tcp: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            udp_bind: "127.0.0.1:7777".to_string(),
            tcp_bind: "127.0.0.1:7778".to_string(),
            userspace_tcp: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CoreConfig {
    /// Pin each worker. When the worker count fits on the physical
    /// cores, pin to the first SMT sibling of a distinct core; otherwise
    /// pin worker `i` to logical CPU `i`.
    pub pin_cores: bool,
    /// Worker thread count; 0 = one per logical CPU.
    pub threads: usize,
    /// Stack size for worker threads, in bytes.
    pub stack_bytes: usize,
}

impl Default for CoreConfig {
    fn default() -> Self {
        CoreConfig {
            pin_cores: true,
            threads: 0,
            stack_bytes: 1 << 20,
        }
    }
}

/// Polling strategy (spec decision matrix D-5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReactorStrategy {
    /// epoll edge-triggered, busy-poll (timeout 0); default.
    #[default]
    EpollBusyPoll,
    /// io_uring SQPOLL (experimental, feature `io-uring`).
    IoUring,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ReactorConfig {
    pub strategy: ReactorStrategy,
    /// Preallocated event array capacity per reactor.
    pub max_events: usize,
    /// Busy-poll the ready queue to empty before yielding (explicit
    /// spin with a zero epoll timeout; for dedicated cores only).
    pub busy_poll: bool,
    /// Poll timeout in milliseconds when not busy-polling.
    pub timeout_ms: i32,
    /// io_uring ring entries (strategy `io-uring`).
    pub io_uring_entries: u32,
    /// io_uring SQPOLL thread CPU; 0 = no SQPOLL thread (needs
    /// CAP_SYS_ADMIN; falls back to a plain ring when rejected).
    pub io_uring_sq_thread: u32,
}

impl Default for ReactorConfig {
    fn default() -> Self {
        ReactorConfig {
            strategy: ReactorStrategy::default(),
            max_events: 256,
            busy_poll: false,
            timeout_ms: 0,
            io_uring_entries: 256,
            io_uring_sq_thread: 0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct UdpConfig {
    pub rcvbuf: usize,
    pub sndbuf: usize,
    /// UDP_SEGMENT (GSO) max segment size; 0 = off.
    pub gso_segment_size: usize,
    /// UDP_GRO coalescing.
    pub gro: bool,
    /// MSG_ZEROCOPY for large datagrams.
    pub zerocopy: bool,
    /// SO_REUSEPORT (one socket per core).
    pub reuseport: bool,
    /// SO_INCOMING_CPU steering.
    pub incoming_cpu: bool,
    /// `IPV6_V6ONLY` on IPv6 binds. `false` (default) is dual-stack:
    /// an `[::]` bind also accepts IPv4-mapped clients.
    pub ipv6_only: bool,
}

impl Default for UdpConfig {
    fn default() -> Self {
        UdpConfig {
            rcvbuf: 4 << 20,
            sndbuf: 4 << 20,
            gso_segment_size: 0,
            gro: false,
            zerocopy: false,
            reuseport: true,
            // Off by default: SO_INCOMING_CPU makes reuseport selection
            // prefer the socket matching the RX CPU, which on loopback
            // (one RX softirq CPU) pins every flow to a single worker,
            // defeating per-core distribution. NIC deployments with
            // RSS/IRQ affinity set it explicitly (see ops-tuning).
            incoming_cpu: false,
            ipv6_only: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TcpConfig {
    pub nodelay: bool,
    pub quickack: bool,
    pub defer_accept: bool,
    /// TCP_FASTOPEN queue length; 0 = off (spoofing caveat documented).
    pub fastopen: u32,
    pub cork: bool,
    pub reuseport: bool,
    pub rcvbuf: usize,
    pub sndbuf: usize,
    /// `IPV6_V6ONLY` on IPv6 binds. `false` (default) is dual-stack.
    pub ipv6_only: bool,
}

impl Default for TcpConfig {
    fn default() -> Self {
        TcpConfig {
            nodelay: true,
            quickack: false,
            defer_accept: false,
            fastopen: 0,
            cork: false,
            reuseport: true,
            rcvbuf: 4 << 20,
            sndbuf: 4 << 20,
            ipv6_only: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SctpConfig {
    pub nodelay: bool,
    /// SCTP_INITMSG max streams (in/out).
    pub init_max_streams: u16,
    /// SCTP_PARTIAL_DELIVERY_POINT, bytes.
    pub partial_delivery_point: u32,
    /// SCTP_MAX_BURST; 0 = kernel default.
    pub max_burst: u32,
    pub reuseport: bool,
}

impl Default for SctpConfig {
    fn default() -> Self {
        SctpConfig {
            nodelay: true,
            init_max_streams: 10,
            partial_delivery_point: 0,
            max_burst: 0,
            reuseport: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MetricsConfig {
    /// Unix socket path for the pull endpoint; empty = disabled.
    pub socket_path: String,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        MetricsConfig {
            socket_path: "/tmp/fds-metrics.sock".to_string(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ZeroCopyConfig {
    /// sendfile/splice for file-backed TCP responses.
    pub splice: bool,
    /// io_uring registered buffers (feature `io-uring`).
    pub registered_buffers: bool,
    /// MSG_ZEROCOPY for UDP large datagrams.
    pub udp_zerocopy: bool,
}

impl Default for ZeroCopyConfig {
    fn default() -> Self {
        ZeroCopyConfig {
            splice: true,
            registered_buffers: false,
            udp_zerocopy: false,
        }
    }
}

/// AF_XDP device binding (feature `af-xdp`): when `device` is
/// non-empty, every worker binds a queue of the device and runs the
/// zero-copy frame datapath (kernel bypass) instead of the kernel
/// socket path. `queues` selects per-worker queues; empty = one queue
/// per worker, round-robin over `queue` (for a single-queue device).
/// Absent a device (or without CAP_NET_RAW) the engine logs and
/// continues on the kernel datapath.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AfXdpConfig {
    /// Device name (e.g. "eth0"); empty = disabled.
    pub device: String,
    /// Queue id on the device (used when `queues` is empty).
    pub queue: u32,
    /// Per-worker queue ids (one per worker, round-robin when shorter).
    pub queues: Vec<u32>,
    /// Bind with XDP_ZEROCOPY (falls back to XDP_COPY when the driver
    /// rejects it).
    pub zero_copy: bool,
    /// Per-ring entry count (power of two).
    pub ring_size: u32,
    /// Umem frame count.
    pub num_frames: u32,
    /// Bind each worker's umem to its NUMA node (mbind); the worker
    /// pins to its core first, so the data plane stays on-node.
    pub numa: bool,
    /// Pinned XSKMAP path (bpffs) to register this socket in; empty =
    /// do not register (frames will not reach the socket without an XDP
    /// program steering into the map).
    pub xskmap: String,
}

impl Default for AfXdpConfig {
    fn default() -> Self {
        AfXdpConfig {
            device: String::new(),
            queue: 0,
            queues: Vec::new(),
            zero_copy: true,
            ring_size: 256,
            num_frames: 4096,
            numa: false,
            xskmap: String::new(),
        }
    }
}

impl Config {
    /// Parse a `config.json` document; missing fields fall back to
    /// defaults.
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        let mut cfg: Config = serde_json::from_str(s)?;
        cfg.apply_env();
        Ok(cfg)
    }

    /// Load from a file path.
    pub fn from_file(path: &std::path::Path) -> Result<Self, ConfigError> {
        let s = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
        Self::from_json(&s).map_err(ConfigError::Json)
    }

    /// Override fields from `FDS_<SECTION>_<KEY>` environment variables.
    pub fn apply_env(&mut self) {
        if let Some(v) = env_flag("FDS_CORE_PIN_CORES") {
            self.core.pin_cores = v;
        }
        if let Some(v) = env_usize("FDS_CORE_THREADS") {
            self.core.threads = v;
        }
        if let Some(v) = env_flag("FDS_REACTOR_BUSY_POLL") {
            self.reactor.busy_poll = v;
        }
        if let Some(v) = env_usize("FDS_REACTOR_MAX_EVENTS") {
            self.reactor.max_events = v;
        }
        if let Some(v) = env_i32("FDS_REACTOR_TIMEOUT_MS") {
            self.reactor.timeout_ms = v;
        }
        if let Ok(v) = std::env::var("FDS_REACTOR_STRATEGY") {
            match v.trim().to_ascii_lowercase().as_str() {
                "epoll" | "epoll-busy-poll" => self.reactor.strategy = ReactorStrategy::EpollBusyPoll,
                "io-uring" | "iouring" => self.reactor.strategy = ReactorStrategy::IoUring,
                other => eprintln!("fds: unknown FDS_REACTOR_STRATEGY {other:?} (epoll | io-uring)"),
            }
        }
        if let Some(v) = env_u32("FDS_REACTOR_IO_URING_ENTRIES") {
            self.reactor.io_uring_entries = v;
        }
        if let Some(v) = env_u32("FDS_REACTOR_IO_URING_SQ_THREAD") {
            self.reactor.io_uring_sq_thread = v;
        }
        if let Ok(v) = std::env::var("FDS_AF_XDP_DEVICE") {
            self.af_xdp.device = v;
        }
        if let Some(v) = env_u32("FDS_AF_XDP_QUEUE") {
            self.af_xdp.queue = v;
        }
        if let Ok(v) = std::env::var("FDS_AF_XDP_QUEUES") {
            self.af_xdp.queues = v
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();
        }
        if let Some(v) = env_flag("FDS_AF_XDP_ZERO_COPY") {
            self.af_xdp.zero_copy = v;
        }
        if let Some(v) = env_u32("FDS_AF_XDP_RING_SIZE") {
            self.af_xdp.ring_size = v;
        }
        if let Some(v) = env_u32("FDS_AF_XDP_NUM_FRAMES") {
            self.af_xdp.num_frames = v;
        }
        if let Some(v) = env_flag("FDS_AF_XDP_NUMA") {
            self.af_xdp.numa = v;
        }
        if let Ok(v) = std::env::var("FDS_AF_XDP_XSKMAP") {
            self.af_xdp.xskmap = v;
        }
        if let Some(v) = env_flag("FDS_UDP_GRO") {
            self.udp.gro = v;
        }
        if let Some(v) = env_flag("FDS_UDP_ZEROCOPY") {
            self.udp.zerocopy = v;
        }
        if let Some(v) = env_flag("FDS_TCP_NODELAY") {
            self.tcp.nodelay = v;
        }
        if let Some(v) = env_flag("FDS_TCP_QUICKACK") {
            self.tcp.quickack = v;
        }
        if let Some(v) = env_flag("FDS_UDP_IPV6_ONLY") {
            self.udp.ipv6_only = v;
        }
        if let Some(v) = env_flag("FDS_TCP_IPV6_ONLY") {
            self.tcp.ipv6_only = v;
        }
        if let Ok(v) = std::env::var("FDS_ENGINE_UDP_BIND") {
            self.engine.udp_bind = v;
        }
        if let Ok(v) = std::env::var("FDS_ENGINE_TCP_BIND") {
            self.engine.tcp_bind = v;
        }
        if let Some(v) = env_flag("FDS_ENGINE_USERSPACE_TCP") {
            self.engine.userspace_tcp = v;
        }
        if let Ok(v) = std::env::var("FDS_METRICS_SOCKET_PATH") {
            self.metrics.socket_path = v;
        }
    }
}

/// Error loading the configuration.
#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "config io: {e}"),
            ConfigError::Json(e) => write!(f, "config json: {e}"),
        }
    }
}

impl std::error::Error for ConfigError {}

fn env_flag(key: &str) -> Option<bool> {
    std::env::var(key).ok().map(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn env_usize(key: &str) -> Option<usize> {
    std::env::var(key).ok()?.trim().parse().ok()
}

fn env_i32(key: &str) -> Option<i32> {
    std::env::var(key).ok()?.trim().parse().ok()
}

fn env_u32(key: &str) -> Option<u32> {
    std::env::var(key).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse_and_empty_json_works() {
        let cfg = Config::default();
        assert_eq!(cfg.reactor.strategy, ReactorStrategy::EpollBusyPoll);
        assert_eq!(cfg.udp.rcvbuf, 4 << 20);
        let parsed = Config::from_json("{}").expect("empty json = defaults");
        assert_eq!(parsed.core.stack_bytes, cfg.core.stack_bytes);
    }

    #[test]
    fn json_overrides_defaults() {
        let cfg = Config::from_json(r#"{ "udp": { "rcvbuf": 16777216 } }"#).unwrap();
        assert_eq!(cfg.udp.rcvbuf, 16777216);
        assert_eq!(cfg.udp.sndbuf, Config::default().udp.sndbuf);
    }

    #[test]
    fn malformed_json_errors() {
        assert!(Config::from_json("{ nope").is_err());
    }
}
