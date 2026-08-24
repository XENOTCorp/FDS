# FDS Sub-Project 3: Build Tooling + Config — Design Spec

**Date:** 2026-08-23
**Status:** Implemented 2026-08-24 (author ruling: "impl directly"; config.json = single repo-root file). Deliverables: `build/build.sh` + `build/detect.sh`, `crates/fds-detect/`, `config/config.schema.json`, repo-root `config.json`, `build/PROFILES.md`. Scope notes: §4.3's wider schema sections were superseded by the engine's shipped config surface (see PROFILES.md "Scope note").
**Depends on:** Sub-project 1 (standard's decision matrices D-1…D-12 as the default-selection rationale)
**Consumed by:** Sub-project 4 (transport engine reads config.json)

---

## 1. Purpose

Two artifacts that make FDS builds hardware-tailored and every runtime knob explicit:

1. **`build.sh`** — a bash build script that detects the current device's specs and modifies cargo/RUSTFLAGS arguments accordingly (CPU microarchitecture → `target-cpu`, SIMD features → `target_feature` sets, L3 cache size → ring/buffer sizing defaults, core count → worker defaults, hugepage availability), while still allowing fully custom flags and explicit overrides.
2. **`config.json`** — a single configuration file for all runtime settings, with an explanation and trade-off note for every parameter, defaults derived from detected hardware, and full overridability.

## 2. Deliverables

| # | Deliverable | Path |
|---|-------------|------|
| 1 | Hardware-adaptive build script | `build/build.sh` |
| 2 | Runtime configuration schema + example | `config/config.schema.json`, `config/config.example.json` |
| 3 | Defaults generator (Rust tool, no Python) | `crates/fds-detect/` (emits detected-hardware summary + suggested defaults) |
| 4 | Build profiles & flag matrix documentation | `build/PROFILES.md` |
| 5 | Dependency security integration | `build.sh --check-deps` (cargo audit/deny) |

## 3. Repo Layout (additions)

```
FDS/
├── build/
│   ├── build.sh                # detection → flags → cargo invocation
│   ├── PROFILES.md             # debug/release flag matrix + rationale
│   └── detect.sh               # sourced by build.sh; lscpu//proc/cpuinfo parsing
├── config/
│   ├── config.schema.json      # JSON Schema for config.json
│   └── config.example.json     # generated-from-hardware example
├── crates/fds-detect/          # Rust tool: hardware summary + suggested defaults
└── .cargo/config.toml          # portable baseline (from sub-project 2)
```

## 4. Design

### 4.1 Detection (precedence: explicit > config > detected defaults)

`build.sh` sources `detect.sh`, which reads, without external tool dependencies beyond what the system ships (lscpu, /proc/cpuinfo, sysfs):

- **CPU vendor/model** → `target-cpu=native` by default; explicit `TARGET_CPU` env/arg to pin a specific uarch (e.g., for cross-machine reproducibility).
- **SIMD features** (avx2, avx512f/bw/dq, sse4.2, etc.) → `target_feature` sets in RUSTFLAGS; the standard's [SIMD] policy governs fallbacks.
- **L3 cache size** (lscpu `L3 cache`) → default ring/buffer sizing via decision matrix D-1 (working set targets a fraction of L3).
- **Core count / topology** (physical vs logical, `core_affinity`-compatible listing) → default worker/thread counts.
- **Hugepage availability** (`/proc/meminfo` HugePages_Total, mount check) → default hugepage buffer policy with fallback (D-11).
- **NUMA topology** (lscpu -p) → default NUMA-aware allocation hints.
- Detection summary is always printed; every detected default is overridable.

### 4.2 Flag matrix (baseline; matches sub-project 2's `.cargo/config.toml`)

| Flag | debug | release |
|------|-------|---------|
| opt-level | 0 | 3 |
| target-cpu | (baseline) | native (adaptive) |
| lto | off | fat |
| codegen-units | 16 | 1 |
| panic | unwind | abort |
| overflow-checks | **on** | off |
| debug-assertions | **on** | off |
| relocation-model | pic | pic |
| static linking | — | on (target-dependent, e.g., `crt-static`) |

`PROFILES.md` documents each flag and its trade-off. Custom flags: `--profile`, `--features`, `RUSTFLAGS_EXTRA`, `TARGET_CPU`, per-flag overrides via env or `config.json` (build section).

### 4.3 config.json (all settings, explained)

Sections (every parameter carries `description`, `trade_off`, `default`, and `derived_from`):

- **build**: profile, target-cpu, feature flags, RUSTFLAGS_EXTRA.
- **rings**: capacities (power-of-two), per-ring sizing, in-flight caps (NT48), L3-derived defaults (D-1).
- **buffers**: pool sizes, packet buffer size, hugepage on/off (D-11), alignment.
- **cores**: worker count, affinity policy (pin-per-core), NUMA node assignment.
- **protocols**: enable/disable tcp, udp, sctp; per-protocol socket options (see 4.4) with latency/throughput trade-offs.
- **poll**: strategy (epoll busy-poll / io_uring SQPOLL / AF_XDP experimental) (D-5), batch sizes (D-4, NT47), timeout.
- **zero_copy**: io_uring registered buffers, MSG_ZEROCOPY, splice/sendfile toggles (D-6, NT52).
- **security**: rlimits (memory caps, stack sizes), MSG_TRUNC enforcement, TCP_MD5SIG/AO (niche, off by default).
- **observability**: metrics interval, per-core counters on/off (standard [OBS]).
- **plugins**: reserved section (later sub-project; ABI versioning flags per standard [PLUGIN]).

`config.schema.json` validates any config.json; `fds-detect` emits a `config.example.json` from detected hardware; the transport engine (sub-project 4) consumes the validated config at startup and preallocates everything per [R]/[ALLOC].

### 4.4 Socket-option catalogs (data source for protocol config)

Captured from the standard's non-normative implementation appendix and the original brief, each option with its effect and trade-off:

- UDP: SO_RCVBUF/SO_SNDBUF (4–16 MB), UDP_SEGMENT (GSO), UDP_GRO, SO_ZEROCOPY.
- TCP: TCP_NODELAY, TCP_QUICKACK, TCP_DEFER_ACCEPT, TCP_FASTOPEN (spoofing caveat), TCP_CORK (latency caveat).
- SCTP: SCTP_NODELAY, SCTP_EVENTS, SCTP_INITMSG, SCTP_PARTIAL_DELIVERY_POINT, SCTP_MAX_BURST, SCTP_PEELOFF, sctp_bindx.
- Threading: SO_REUSEPORT, SO_INCOMING_CPU, SO_ATTACH_REUSEPORT_CBPF/EBPF; system guidance (isolcpus/nohz_full/rcu_nocbs, IRQ smp_affinity steering, ethtool queue/coalescing, kernel.numa_balancing=0) lives in an ops document, not forced by the script.

### 4.5 Dependency security

`build.sh --check-deps` runs `cargo audit` and `cargo deny` (standard [SEC]); optional in normal builds.

## 5. Constraints

- No Python; detection/parsing in bash (detect.sh) and Rust (fds-detect).
- Deterministic: same machine + same inputs → same flags; detection summary logged.
- Everything detected is overridable; explicit overrides always win.
- Defaults follow the standard's decision matrices D-1…D-12 and the theorem rationales (NT47 batch sizing, NT48 ring invariant, NT52 zero-copy).

## 6. Non-Goals

- The engine itself (sub-project 4).
- Runtime adaptation beyond startup-time config (config is read once at startup; live reload is a later feature, standard [OBS]/[PLUGIN] pending).
- Shipping a full distro packaging story.

## 7. Open Decision Points (for author)

1. Minimum bash version / portability target (Linux-only is assumed; bash 4+?).
2. Should `build.sh` also orchestrate the thesis/standard doc builds (paper + verify tools), or stay app-only?
3. config.json location convention: repo root `config.json` vs `config/` dir; single file vs layered (base + machine override).

## 8. Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| lscpu output varies across distros | Parse /proc/cpuinfo + sysfs as fallback paths in detect.sh; fds-detect cross-checks |
| target-cpu=native hurts portability | Explicit TARGET_CPU pin for reproducible cross-machine builds |
| Hugepage-dependent defaults fail at runtime | Detection + fallback; config override; D-11 |
| Config sprawl (over-configuration) | Every param must have a trade-off note and a decision-matrix origin, else it is dropped (standard: no overengineering) |
