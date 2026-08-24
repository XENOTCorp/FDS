# FDS — Fast Data Transmission

A Rust project for TCP/UDP/SCTP communication engineered to the silicon:
nonblocking, edge-triggered, busy-polling, batched, zero-allocation
dataplanes — usable for web servers, DNS, FTP, and custom protocols.
Linux-only.

The project is built as sequential sub-projects, each grounded in a formal
paper (thesis) and a software standard:

| Sub-project | What | Where | Status |
|---|---|---|---|
| 1 | **Paper + standard** — the Mol category theory (NT1–NT55), a rewriting-based cost model, an ARCSS-style software standard, and proof-verification tools | `docs/paper/`, `docs/standard/`, `docs/paper/verify/` | merged |
| 2 | **Mol framework** — `mol-core` (lib `mol`): atoms, molecules, composition, lock-free rings, buffers, memory layout, SIMD checksums, authoring templates | `crates/mol-core/`, `templates/` | merged |
| 3 | **Build tooling** — adaptive build script + `config.json` hardware detection | *(next)* | pending |
| 4 | **Transport engine** — `fds-core` (bin `fds`): epoll busy-poll reactor, UDP/TCP/SCTP transports, hot/cold connection state, metrics, bench/fuzz | `crates/fds-core/`, `docs/ops-tuning.md` | merged |

## Quick start

```sh
# Build and test everything
cargo test --workspace          # 89 tests, ~2 s
cargo clippy --workspace --all-targets -- -D warnings

# Run the engine (UDP + TCP echo on 127.0.0.1:7777 / 7778)
# One worker per logical CPU (2x physical on hyperthreaded machines);
# SO_REUSEPORT distributes flows across workers.
cargo run -p fds-core

# Measure: throughput, transport latency, engine latency
cargo run -p fds-core --release -- --bench 5
# Byte ceiling: one-way large-datagram throughput per direction (the 10-40+ Gbps loopback number)
cargo run -p fds-core --release -- --bench-large 60000 5
cargo run -p fds-core --release -- --latency 5
# (engine running in another terminal)
cargo run -p fds-core --release -- --latency-against 127.0.0.1:7777 5

# io_uring reactor strategy, explicit worker count (default: per logical CPU)
FDS_REACTOR_STRATEGY=io-uring FDS_CORE_THREADS=4 cargo run -p fds-core --release

# Fuzz the parsers/checksums (deterministic, stable-rust)
cargo run -p fds-core --release -- --fuzz 1000000
```

## Repo layout

```
Cargo.toml                 # workspace: profiles (release = silicon-level)
.cargo/config.toml         # portable baseline flags
crates/mol-core/           # sub-project 2: the Mol framework (lib `mol`)
crates/fds-core/           # sub-project 4: the engine (bin `fds`)
templates/                 # authoring templates (pure/effectful/hybrid/reactor)
docs/paper/                # sub-project 1: thesis (99 pp, NT1–NT55) + verify tools
docs/standard/             # sub-project 1: software standard (50 policies)
docs/ops-tuning.md         # NIC/kernel/engine tuning guide
docs/superpowers/specs/    # design specs (one per sub-project)
scripts/perf.sh            # perf stat/record, llvm-mca, cachegrind, iperf3 wrapper
```

## Conventions

- **No Python anywhere**; Rust only.
- **No public API on the engine**: `fds-core` is a binary package; every
  module is `pub(crate)`. The `fds` binary is the product. (The framework
  `mol-core` is a normal library with a public API and templates.)
- **Zero allocation in hot paths** (enforced by tests where it matters);
  structures are preallocated at startup.
- **Every `unsafe` block carries a `SAFETY:` comment**.
- **Bounds before indexing**: all network input is validated `&[u8]`.
- **Tests are fast**: the whole workspace suite runs in ~2 s; threaded
  stress tests are `#[ignore]`d and run serially (~2 s).

See `docs/engine.md` for the engine architecture and how to add a
transport handler; `docs/ops-tuning.md` for getting the machine to
deliver the numbers.
