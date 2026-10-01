# Code review and follow-up ledger

This pass combines source review, repository-wide risk-pattern searches,
compile/lint checks, and executable regressions. It is **not** a proof of
safety, an exhaustive line-by-line sign-off, or a performance ranking.
The prior working-tree cleanup is preserved; no benchmark snapshots or
research documents were deleted.

## Coverage

| Area | Review / validation |
| --- | --- |
| `mol` production modules | Ownership, initialization, cleanup, non-Copy payloads, arithmetic, SIMD bounds, combinator contracts; unit/law/layout/stress/allocation tests |
| TCP/UDP, reactor, public API | Syscall semantics, short I/O, error paths, descriptor handling, readiness flags/timeouts/generations; loopback and socketpair regressions |
| Engine and CLI | Startup/shutdown, affinity, TCP backpressure, buffer reuse, config/argument failures; bounded shutdown and byte-integrity tests |
| io_uring | Submission contracts, stable kernel pointers, lifecycle transitions, ordered sends, pool allocation and completion processing; selected/forced-legacy flood tests |
| AF_XDP | Frame checkout identity, ring/frame sizing, NUMA bit count, header/checksum validation; hardware-independent tests and capability-dependent setup probes |
| Detection/config tooling | Integer bounds, cache-size overflow, generated-schema consistency; tests and schema regeneration |
| Scripts/CI/docs | Shell syntax, dangerous cleanup searches, primary comparison-runner PID isolation, CI feature coverage, current-vs-historical claims |

Legacy comparison scripts and C probes received risk scans, not complete
runtime or privileged validation. The thesis verification binaries and
historical evidence are outside this implementation review.

## Main corrections

1. TCP vectored I/O no longer advances past partly processed buffers;
   vectored writes suppress SIGPIPE. DEFER_ACCEPT belongs on the listener.
2. Epoll TCP echo preserves an unsent suffix and pauses reads. Each
   connection allocates one reusable 64 KiB buffer at setup rather than
   clearing a stack buffer per read. Interest changes occur only when
   entering/leaving write backpressure.
3. Worker exit, startup failure, and panic stop peer workers; all spawned
   threads are joined. CPU placement reads topology once and honors
   noncontiguous affinity masks.
4. io_uring readiness uses private request IDs, real readiness flags,
   bounded waits, and reused completion/event storage. Cancel CQEs and
   stale generations cannot be delivered under a recycled application token.
5. Unused linear pending-token tracking was removed. Debug configuration
   is cached. The completion buffer matches actual CQ capacity. Legacy
   mode does not retain an unused 4 MiB registered arena.
6. Kernel-referenced timeout/accept-length values have stable boxed
   addresses; datapaths cannot rearm an already-used instance. Modern
   sends are serialized per stream to preserve byte order, including
   short-send tails. Live-mode switching now fails closed.
7. AF_XDP validates socket identity and checkout generation before
   mutable access or release. TX completion counting no longer stores
   every address in a dynamically growing queue. Frame/ring limits and
   the NUMA nodemask bit count were corrected.
8. UDP zero-copy echo was removed: inclusive send-ID notification ranges
   cannot be replaced by notification counts or timeout heuristics. Raw
   zero-copy sends now require an unsafe lifetime promise; tests no longer
   mutate in-flight pages.
9. IPv4 fragments without reassembly and IPv6 UDP zero checksums/payload
   overruns are rejected. Computed IPv4 UDP checksum zero is sent as FFFF.
   The TCP prototype validates checksums/IP options, bounds unacknowledged
   buffering, and caps queued retransmissions.
10. Generic zero initialization is unsafe; mapping/size arithmetic and
    large-input checksum carries are checked. Combinators support moved
    non-Copy payloads without claiming compiler-enforced purity.
11. Metrics does not delete arbitrary files or active sockets, checks
    pathname identity on teardown, and reuses report formatting storage.
12. Configuration, CLI, documentation, shell checks, and generated schema
    were tightened. The primary backend comparison kills only its tracked
    child PID, uses exclusive single-worker ports, and bounds client runs.

Breaking changes and adaptation examples: [migration.md](migration.md).

## Validation

The initial review used Rust 1.98.0. The publication validation now also
passes on the pinned **Rust 1.97.1**, including linked SCTP tests with a
locally built lksctp-tools v1.0.21 library. See [validation.md](validation.md)
for the extended matrix, repeat runs, Miri checks, and coverage limits.
From `Code/`:

```sh
cargo +stable test --workspace --locked --no-default-features
cargo +stable test --workspace --release --locked --no-default-features
cargo +stable test --workspace --locked --no-default-features --features io-uring,af-xdp
cargo +stable test --workspace --release --locked --no-default-features --features io-uring,af-xdp
cargo +stable test -p mol --locked -- --ignored --test-threads=1
cargo +stable clippy --workspace --all-targets --all-features --locked -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo +stable doc --workspace --no-deps --all-features --locked
cargo +stable fmt --all --check
for script in build/*.sh scripts/*.sh; do bash -n "$script"; done
bash build/test-build.sh
```

SCTP runtime/link tests now pass, including real loopback throughput and
negative C-int error returns. AF_XDP copy-mode UMEM/ring setup and TX-pool
reuse pass on a veth in an isolated user/network namespace. Real-NIC RX/TX
zero-copy, privileged NUMA placement, and a new comparative performance
campaign remain unvalidated. Allocation regressions cover specified paths,
not every runtime error/cancellation combination.

## Remaining work before production claims

- **High priority:** model-check io_uring close/error/half-close and buffer
  exhaustion lifecycles. Add peer half-close, runtime opcode rejection,
  cancellation-race, SQ/CQ saturation, and connection-slot-reuse tests.
  Remove obsolete SEND_ZC notification bookkeeping only with state tests.
- **High priority:** the userspace TCP prototype is not an RFC-complete
  implementation. It still needs negotiated windows/MSS, congestion
  control, handshake retransmission, FIN/RST, sequence validation,
  out-of-order buffering, and interop testing. It allocates per packet.
- UDP GRO delivery needs ancillary segment-size handling before the echo
  path can preserve datagram boundaries for coalesced packets.
- Bound handler work and reschedule edge-triggered readiness fairly under
  sustained floods; current drain-to-EAGAIN loops can monopolize a worker.
- Metrics large/slow-client responses need queued nonblocking writes;
  report-buffer reuse alone does not guarantee complete delivery. Use a
  service-owned socket directory to avoid adversarial pathname races.
- Make environment-value parsing consistently reject malformed input;
  some legacy overrides still ignore invalid values or partially parse lists.
- Legacy benchmark scripts still contain process-name cleanup and
  machine-specific paths/privileged commands. Use disposable isolated
  environments until those runners adopt scoped cleanup and namespaces.
- Re-run reproducible performance comparisons after correctness changes.
  Serializing stream writes and validating frame generations have costs;
  old throughput snapshots are not current-version claims.
