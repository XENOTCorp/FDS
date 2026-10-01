# Build profiles and flags

Three layers compose the final build flags, lowest to highest precedence:

1. **Workspace baseline:** `Cargo.toml` `[profile.*]` (portable: opt-level, LTO,
   panic, overflow-checks, codegen-units). This file is the contract for every
   machine; it is deliberately *not* machine-specific.
2. **Cargo configuration:** user and project config files may supply flags
   and linker choices. More local configuration takes precedence.
3. **Build wrapper:** `build/build.sh` reports hardware and supplies
   `CARGO_ENCODED_RUSTFLAGS`, overriding both generic and target-specific
   Cargo config flags. `build.rustflags` is also printed for inspection.

Explicit caller `RUSTFLAGS` or `CARGO_ENCODED_RUSTFLAGS` environments remain
authoritative and produce a warning; unset them for wrapper-controlled
builds. Generic `build.rustflags` alone is insufficient because Cargo
prefers `target.<triple>.rustflags`, even from a user's global config.

## Flag matrix

| Flag | debug | release | Adaptive? | Effect / trade-off |
|------|-------|---------|-----------|--------------------|
| `opt-level` | 1 (own), 3 (deps) | 3 | no | 0 keeps debug symbols/stepping; 3 is the silicon target. Deps at 3 in dev: compiled once, cached. |
| `target-cpu` | `native` with wrapper | `native` with wrapper | **yes** | `native` targets the build host. Pin `TARGET_CPU=haswell` or `x86-64-v3` for a fixed CPU baseline. Plain Cargo uses its configured baseline. |
| `target-feature` | CPU baseline | CPU baseline | no | The compiler derives features from `target-cpu`; pinned builds never add detected host features. Explicit extra flags remain the caller's responsibility. |
| `lto` | off | fat | no | fat LTO across crates at release; slows the build, best codegen. |
| `codegen-units` | 16 | 1 | no | 1 unit = whole-crate optimization; 16 = fast parallel debug builds. |
| `panic` | unwind | abort | no | abort shrinks the binary and removes landing pads from the hot path; the test profile always unwinds (cargo forces it). |
| `overflow-checks` | on | off | no | On in debug: silent wraparound is the classic buffer bug; off at release is the size/speed trade (the dataplane validates lengths explicitly, see `parse.rs`). |
| `debug-assertions` | on | off | no | On in debug for invariant checks (e.g. ring indices); off at release. |
| `relocation-model` | target default | target default | no | Cargo/rustc target defaults apply unless overridden by user configuration. |
| `RUSTFLAGS_EXTRA` | env | env | **yes** | Anything extra, appended verbatim (space-separated). Highest override knob. |

The matrix is the decision-matrix baseline from sub-project 2's design spec,
updated for the adaptive layer; `PROFILES.md` documents each flag's trade-off so
no flag is set "because it is usually faster" (standard IO-04).

## Decision origins for config.json defaults

Every knob in `config.json` carries `x-derived-from` in
`config/config.schema.json`. The hardware-derived ones:

| Default | Origin | Rule |
|---------|--------|------|
| `udp.rcvbuf`/`sndbuf`, `tcp.rcvbuf`/`sndbuf` | D-1 (ring/buffer sizing, L3-aware) | `clamp(pow2(L3/2), 4 MiB, 16 MiB)`: a socket buffer absorbs one L3-sized burst while the working set stays cache-resident. Power of two keeps the kernel's reported (doubled) value and the ring layout aligned (the ring-capacity invariant). |
| `core.threads` = 0 | engine default ([CONC]) | 0 = one worker per logical CPU (2x physical on SMT); the default is already hardware-adaptive. |
| `reactor.strategy` | D-5 (polling strategy) | Moderate packet rate + small syscall share → readiness loop (`epoll-busy-poll`); high rate where syscall amortization pays → kernel-side batching (`io-uring`); extreme rate + zero-copy + a dedicated core → zero-copy kernel ring (AF_XDP, `af_xdp.device`). |
| Allocation/zero-allocation | D-11 (allocation policy) | The hot path never allocates (ALLOC-01/02); buffers are preallocated at startup; enforced by the zero-alloc test in mol. |
| Batch sizes, in-flight caps | D-4 (batch size, syscall amortization) | UDP recvmmsg slots default to 4 on this CPU (D-1: 4 × 60 KiB fits L2; a 64-slot 60 KiB batch misses L3). Override `FDS_UDP_RX_SLOTS`. Other ring capacities follow the occupancy bound; the observable epoll knob is `reactor.max_events`. |

## Workflow

```sh
build/build.sh --summary          # detection facts (deterministic)
build/build.sh --release          # adaptive release build
build/build.sh --release --emit-config   # refresh Code/config.json from hardware, then build
TARGET_CPU=haswell build/build.sh --release   # portable/pinned build
build/build.sh --check-deps       # cargo audit + cargo deny ([SEC]); installs:
                                  #   cargo install cargo-audit cargo-deny
```

After regenerating `config.json`, validate it:

```sh
cargo run -p fds-detect -- --validate-config config.json
```

## Scope note

The sub-project 3 design spec's §4.3 sketched a wider config surface (rings,
buffers, security, observability, plugins sections). Sub-project 4 shipped the
engine first with a minimal runtime module (`config.rs`), so the schema here is
the **engine's actual contract**; every field the engine reads, with its
rationale. Speculative sections were dropped rather than shipped dead: the
standard's no-overengineering rule. Ring/buffer sizing decisions surface as the
D-1 socket-buffer defaults above and as engine constants (the ring-capacity invariant), documented in
`Docs/wiki/`.
