# Publication validation

Validated on Linux 7.2.2 x86-64 with the repository's pinned Rust 1.97.1.
Miri used nightly Rust 1.100.0 (`1303417c4`, 2026-09-21) with strict
provenance. This records tested coverage, not a universal safety or
performance guarantee.

## Results

| Check | Result |
| --- | --- |
| Minimal workspace, debug and release | Pass |
| Full workspace including linked SCTP, debug and release | Pass |
| Each optional feature separately: SCTP, io_uring, AF_XDP | Pass |
| Ten consecutive full-feature suites after fixture repair | Pass |
| Twenty native threaded-ring stress runs | Pass |
| Miri: 13 laws, 9 safety cases, 5 ring cases, SPSC and MPMC threads, 5 frame-identity cases | Pass |
| AF_XDP copy-mode UMEM/ring setup and 128 TX-pool reuse cycles in isolated veth namespace | Pass |
| Parser/checksum fuzz smoke: 1,000,000 iterations | Pass |
| Eight-channel full-duplex example | Completes |
| Pinned-toolchain Clippy, rustdoc with warnings denied, and formatting | Pass |
| Shell syntax, build-wrapper argument/environment regressions | Pass |
| Actual pinned-CPU wrapper build and emitted compiler flags | Pass |
| Generated config schema matches checked-in schema; checked-in config validates | Pass |
| Two standalone C helper syntax checks with GCC | Pass |
| All six thesis verification binaries | Pass |
| Thesis compile, bibliography/reference/style gates, 68 theorem labels | Pass, 113 pages |
| cargo-audit 0.22.2: lockfile's 28 dependencies | No vulnerabilities or informational warnings |

SCTP was tested against locally built lksctp-tools **v1.0.21**, upstream
commit `37d5f1225573b91d706a5e547d081f79963a9deb`; the kernel reports SCTP
support and the actual SCTP roundtrip/throughput tests passed. Nothing was
installed into system library directories. Use normal `libsctp-dev` on
CI or a developer system; a custom library prefix needs `RUSTFLAGS=-L
native=<prefix>/lib` and `LD_LIBRARY_PATH=<prefix>/lib` for tests.

The RustSec database contained 1,277 advisories at commit
`9b3a3b73a7f42606494c943e95f8196e9994df46`. This audit does not cover native
libsctp, the compiler, the kernel, or future advisory updates.

## Issues found while testing

- Repeated full-feature runs exposed a flaky API fixture that set its
  synchronous TCP peer nonblocking and later called `read_exact` as if a
  read timeout restored blocking behavior. The fixture now stays blocking
  with bounded I/O timeouts; reactor-driven streams remain nonblocking.
- Target-specific user Cargo configuration could override generic
  `build.rustflags`, undermining the wrapper's pinned CPU. The wrapper
  now injects encoded flags when no explicit caller flag environment is
  present; caller overrides remain visible and authoritative.
- The compiler-cache daemon stalled uncached builds on this host. Those
  builds were rerun with `RUSTC_WRAPPER=`; no shared daemon was stopped.
- The original large MPMC workload exceeded the Miri time budget. Miri
  uses 100 items per producer and an 8-slot queue to retain contention and
  wraparound. Native tests still transfer 10,000 items with a 1,024-slot
  queue. Both runs passed; a timeout was not counted as a pass.

CI now includes minimal/full builds, an isolated AF_XDP setup smoke test,
strict-provenance Miri checks, and the six dependency-free thesis verifiers.
The local thesis build used a temporary copy so historical logs/PDFs were
not overwritten.

## Limits

- No real-NIC AF_XDP RX/TX or native zero-copy throughput was exercised.
  The veth test deliberately checks copy-mode setup and userspace-owned
  TX buffers without attaching an XDP steering program.
- No privileged NUMA-placement or full protocol-interop campaign ran.
- Destructive legacy comparison scripts were not executed against the
  host. Their shell syntax was checked; use isolated disposable systems.
- cargo-deny license policy was not run; the repository does not yet
  contain a project-specific deny policy.
- Neither fuzz smoke nor Miri covers every input, interleaving, kernel
  behavior, or lifecycle. Remaining risks are in [code-review.md](code-review.md).
