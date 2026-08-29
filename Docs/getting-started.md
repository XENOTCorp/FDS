# Getting started

This guide installs FDS, runs the tests, and starts the echo engine.

## 1. Install the tools

Install Rust 1.97.1 or later. Use a Linux host. The engine does not run on
other kernels.

Confirm the toolchain:

```sh
rustc --version
uname -s
```

Install the development packages for libsctp and liburing. These are
required for the default features. Use the package manager of your
distribution. On Debian or Ubuntu:

```sh
sudo apt-get install -y libsctp-dev liburing-dev
```

## 2. Open the code tree

```sh
cd Code
```

All `cargo` commands in this guide run from `Code/`.

## 3. Test the workspace

```sh
cargo test --release
cargo clippy --all-targets -- -D warnings
```

Both commands must exit 0.

## 4. Build the adaptive release

```sh
bash build/build.sh --release
```

The script detects the host, sets the codegen flags, and builds the
workspace. The engine binary is `target/release/fds`. Use
`bash build/build.sh --summary` to show the detection facts.

## 5. Run the echo engine

```sh
./target/release/fds
```

The process starts one worker per logical CPU. UDP echo binds to
127.0.0.1:7777. TCP echo binds to 127.0.0.1:7778.

Stop the process with Ctrl-C.

## 6. Optional configuration

Copy `config.json` in `Code/` or pass a path:

```sh
./target/release/fds /path/to/config.json
```

Override one key with an environment variable. The form is
`FDS_<SECTION>_<KEY>`.

```sh
FDS_CORE_THREADS=2 ./target/release/fds
```

The schema is `Code/config/config.schema.json`. Regenerate the host
configuration:

```sh
cargo run --release -p fds-detect -- --emit-config
```

## 7. Mini project: full-duplex channels

This example builds a TCP echo server on the reactor. It then opens eight
connections and streams in both directions.

```sh
cargo run --release -p fds --example full_duplex_channels
```

## 8. Measure

Run the engine in one terminal. Then run the benchmark client in another:

```sh
cargo run --release -p fds-engine -- --bench-large 60000 5
cargo run --release -p fds-engine -- --latency 5
```

Against a running engine:

```sh
cargo run --release -p fds-engine -- --bench-udp-against 127.0.0.1:7777 5
cargo run --release -p fds-engine -- --bench-tcp-against 127.0.0.1:7778 5
```

Published numbers: [benchmarks.md](benchmarks.md).

## 9. Build the thesis

```sh
cd ../Docs/paper
bash build.sh
```

Add `--verify` to run the six proof-checking tools.

## Next

- [Wiki index](wiki/README.md)
- [Architecture](wiki/architecture.md)
- [Configuration](wiki/configuration.md)
- [Operations](wiki/operations.md)
