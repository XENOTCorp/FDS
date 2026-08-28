# Build

All commands in this page run from `Code/` unless noted.

## Adaptive build

`build/build.sh` detects the host, derives rustflags, and invokes cargo. The workspace `Cargo.toml` stays the portable baseline. Host flags in `~/.cargo/config.toml` are the developer baseline. `build.sh` injects `cargo --config build.rustflags=[...]`, which has the highest precedence.

```sh
bash build/build.sh --release
bash build/build.sh --summary
TARGET_CPU=x86-64-v3 bash build/build.sh --release
cargo run -p fds-detect -- --emit-config
```

Overrides:

- `TARGET_CPU`: pin a uarch (for example `haswell`, `skylake-avx512`) instead of `native`.
- `RUSTFLAGS_EXTRA`: extra rustflags, space-separated.

## Profiles

| Flag | debug | release | Adaptive | Effect |
| --- | --- | --- | --- | --- |
| `opt-level` | 1 (own), 3 (deps) | 3 | no | 3 is the silicon target. Deps at 3 in dev are compiled once and cached. |
| `target-cpu` | (host) | `native` | yes | `native` enables every feature this CPU has. Pin with `TARGET_CPU` for portable binaries. |
| `lto` | off | fat | no | Fat LTO across crates at release. |
| `codegen-units` | 16 | 1 | no | One unit is whole-crate optimization. |
| `panic` | unwind | abort | no | Abort shrinks the binary. The test profile always unwinds. |
| `overflow-checks` | on | off | no | Off at release. The dataplane validates lengths explicitly. |
| `debug-assertions` | on | off | no | Ring index checks in debug. |
| `strip` | off | on | no | Shrinks I-cache footprint. |

Full matrix: [Code/build/PROFILES.md](../../Code/build/PROFILES.md).

## fds-detect

```sh
cargo run -p fds-detect
cargo run -p fds-detect -- --emit-config config.json
cargo run -p fds-detect -- --generate-schema config/config.schema.json
cargo run -p fds-detect -- --validate-config config.json
```

Detection is deterministic: same machine, same inputs, same output.

Socket buffer defaults follow `clamp(pow2(L3/2), 4 MiB, 16 MiB)` so one L3-sized burst stays cache-resident.

## Features

Default features on `fds` and `fds-engine`: `sctp`, `io-uring`, `af-xdp`. Disable them for a slimmer library consumer.

```sh
cargo build --release -p fds --no-default-features
```

SCTP needs libsctp and the kernel `sctp` module. io_uring needs kernel 5.19 or later. AF_XDP needs an XDP-capable device at runtime. Tests skip those paths when the host cannot provide them.

## Tests and clippy

```sh
cargo test --release
cargo clippy --all-targets -- -D warnings
```

Both commands must exit 0.

## Thesis PDF

```sh
cd ../Docs/paper
bash build.sh
```

Add `--verify` to run the six proof-checking tools in `Docs/paper/verify/`.
