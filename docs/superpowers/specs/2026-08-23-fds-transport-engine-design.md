# FDS Sub-Project 4: Transport Engine — Design Spec

**Date:** 2026-08-23
**Status:** Draft for review (written in advance per author request; design-approval loop still applies before implementation)
**Depends on:** Sub-project 1 (standard policies [IO], [SIMD], [CONC], [SEC], [OBS]), Sub-project 2 (framework: rings, buffers, memory, layout), Sub-project 3 (config.json, build flags)
**Framing:** The engine is the flagship instantiation of Mol: the reactor loop is a traced molecule (NT34–NT36), batching is operadic composition (NT46–NT47), connection state is a hybrid molecule, and every optimization is backed by a theorem or a standard decision matrix.

---

## 1. Purpose

The TCP/UDP/SCTP dataplane: nonblocking, edge-triggered, busy-polling, batched, zero-allocation I/O engineered for latency/throughput up to the silicon, per the original brief. Linux-only.

## 2. Deliverables

| # | Deliverable | Path |
|---|-------------|------|
| 1 | Reactor (edge-triggered epoll, busy-poll) | `crates/fds-transport/src/reactor.rs` |
| 2 | UDP transport (recvmmsg/sendmmsg, GSO/GRO, zero-copy) | `crates/fds-transport/src/udp.rs` |
| 3 | TCP transport (scatter-gather, sendfile/splice, options) | `crates/fds-transport/src/tcp.rs` |
| 4 | SCTP transport (control buffers, peeloff, multi-homing) | `crates/fds-transport/src/sctp.rs` |
| 5 | Connection/association state (hot/cold split) | `crates/fds-transport/src/conn.rs` |
| 6 | SIMD parsing/checksum atoms (bounds-safe) | `crates/fds-transport/src/parse.rs`, `checksum.rs` |
| 7 | Observability (lock-free per-core counters, pull metrics) | `crates/fds-transport/src/metrics.rs` |
| 8 | Benchmarks + profiling harness | `benches/`, `scripts/perf.sh` |
| 9 | Fuzz targets for parsers | `fuzz/` |
| 10 | Ops/system-tuning document | `docs/ops-tuning.md` |

## 3. Design

### 3.1 Reactor

- Nonblocking sockets (`O_NONBLOCK`; `accept4(..., SOCK_NONBLOCK)` — no separate fcntl).
- Edge-triggered epoll, `epoll_wait(..., timeout=0)` busy-poll; **drain to EAGAIN** on every ready fd (no lost events).
- Event arrays preallocated at startup; per-core reactors, one thread pinned per physical core (`core_affinity`); SO_REUSEPORT with one socket per core; SO_INCOMING_CPU steering; optional SO_ATTACH_REUSEPORT_CBPF/EBPF for custom balancing (config-gated).
- Batch amortization per NT46–NT47; prefetch hints for the next packet/connection while processing the current one; nontemporal stores (`_mm_stream_si128`) for data not reused soon; branch hints (`likely`/`unlikely`), `#[cold]` on error paths; branchless cmov patterns via `std::hint::black_box`; manual unrolling to expose ILP; interleaved independent operations to shorten dependency chains (standard [MOL], [CACHE]).
- Polling strategy selectable in config: epoll busy-poll (default) | io_uring SQPOLL (feature-gated) | AF_XDP (experimental, raw Ethernet — no TCP/UDP stack; documented loss) (D-5).

### 3.2 UDP

- `recvmmsg`/`sendmmsg` with preinitialized, reused `mmsghdr`/`iovec` arrays; `MSG_TRUNC` to detect oversized datagrams (never read past the buffer).
- UDP_SEGMENT (GSO) for NIC-segmented large sends; UDP_GRO to coalesce receives; SO_ZEROCOPY for large datagrams (feature-gated); SO_RCVBUF/SO_SNDBUF 4–16 MB (config).
- Batch ring between recvmmsg and processing is the framework's ring (NT48 invariant).

### 3.3 TCP

- `readv`/`writev` scatter-gather for partial reads/writes; `sendfile`/`splice` for file-backed responses (zero-copy, valid-fd discipline — no double-close; D-6).
- TCP_NODELAY (default on), TCP_QUICKACK, TCP_DEFER_ACCEPT, TCP_FASTOPEN (config-gated; spoofing caveat documented), TCP_CORK (opt-in; latency caveat).
- Connection state: hot fields and cold fields in separate cache lines (standard [CACHE]); per-core connection tables; send/receive rings lock-free.

### 3.4 SCTP

- `sctp_recvmsg`/`sctp_sendmsg` with preallocated ancillary control buffers; SCTP_NODELAY; SCTP_EVENTS for association up/down notifications; SCTP_INITMSG to preconfigure streams; SCTP_PARTIAL_DELIVERY_POINT; SCTP_MAX_BURST; SCTP_PEELOFF to move an association to a dedicated socket (per-association processing); `sctp_bindx` for multi-homing.
- Requires `libsctp` at build/runtime; availability detected by build tooling (sub-project 3); compile-time feature `sctp`.

### 3.5 SIMD atoms

- Checksums (IP/TCP/UDP/SCTP) and header parsing vectorized with the framework's bounds-safe SIMD helpers: lengths/alignment checked before vector loops; remainder handled with masks — **never out of bounds** (standard [SIMD]).
- Portable SIMD (`wide`) with `std::arch` native fallback; SoA for SIMD-heavy field transforms, AoS for per-packet processing; precomputed lookup tables for parsing; no division/modulo in hot paths; power-of-two sizes + bitmasks.
- Parser atoms are pure molecules (NT10 left factoring, NT28 fast-path specialization) — the protocol parsers are the paper's worked example.

### 3.6 Zero-copy & offloads (config-gated)

- Checksum offload, TSO/GSO, LRO/GRO enabled when the NIC supports them (runtime detection, documented in ops-tuning).
- io_uring with registered buffers (IORING_REGISTER_BUFFERS) as an optional path; MSG_ZEROCOPY for UDP large datagrams; splice for TCP static content.
- Jumbo frames (MTU 9000) and interrupt-coalescing tuning (ethtool `-C rx-usecs 0`) are **ops guidance** (docs/ops-tuning.md), not engine code.

### 3.7 Security (standard [SEC])

- Length validation before any indexing; `get` over direct indexing on untrusted input; all network input as `&[u8]`, never assume NUL-termination.
- MSG_TRUNC truncation detection on every datagram path; SO_LOCK_FILTER where BPF filters are attached.
- `std::thread::Builder` stack-size control; `setrlimit` memory caps at startup (config).
- TCP_MD5SIG/TCP_AO: niche, off by default, config-gated with the performance/spoofing trade-off documented.
- `cargo audit`/`cargo deny` in CI; clippy with all lints enabled; fuzz targets for all parsers.
- Parsers built as pure atoms with declared domains (standard [A]; thesis Ch. 14 testing: property tests + fuzz + differential).

### 3.8 Observability

- Lock-free per-core counters (padded atomics); pull-based metrics (Unix socket or HTTP `/metrics`); feature-gated tracing, zero-cost when compiled out (standard [OBS]).

## 4. Profiling & Tuning (harness, not gates)

`scripts/perf.sh` wraps: `perf stat -e cache-misses,cache-references,instructions,cycles,branch-misses`; `perf record -g` + `perf report`; `cargo asm` inspection of hot functions; `llvm-mca` ILP/port-pressure analysis; `valgrind --tool=cachegrind` cache simulation; heap-tracking zero-alloc verification (heaptrack or the framework's counting allocator); `iperf3` raw throughput. Benchmarks in `benches/`; cost-regression tests per thesis Ch. 14 (cost-enriched regression, NT25–NT27).

## 5. Constraints

- Linux-only; no Python; Rust only (Julia not needed).
- Conforms to standard policies [A], [MOL], [R], [ALLOC], [CACHE], [SIMD], [IO], [CONC], [SEC], [OBS], [TEST]; config.json from sub-project 3 is the sole runtime configuration source.
- Every optimization cites its theorem or decision matrix (NT47 batching, NT48 rings, NT52 zero-copy roundtrip, D-1…D-12).

## 6. Non-Goals

- Plugins/hot-swap runtime (later sub-project; engine exposes clean hook points per standard [PLUGIN]).
- AF_XDP as a production path (experimental only).
- Userspace TCP/IP stacks, kernel-bypass beyond the listed options.
- Portability beyond Linux.

## 7. Open Decision Points (for author)

1. io_uring vs epoll as the *default* reactor (epoll default recommended; io_uring feature-gated).
2. SCTP feature-gated behind `libsctp` presence — acceptable?
3. Metrics endpoint: Unix socket vs HTTP (no-alloc constraint favors Unix socket).
4. Whether AF_XDP ships at all in the first engine milestone (recommended: experimental flag only).

## 8. Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| Edge-triggered epoll missed events | Drain-to-EAGAIN discipline is a hard policy ([IO]); reactor tests with racy fd churn |
| Zero-copy lifetime bugs (registered buffers, splice fds) | Valid-fd discipline, single-owner fds, fuzz + sanitizer CI |
| FASTOPEN spoofing | Config-gated off-by-default with documented risk |
| SCTP availability | Build-time detection; feature flag; graceful absence |
| SIMD OOB | Bounds/alignment prechecks mandatory; UBSan + fuzz CI |
| Cache-line false sharing regressions | static_assertions on layout; perf cache-miss regression tests |
