//! The `fds` binary: a thin CLI over the [`fds_core`] library.
//!
//! Usage: `fds [config.json]` runs the built-in echo engine; `fds
//! --bench <secs>` runs the UDP loopback benchmark; `fds --bench-large
//! <datagram> <secs>` runs the one-way large-datagram byte-ceiling
//! benchmark; `fds --latency <secs>` measures engine loopback latency;
//! `fds --latency-against <addr> <secs>` measures engine latency from a
//! second process; `fds --fuzz <iters>` runs the parser fuzz harness.

use fds_core::benchmarks;
use fds_core::config::Config;
use fds_core::engine;
use fds_core::fuzz;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("--bench") => {
            let secs = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2);
            benchmarks::run(secs)
        }
        Some("--bench-large") => {
            let datagram = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(60_000);
            let secs = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3);
            benchmarks::run_large(datagram, secs)
        }
        Some("--latency-tcp") => {
            let secs = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2);
            benchmarks::run_latency_tcp(secs)
        }
        Some("--metrics-pull") => {
            let path = args.get(1).map(String::as_str).unwrap_or("/tmp/fds-metrics.sock");
            benchmarks::run_metrics_pull(path)
        }
        Some("--bench-sctp") => {
            let secs = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(3);
            benchmarks::run_sctp(secs)
        }
        Some("--bench-tcp-against") => {
            let addr: std::net::SocketAddr = args
                .get(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| {
                    eprintln!("fds: --bench-tcp-against <addr> [secs] — using 127.0.0.1:7778");
                    "127.0.0.1:7778".parse().unwrap()
                });
            let secs = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3);
            benchmarks::run_tcp_against(addr, secs)
        }
        Some("--bench-udp-against") => {
            let addr: std::net::SocketAddr = args
                .get(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| {
                    eprintln!("fds: --bench-udp-against <addr> [secs] — using 127.0.0.1:7777");
                    "127.0.0.1:7777".parse().unwrap()
                });
            let secs = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3);
            benchmarks::run_udp_against(addr, secs)
        }
        Some("--latency") => {
            let secs = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2);
            benchmarks::run_latency(secs)
        }
        Some("--latency-against") => {
            let addr: std::net::SocketAddr = args
                .get(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| {
                    eprintln!("fds: --latency-against <addr> [secs] — using 127.0.0.1:7777");
                    "127.0.0.1:7777".parse().unwrap()
                });
            let secs = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2);
            benchmarks::run_engine_latency(addr, secs)
        }
        Some("--fuzz") => {
            let iters = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1_000_000);
            fuzz::run(iters);
            Ok(())
        }
        _ => {
            let path = args
                .first()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("config.json"));
            let cfg = load_config(&path);
            engine::run(&cfg)
        }
    };
    if let Err(e) = code {
        eprintln!("fds: {e}");
        std::process::exit(1);
    }
}

/// Load `config.json`, falling back to defaults with a note.
fn load_config(path: &std::path::Path) -> Config {
    match std::fs::metadata(path) {
        Ok(_) => match Config::from_file(path) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("fds: bad config {}: {e}", path.display());
                std::process::exit(1);
            }
        },
        Err(_) => {
            let cfg = Config::default();
            eprintln!(
                "fds: no config at {} — using defaults (epoll busy-poll, udp 127.0.0.1:7777)",
                path.display()
            );
            cfg
        }
    }
}
