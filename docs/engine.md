# fds-core engine — architecture and development guide

The engine is the flagship instantiation of the Mol framework (thesis
ch. 10: the reactor loop is a traced molecule; ch. 13: the standard
policies). It is a **binary package with no public API** — every module
is `pub(crate)`, the `fds` binary is the product, and tests live
in-module.

## Data flow

```
                     ┌────────────────────────────────────────────┐
   sockets (UDP/TCP) │  Reactor (epoll, edge-triggered)           │
      ────────events─▶│  poll_busy → copy_events (stack buffer)   │
                     │        │                                   │
                     │        ▼                                   │
                     │  drain_udp / drain_accept / drain_tcp      │
                     │  (each drains its fd to EAGAIN)            │
                     │        │                                   │
                     │        ▼                                   │
                     │  transports: recvmmsg/sendmmsg batches,    │
                     │  readv/writev, conn hot/cold state,        │
                     │  Ctx counters (packets/bytes/drops)        │
                     └────────────────────────────────────────────┘
                                     │  metrics listener is registered
                                     │  in the reactor (no per-loop syscalls)
                                     ▼
                          MetricsServer (Unix socket pull)
```

Loop invariants:

- **Edge-triggered + drain-to-EAGAIN**: after an event fires, the handler
  MUST drain the fd until the syscall returns `EAGAIN`, or no further
  edge is generated and events are lost. This is the engine's hard
  policy (standard [IO]).
- **Busy-poll**: `epoll_wait(timeout=0)` is called repeatedly until the
  ready list is empty. The engine burns CPU when idle — that is the
  latency contract (no wakeup latency); disable by setting
  `reactor.busy_poll=false` in config.
- **No allocation in the hot path**: receive buffers, event arrays,
  echo batches, and the connection table are preallocated at startup.
  The only per-connection allocation is the `HashMap` entry created at
  accept and removed at close.

## Module map

| Module | Responsibility |
|---|---|
| `main.rs` | Arg dispatch (`--bench`, `--latency`, `--latency-against`, `--fuzz`, else engine), config.json loading, async-signal-safe SIGINT |
| `engine.rs` | The loop: reactor wiring, UDP/TCP echo handlers, counters, metrics serving, core pinning |
| `reactor.rs` | epoll instance + preallocated event array; `register/modify/unregister`, `poll_timeout`, `poll_busy`, `copy_events` |
| `udp.rs` | Nonblocking IPv4 socket; `recv_batch`/`send_batch` (recvmmsg/sendmmsg, preallocated arrays; receive buffers const-generic, engine uses `MAX_DATAGRAM` = 64 KiB so any datagram arrives whole), GSO (UDP_SEGMENT), GRO, MSG_TRUNC, SO_INCOMING_CPU, MSG_ZEROCOPY (`send_to_zerocopy`) |
| `tcp.rs` | `TcpListener` (accept4 nonblocking, FASTOPEN/NODELAY/QUICKACK/DEFER_ACCEPT/CORK), `TcpStream` (read/readv/write_all/writev/splice_from_fd via a pipe — `splice(file→socket)` is EINVAL on Linux) |
| `sctp.rs` | libsctp FFI (declared in-crate against `netinet/sctp.h`; `sctp_recvmsg` returns `c_int`), send/recv with stream ids, peeloff, multi-homing via `sctp_bindx`; tests skip when the kernel module is absent |
| `conn.rs` | `HotState`/`ColdState` on own cache lines (false-sharing discipline), `ConnTable` (preallocated slots + lock-free free list), packed `ConnectionId` (core << 32 | slot) used as epoll tokens |
| `checksum.rs` | IP/TCP/UDP one's-complement via the framework SIMD; SCTP CRC32c (table-driven) |
| `parse.rs` | Bounds-safe IPv4/UDP/TCP header parsers as pure atoms; LCG property sweep |
| `metrics.rs` | Per-core padded counters (`CounterSet`), report formatting without allocation, Unix-socket pull server |
| `bench.rs` | `--bench` (throughput, echo), `--bench-large` (one-way byte-ceiling, per direction), `--latency` (transport RTT), `--latency-against` (engine RTT) |
| `fuzz.rs` | Deterministic xorshift64 harness over parsers/checksums (libFuzzer would need a public API — see below) |
| `config.rs` | config.json + `FDS_*` env overrides; all sections defaulted |
| `io_uring_reactor.rs` | Experimental io_uring path (feature `io-uring`, io-uring crate 0.7, SQPOLL fallback) |
| `af_xdp.rs` | Experimental AF_XDP path (feature `af-xdp`, UAPI declared in-crate, needs an XDP device at runtime) |

## Configuration

`config.json` is the sole runtime configuration source; every section has
a default. Environment overrides: `FDS_<SECTION>_<KEY>` (e.g.
`FDS_REACTOR_BUSY_POLL=0`, `FDS_ENGINE_UDP_BIND=0.0.0.0:7777`). See
`config.rs` for the full schema; `docs/ops-tuning.md` maps each knob to
the sysctl/ethtool counterpart.

## Adding a protocol handler (the "how to code" pattern)

The echo handlers are deliberately minimal placeholders — this is the
extension point:

1. **Bind** your socket(s) in `engine::run`, register them with
   `reactor.register(fd, token, Interest::Readable)` using a reserved
   token (see the `TOKEN_*` constants).
2. **Write a `drain_*` function** that loops until `EAGAIN`, pulls a
   batch, processes each item (parse → validate lengths → update the
   connection's hot state → produce output), and pushes output via the
   transport's batch send. Keep it allocation-free.
3. **Dispatch** the token in the `match ev.token` block.
4. For a connection-oriented protocol, acquire a `ConnTable` slot at
   accept, release it at close (`conns.release_slot`), and update
   `conns.conn_mut(slot).hot` per step.

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

Reference numbers on the dev laptop (i5-5200U, loopback, release):

| Measurement | p50 | p99 | p999 | max |
|---|---|---|---|---|
| transport RTT | 12µs | 30µs | 73µs | 2.4ms |
| engine RTT (pinned) | 13µs | 23µs | 43µs | 0.5ms |
| throughput (1400 B echo) | 114k pps, 152 MB/s (~1.2 Gbps) | | | |

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

- The engine loop wires the epoll reactor + UDP/TCP echo; SCTP/io_uring/
  AF_XDP are compiled, tested in-module, and startup-probed but not yet
  bound into the loop (an interim crate-scoped `#![allow(dead_code)]`
  documents this).
- TCP echo drops data on a write-`WouldBlock` instead of queueing (the
  production design is a per-connection send ring).
- The engine is single-core; per-core thread fan-out is the next wiring
  step (the `core.threads` config knob is reserved for it).
- `fuzz.rs` is the deterministic stable-rust harness; libFuzzer targets
  would need a public API (author ruling) and a nightly toolchain.
