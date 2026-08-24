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
| `udp.rs` | Nonblocking IPv4 socket; `recv_batch`/`send_batch` (recvmmsg/sendmmsg, preallocated arrays), GSO (UDP_SEGMENT), GRO, MSG_TRUNC, SO_INCOMING_CPU, MSG_ZEROCOPY (`send_to_zerocopy`) |
| `tcp.rs` | `TcpListener` (accept4 nonblocking, FASTOPEN/NODELAY/QUICKACK/DEFER_ACCEPT/CORK), `TcpStream` (read/readv/write_all/writev/splice_from_fd via a pipe — `splice(file→socket)` is EINVAL on Linux) |
| `sctp.rs` | libsctp FFI (declared in-crate against `netinet/sctp.h`; `sctp_recvmsg` returns `c_int`), send/recv with stream ids, peeloff, multi-homing via `sctp_bindx`; tests skip when the kernel module is absent |
| `conn.rs` | `HotState`/`ColdState` on own cache lines (false-sharing discipline), `ConnTable` (preallocated slots + lock-free free list), packed `ConnectionId` (core << 32 | slot) used as epoll tokens |
| `checksum.rs` | IP/TCP/UDP one's-complement via the framework SIMD; SCTP CRC32c (table-driven) |
| `parse.rs` | Bounds-safe IPv4/UDP/TCP header parsers as pure atoms; LCG property sweep |
| `metrics.rs` | Per-core padded counters (`CounterSet`), report formatting without allocation, Unix-socket pull server |
| `bench.rs` | `--bench` (throughput), `--latency` (transport RTT), `--latency-against` (engine RTT) |
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

- `--bench <secs>`: UDP loopback throughput (pps / MB/s), batched.
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
| throughput | 107k pps (dev) / 127k pps (release), 143–169 MB/s |

The datapath is syscall/kernel-bound on loopback; the busy-poll reactor
removes wakeup latency and core pinning removes migration stalls. True
sub-µs paths require AF_XDP/io_uring on real NIC hardware (compiled,
device-gated).

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
