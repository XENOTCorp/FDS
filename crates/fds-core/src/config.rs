//! Runtime configuration (`config.json`), per the standard: the config
//! file is the sole runtime configuration source; defaults are the
//! baseline and environment variables override individual fields
//! (`FDS_<SECTION>_<KEY>`, e.g. `FDS_REACTOR_BUSY_POLL=1`).
//!
//! Sub-project 3 owns the adaptive *build-time* configuration; this
//! module is the engine's runtime side, read once at startup.

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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CoreConfig {
    /// Pin each worker thread to a physical core.
    pub pin_cores: bool,
    /// Worker thread count; 0 = one per physical core.
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
    /// epoll edge-triggered, busy-poll (timeout 0) — default.
    #[default]
    EpollBusyPoll,
    /// io_uring SQPOLL (experimental, feature `io-uring`).
    IoUring,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ReactorConfig {
    pub strategy: ReactorStrategy,
    /// Preallocated epoll event array capacity per reactor.
    pub max_events: usize,
    /// Busy-poll the ready queue to empty before yielding.
    pub busy_poll: bool,
    /// epoll_wait timeout in milliseconds when not busy-polling.
    pub timeout_ms: i32,
}

impl Default for ReactorConfig {
    fn default() -> Self {
        ReactorConfig {
            strategy: ReactorStrategy::default(),
            max_events: 256,
            busy_poll: true,
            timeout_ms: 0,
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
            incoming_cpu: true,
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
