//! Thin CLI: parse explicitly, preserve errors, and keep defaults predictable.
#[cfg(test)]
mod alloc_count;
mod benchmarks;
mod engine;
mod signals;

use fds::config::{Config, ConfigError};
use std::{io, path::Path, str::FromStr};

const HELP: &str = "FDS networking examples and benchmarks
Usage: fds [config.json]
       fds --bench [seconds]
       fds --bench-large [datagram-bytes] [seconds]
       fds --bench-sctp [seconds]
       fds --bench-ustack [seconds]
       fds --latency [seconds]
       fds --latency-tcp [seconds]
       fds --bench-tcp-against [IP:port] [seconds]
       fds --bench-udp-against [IP:port] [seconds]
       fds --latency-against [IP:port] [seconds]
       fds --metrics-pull [socket-path]
       fds --fuzz [iterations]
       fds --help | --version
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = run_cli(&args) {
        eprintln!("fds: {error}");
        std::process::exit(1);
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn argument<T: FromStr>(args: &[String], index: usize, default: T) -> io::Result<T> {
    args.get(index).map_or(Ok(default), |text| {
        text.parse()
            .map_err(|_| invalid(format!("invalid argument {index}: {text:?}")))
    })
}

fn run_cli(args: &[String]) -> io::Result<()> {
    let command = args.first().map(String::as_str);
    let max_args = match command {
        Some(
            "--bench-large" | "--bench-tcp-against" | "--bench-udp-against" | "--latency-against",
        ) => 3,
        Some("--help" | "-h" | "--version") => 1,
        Some(flag) if flag.starts_with('-') => 2,
        _ => 1,
    };
    if args.len() > max_args {
        return Err(invalid("too many arguments; see --help"));
    }
    match command {
        Some("--help" | "-h") => {
            print!("{HELP}");
            Ok(())
        }
        Some("--version") => {
            println!("fds {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--bench") => benchmarks::run(argument(args, 1, 2)?),
        Some("--bench-large") => {
            benchmarks::run_large(argument(args, 1, 60_000)?, argument(args, 2, 3)?)
        }
        Some("--bench-sctp") => benchmarks::run_sctp(argument(args, 1, 3)?),
        Some("--bench-ustack") => benchmarks::run_ustack(argument(args, 1, 1)?),
        Some("--latency") => benchmarks::run_latency(argument(args, 1, 2)?),
        Some("--latency-tcp") => benchmarks::run_latency_tcp(argument(args, 1, 2)?),
        Some("--bench-tcp-against") => benchmarks::run_tcp_against(
            argument(args, 1, ([127, 0, 0, 1], 7778).into())?,
            argument(args, 2, 3)?,
        ),
        Some("--bench-udp-against") => benchmarks::run_udp_against(
            argument(args, 1, ([127, 0, 0, 1], 7777).into())?,
            argument(args, 2, 3)?,
        ),
        Some("--latency-against") => benchmarks::run_engine_latency(
            argument(args, 1, ([127, 0, 0, 1], 7777).into())?,
            argument(args, 2, 2)?,
        ),
        Some("--metrics-pull") => benchmarks::run_metrics_pull(
            args.get(1)
                .map(String::as_str)
                .unwrap_or("/tmp/fds-metrics.sock"),
        ),
        Some("--fuzz") => {
            fds::fuzz::run(argument(args, 1, 1_000_000)?);
            Ok(())
        }
        Some(flag) if flag.starts_with('-') => {
            Err(invalid(format!("unknown option {flag:?}; see --help")))
        }
        path => {
            let cfg = load_config(Path::new(path.unwrap_or("config.json")), path.is_none())?;
            engine::run(&cfg)
        }
    }
}

/// Only an absent *implicit* config may fall back. Explicit paths,
/// permissions errors, malformed JSON, and invalid settings must fail.
fn load_config(path: &Path, allow_missing: bool) -> io::Result<Config> {
    match Config::from_file(path) {
        Ok(cfg) => Ok(cfg),
        Err(ConfigError::Io(error)) if allow_missing && error.kind() == io::ErrorKind::NotFound => {
            let mut cfg = Config::default();
            cfg.apply_env();
            cfg.validate()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
            eprintln!("fds: no config at {}; using defaults", path.display());
            Ok(cfg)
        }
        Err(ConfigError::Io(error)) => Err(error),
        Err(error) => Err(io::Error::new(io::ErrorKind::InvalidData, error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_and_unknown_arguments_fail_without_starting_engine() {
        for args in [
            vec!["--unknown"],
            vec!["--bench", "oops"],
            vec!["--latency-against", "localhost"],
            vec!["--version", "extra"],
        ] {
            let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
            assert_eq!(
                run_cli(&args).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

    #[test]
    fn explicit_missing_configuration_is_an_error() {
        let path = std::env::temp_dir().join(format!("fds-missing-config-{}", std::process::id()));
        assert_eq!(
            load_config(&path, false).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }
}
