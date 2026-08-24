# fds-core engine — architecture and development guide

The engine is the flagship instantiation of the Mol framework (thesis
ch. 10: the reactor loop is a traced molecule; ch. 13: the standard
policies). The crate is **lib + bin**: the library (`fds_core`) exposes
the transport primitives — [`reactor`], [`tcp`], [`udp`], [`conn`],
[`config`], [`metrics`], [`util`] — and the `fds` binary is the thin CLI
over the built-in echo engine (`engine::run`). The no-public-API ruling
held for the engine milestone; building consumers on the primitives
(Atomos's H1 server is the first) superseded it for the library surface.
Experimental paths (io_uring, AF_XDP) and the parser/checksum atoms stay
crate-private. Tests live in-module.

## Architecture: one worker per logical CPU

The engine runs **one worker thread per logical CPU** (`core.threads`;
0 = auto, which is 2x the physical core count on hyperthreaded
machines). Each worker owns, exclusively:

- its datapath — epoll edge-triggered busy-poll with the syscall
  transports by default, or the **io_uring completion-driven datapath**
  (recv/send/accept/read/write all through the ring) as the `io-uring`
  strategy;
- its own SO_REUSEPORT UDP socket + TCP listener on the shared bind
  addresses — the kernel steers flows across workers by 4-tuple hash;
- its own `ConnTable`, active-stream map, 64 KiB-datagram receive batch
  and per-core counters (the metrics `core.<i>.*` lines);

and pins itself to logical CPU `i` (`sched_setaffinity`) when
`core.pin_cores` is set. Nothing is shared except the metrics bundle
(padded per-core slots) and the stop flag — the standard's
shared-nothing, per-core recipe.

## Data flow (one worker)

```
                     ┌────────────────────────────────────────────┐
   sockets (UDP/TCP) │  epoll: wait → copy_events → drain_*       │
                     │  io_uring: completions → dispatch → resub  │
                     │        │                                   │
                     │        ▼                                   │
                     │  drain_udp / drain_accept / drain_tcp      │
                     │  (each drains its fd to EAGAIN)            │
                     │        │                                   │
                     │        ▼                                   │
                     │  transports: recvmmsg/sendmmsg batches,    │
                     │  readv/writev, conn hot/cold state,        │
                     │  per-core counters (Metrics.core[i])       │
                     └────────────────────────────────────────────┘
                                     │  metrics listener is registered
                                     │  on worker 0 only (no per-loop
                                     │  syscalls elsewhere)
                                     ▼
                          MetricsServer (Unix socket pull)
```

Loop invariants (each worker, epoll path):

- **Edge-triggered + drain-to-EAGAIN**: after an event fires, the
  handler MUST drain the fd until the syscall returns `EAGAIN`, or no
  further edge is generated and events are lost. This is the engine's
  hard policy (standard [IO]). The loop processes each poll batch fully
  before polling again, so no batch is ever dropped. The io_uring
  datapath keeps the same discipline: each slot/connection has exactly
  one op in flight, re-armed only after its handler finished.
- **Busy-poll**: `wait(timeout 0)` is called repeatedly until the ready
  list is empty. The engine burns CPU when idle — that is the latency
  contract (no wakeup latency); disable by setting
  `reactor.busy_poll=false` in config (`reactor.timeout_ms` then
  bounds the wait). The io_uring datapath busy-polls the completion
  queue the same way (`reactor.busy_poll` drives it).
- **No allocation in the hot path**: receive buffers, event arrays,
  echo batches, and the connection table are preallocated at startup.
  The only per-connection allocation is the `HashMap` entry created at
  accept and removed at close.

## Module map

| Module | Responsibility |
|---|---|
| `lib.rs` | The library surface: `pub mod` `reactor`, `tcp`, `udp`, `conn`, `config`, `metrics`, `util`, `engine`, `benchmarks`, `fuzz`; experimental paths (`io_uring_reactor`, `af_xdp`) and the parser/checksum atoms stay private |
| `main.rs` (bin) | Thin CLI over the library: arg dispatch (`--bench`, `--bench-large`, `--latency`, `--latency-against`, `--fuzz`, else engine), config.json loading |
| `engine.rs` | The built-in echo loop: worker fan-out per logical CPU, strategy dispatch (epoll loop vs io_uring datapath), UDP/TCP echo handlers, per-core counters, metrics serving, core pinning, AF_XDP forwarding thread; `run(&Config)` is public (the reference loop) |
| `reactor.rs` | `Reactor` (rustix epoll, edge-triggered, preallocated event array): `register/modify/unregister/poll_timeout/copy_events` — the epoll strategy's readiness source; `PollTimeout` re-exported so consumers build timeouts without rustix |
| `udp.rs` | Nonblocking IPv4 socket; `recv_batch`/`send_batch` (recvmmsg/sendmmsg, preallocated arrays; receive buffers const-generic, engine uses `MAX_DATAGRAM` = 64 KiB so any datagram arrives whole), GSO (UDP_SEGMENT), GRO, MSG_TRUNC, SO_INCOMING_CPU, MSG_ZEROCOPY (`send_to_zerocopy`); options are set **before bind** so SO_REUSEPORT group admission works |
| `tcp.rs` | `TcpListener` (IPv4+IPv6, accept4 nonblocking, options before bind, FASTOPEN/NODELAY/QUICKACK/DEFER_ACCEPT/CORK, `local_addr()` for port-0, backlog parameter), `TcpStream` (read/readv/write_all/writev/splice_from_fd via a pipe — `splice(file→socket)` is EINVAL on Linux) |
| `sctp.rs` | libsctp FFI (declared in-crate against `netinet/sctp.h`; `sctp_recvmsg` returns `c_int`), send/recv with stream ids, peeloff, multi-homing via `sctp_bindx`; tests skip when the kernel module is absent; engine path not yet bound |
| `conn.rs` | `HotState`/`ColdState` on own cache lines (false-sharing discipline), `ConnTable` (preallocated slots + lock-free free list, heap-backed arena via `mol::Pool`), packed `ConnectionId` (core << 32 | slot) used as poller tokens; slot guards held for the connection's lifetime (never double-release) |
| `checksum.rs` | IP/TCP/UDP one's-complement via the framework SIMD; SCTP CRC32c (table-driven) |
| `parse.rs` | Bounds-safe IPv4/UDP/TCP header parsers as pure atoms; LCG property sweep |
| `metrics.rs` | Per-core padded counters (`CounterSet`), `add_packets/bytes/drops(core, …)` from the workers, aggregate `.total` + per-core lines, Unix-socket pull server |
| `benchmarks.rs` | CLI tooling: `--bench` (throughput, echo), `--bench-large` (one-way byte-ceiling, per direction), `--latency` (transport RTT), `--latency-against` (engine RTT) |
| `fuzz.rs` | Deterministic xorshift64 harness over parsers/checksums (libFuzzer would need a public API — see below) |
| `config.rs` | config.json + `FDS_*` env overrides; all sections defaulted |
| `util.rs` | `pin_to_core` (sched_setaffinity), `now_ticks` (coarse monotonic seconds via vDSO — no clock syscall per packet) |
| `io_uring_reactor.rs` | `IoUringReactor` (feature `io-uring`, io-uring crate 0.7) + `IoUringDatapath`: the `io-uring` strategy's completion-driven UDP/TCP echo — IORING_OP_RECVMSG/SENDMSG per UDP slot, IORING_OP_ACCEPT/READ/WRITE per TCP connection, a periodic IORING_OP_TIMEOUT for idle wakeups; fds are blocking so ops wait in-kernel; SQPOLL with EPERM fallback |
| `af_xdp.rs` | Experimental AF_XDP path (feature `af-xdp`, UAPI declared in-crate): umem + rings + bind, plus `process_frame` — the Ethernet/IPv4/UDP validate-and-echo pipeline (parse + checksum atoms, MAC swap, TTL decrement, IP checksum recompute) unit-tested with synthetic frames; the engine runs a forwarding thread when `af_xdp.device` is configured and opens |

## Building on the primitives (consumers)

The library surface is the seam for application servers: `reactor` +
`tcp` + `conn` replace a hand-rolled epoll loop and socket setup, and
`config`/`metrics`/`util` provide the plumbing conventions. The first
consumer is **Atomos** (sibling repo): its H1 engine runs one FDS
`Reactor` per pinned worker, binds FDS `TcpListener`s (SO_REUSEPORT,
options before bind), stores per-connection HTTP state keyed by FDS
`ConnectionId` tokens in a `ConnTable`, and ports its read/parse/
dispatch/wire-cache state machine onto the drain-to-EAGAIN discipline.
The `ConnTable` slot-guard rule is the invariant consumers must keep:
hold the guard for the connection's lifetime and let its `Drop` release
the slot — calling `release_slot` while a guard is alive double-releases
the free-list ring.

## Configuration

`config.json` is the sole runtime configuration source; every section has
a default. Environment overrides: `FDS_<SECTION>_<KEY>` (e.g.
`FDS_REACTOR_BUSY_POLL=0`, `FDS_CORE_THREADS=4`,
`FDS_REACTOR_STRATEGY=io-uring`, `FDS_ENGINE_UDP_BIND=0.0.0.0:7777`,
`FDS_METRICS_SOCKET_PATH=/tmp/fds-metrics.sock`). See `config.rs` for the
full schema; `docs/ops-tuning.md` maps each knob to the sysctl/ethtool
counterpart.

Key knobs:

- `core.threads`: worker count; 0 = one per logical CPU (default).
- `core.pin_cores`: pin worker `i` to logical CPU `i` (default on).
- `reactor.strategy`: `epoll-busy-poll` (default, syscall transports)
  or `io-uring` — the completion-driven datapath where UDP/TCP echo
  runs entirely through the ring (falls back to epoll with a log line
  if io_uring is unavailable).
- `reactor.io_uring_entries` / `reactor.io_uring_sq_thread`: ring size
  (floored at 72; the initial 64 UDP recvs must fit) and SQPOLL CPU
  (0 = off; needs CAP_SYS_ADMIN).
- `af_xdp.device` / `af_xdp.queue`: bind an XDP device queue on a
  dedicated thread and run `process_frame` (validate + echo) on every
  frame (device-gated; absent hardware the engine logs and runs on the
  kernel datapath).
- `udp.incoming_cpu`: default **off** — on loopback it pins all traffic
  to one worker (see ops-tuning); enable only with NIC RSS/IRQ affinity.

## Adding a protocol handler (the "how to code" pattern)

The echo handlers are deliberately minimal placeholders — this is the
extension point:

1. **Bind** your socket(s) in `worker_main`, register them with
   `reactor.register(fd, token, Interest::Readable)` (epoll path) or
   add ops to the datapath's `dispatch` (io_uring path) using a
   reserved token (see the `TOKEN_*` constants).
2. **Write a `drain_*` function** (epoll) that loops until `EAGAIN`,
   pulls a batch, processes each item (parse → validate lengths →
   update the connection's hot state → produce output), and pushes
   output via the transport's batch send. Keep it allocation-free;
   count into `metrics.add_*(core, …)`.
3. **Dispatch** the token in the `match ev.token` block; in the io_uring
   datapath, keep exactly one op in flight per slot/connection and
   re-arm it from the completion handler.
4. For a connection-oriented protocol, acquire a `ConnTable` slot at
   accept and release it at close — in the epoll path hold the
   `ConnectionSlot` guard for the connection's lifetime; in the io_uring
   path use `acquire_index`/`release_slot` (the guard would release at
   accept and double-release at close).

Rules that keep the code consistent with the standard: bounds before
indexing; `SAFETY:` on every `unsafe`; no allocation per packet; tests
in-module with a fast default suite.

## Measurement

- `--bench <secs>`: UDP loopback throughput (pps / MB/s), 1400-byte
  datagrams echoed by a std peer — the realistic round-trip number.
- `--bench-large <datagram> <secs>`: one-way loopback throughput per
  direction (engine send → std drain, then std source → engine recv) with
  datagrams up to the IPv4 wire max (65507 B). This is the byte-ceiling
  measurement behind the "10–40+ Gbps loopback" claim.
- `--latency <secs>`: single-flight transport RTT (p50/p99/p999/max).
- `--latency-against <addr> <secs>`: RTT against a running engine —
  the end-to-end number the busy-poll + pinning target.
- `scripts/perf.sh`: `perf stat`/`record`, `cargo asm`, `llvm-mca`,
  `valgrind --tool=cachegrind`, `iperf3` commands.

Worker count is set with `FDS_CORE_THREADS` (0 = one per logical CPU,
the default). Verify per-worker distribution by pulling the metrics
endpoint while under load: all `core.<i>.packets` lines should be
non-zero (loopback steers per-flow; a single client flow lands on one
worker by design).

Reference numbers on the dev laptop (i5-5200U, 2C/4T, loopback,
release, 4 workers). The machine's tails are noisy under 5+ threads on
4 logical CPUs; the two strategies are at parity, so treat the spread
as scheduling noise, not a datapath difference:

| Measurement | p50 | p99 | p999 |
|---|---|---|---|
| engine RTT, epoll (pinned, busy-poll) | 12–21µs | 24–35µs | 55µs–5ms (noisy) |
| engine RTT, io_uring (pinned, busy-poll CQ) | 12–22µs | 22–30µs | 51µs–2.5ms (noisy) |
| throughput (1400 B echo, `--bench`) | 114k pps, 152 MB/s (~1.2 Gbps) | | |
| ping-pong, 4 clients → 4 workers | 123k pps | | |
| ping-pong, 8 clients → 4 workers | 153k pps | | |

The io_uring datapath's win is syscall amortization (one ring, batched
completions), not single-flight latency — on loopback both strategies
are parity because the kernel datapath dominates. Jumbo (60 KB)
datagrams round-trip intact through the ring (64 KiB slots).

On this 4-thread laptop the client threads (per-packet syscalls) are the
ping-pong ceiling; the per-core win shows as better tails and as headroom
on hosts with more cores.

Large-datagram throughput (one-way, per direction, 3 s):

| Datagram size | Path | Measured |
|---|---|---|
| 60 000 B | engine send (sendmmsg) → std drain | 32.3 Gbps (67 kpps) |
| 60 000 B | std sender → engine recv (recvmmsg) | 29.0 Gbps (60 kpps) |
| 60 000 B | std-only one-way probe (per-packet) | 27.7 Gbps (58 kpps) |
| 8 192 B | std-only one-way probe | 9.2 Gbps (141 kpps) |
| 1 400 B | std-only one-way probe | 2.1 Gbps (191 kpps) |
| 1 400 B | engine send (one-way) | 2.9 Gbps (258 kpps) |
| 64 B | std-only one-way probe | 0.11 Gbps (205 kpps) |

### Why `--bench` shows ~1.2 Gbps and not 10–40+ Gbps

Throughput is **packet rate × packet size**. Loopback's byte ceiling is
memory bandwidth (~30+ Gbps on this machine, verified above), but every
datagram costs at least one syscall on each side, and a per-packet
syscall on this CPU tops out around 200k pps. The quoted "10–40+ Gbps"
assumes large datagrams; the arithmetic:

- 1 400 B datagrams: 10 Gbps needs ~900k pps — unreachable with
  per-packet syscalls. `--bench` also pays a full echo (recv *and* send
  at the std peer, 2 syscalls per packet), which is why the round-trip
  number lands at ~114k pps ≈ 1.2 Gbps.
- 60 000 B datagrams: 30 Gbps needs only ~62k pps — trivially met, so
  the loopback shows its real memory-bound ceiling (29–32 Gbps).
- Batching (recvmmsg/sendmmsg) multiplies the syscall budget: the
  engine's batched send at 1 400 B does 258 kpps vs 191 kpps for the
  per-packet std probe — a 1.35× lift from syscall amortization alone.

So the engine datapath is not bandwidth-starved; it is packet-rate
starved for small datagrams, which is a kernel-syscall reality, and
bandwidth-bound for large datagrams, where it delivers the quoted
numbers. Production tricks to move more bytes at small packet sizes:
GSO/GRO (send one large buffer, let the kernel segment/coalesce), larger
datagrams at the application layer, and — for NIC traffic — AF_XDP
(compiled, device-gated; see `af_xdp.rs`).

## Known limitations

- The engine binds UDP + TCP echo; the SCTP transport is compiled,
  tested in-module and startup-probed but not yet bound into the worker
  loop (an interim crate-scoped `#![allow(dead_code)]` documents the
  remaining unwired items: SCTP, MSG_ZEROCOPY, registered buffers,
  splice, the io_uring transport-op helpers `submit_read`/`submit_write`
  — the datapath uses RECVMSG/SENDMSG/READ/WRITE ops of its own).
- AF_XDP's `process_frame` pipeline (validate + echo) is unit-tested
  with synthetic frames, but no XDP-capable device exists on the dev
  machine, so the umem/ring data path is compile- and unit-tested only,
  not exercised end-to-end here.
- The io_uring datapath is complete for the UDP + TCP echo workload
  (recv/send/accept/read/write through the ring, busy-pollable CQ).
  Remaining io_uring work: registered buffers, multishot recv, and
  real-NIC validation (SQPOLL needs CAP_SYS_ADMIN).
- TCP echo drops data on a write-`WouldBlock` instead of queueing (the
  production design is a per-connection send ring).
- `fuzz.rs` is the deterministic stable-rust harness; libFuzzer targets
  would need a public API (author ruling) and a nightly toolchain.
