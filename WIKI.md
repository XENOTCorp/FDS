# FDS wiki

## Table of contents

1. [Overview](#overview)
2. [Architecture](#architecture)
3. [Datapaths](#datapaths)
4. [Features](#features)
5. [Configuration](#configuration)
6. [Operations tuning](#operations-tuning)
7. [Implementation examples](#implementation-examples)
8. [Building applications](#building-applications)

## Overview

FDS is a Linux network engine for TCP, UDP, and SCTP in Rust. The
engine runs one worker thread per logical CPU. Each worker owns its
poller, its sockets, its connection table, and its counters. Nothing is
shared except a metrics bundle. The hot path performs no allocation and
no per-packet syscall beyond the batched transport.

The library surface is `fds-core`. The `fds` binary is a thin command
line over the built-in echo engine. Applications build their own loops
on the primitives: `reactor`, `tcp`, `udp`, `conn`, `config`, `metrics`,
`util`. Atomos, an HTTP server, is built this way.

## Architecture

### One worker per logical CPU

Each worker owns, exclusively:

- its datapath: epoll edge-triggered busy-poll with the syscall
  transports by default, or the io_uring completion-driven datapath;
- its own SO_REUSEPORT UDP socket and TCP listener on the shared bind
  addresses. The kernel steers flows across workers by 4-tuple hash;
- its own connection table, active-stream map, 64 KiB datagram receive
  batch, and per-core counters.

The worker pins itself to its logical CPU with `sched_setaffinity`
when `core.pin_cores` is set.

### Data flow (one worker)

```
sockets (UDP/TCP)  ->  epoll: wait, copy events, drain handlers
                          |
                          v
                  drain_udp / drain_accept / drain_tcp
                  (each drains its fd until EAGAIN)
                          |
                          v
                  transports: recvmmsg/sendmmsg batches,
                  readv/writev, connection hot/cold state,
                  per-core counters
```

The metrics listener is registered on worker 0 only. Metrics are pulled
over a Unix socket.

### Loop invariants

- Edge-triggered with drain to EAGAIN. After an event fires, the
  handler must drain the fd until the syscall returns EAGAIN, or no
  further edge is generated and events are lost. The loop processes
  each poll batch fully before polling again.
- Busy-poll: `wait(timeout 0)` repeats until the ready list is empty.
  The engine burns CPU when idle; that is the latency contract. Set
  `reactor.busy_poll=false` to disable.
- No allocation in the hot path. Receive buffers, event arrays, echo
  batches, and the connection table are preallocated at startup. The
  only per-connection allocation is the connection-map entry created
  at accept and removed at close.

### Module map

| Module | Responsibility |
| --- | --- |
| `reactor` | rustix epoll, edge-triggered, preallocated event array: `register`, `modify`, `unregister`, `poll_timeout`, `poll_once`, `copy_events` |
| `tcp` | nonblocking `TcpListener` (accept4, options before bind, FASTOPEN/NODELAY/QUICKACK/DEFER_ACCEPT/CORK) and `TcpStream` (read, readv, write_all, writev, splice) |
| `udp` | nonblocking IPv4 socket: `recv_batch`/`send_batch` (recvmmsg/sendmmsg), GSO, GRO, MSG_TRUNC, SO_INCOMING_CPU, MSG_ZEROCOPY |
| `sctp` | libsctp FFI: send/recv with stream ids, peeloff, multi-homing |
| `conn` | hot/cold state on separate cache lines, preallocated `ConnTable` with a lock-free free list, packed `ConnectionId` tokens |
| `checksum` | one's-complement checksums via SIMD; SCTP CRC32c |
| `parse` | bounds-safe IPv4/UDP/TCP header parsers |
| `metrics` | per-core padded counters, Unix-socket pull server |
| `config` | `config.json` plus `FDS_*` environment overrides |
| `util` | thread pinning, coarse monotonic ticks (vDSO, no clock syscall per packet) |
| `engine` | the reference loop: worker fan-out, strategy dispatch, echo handlers, metrics serving, core pinning, AF_XDP forwarding thread |
| `benchmarks` | `--bench`, `--bench-large`, `--latency`, `--latency-against` |
| `fuzz` | deterministic harness over parsers and checksums |

Experimental paths stay crate-private: the io_uring reactor (feature
`io-uring`) and the AF_XDP frame pipeline (feature `af-xdp`).

## Datapaths

### Kernel datapath (default)

The engine runs on the kernel socket path: epoll readiness, recvmmsg
and sendmmsg batches of 64 datagrams, readv and writev on TCP. This is
the default and the measured fastest strategy on this machine
(BENCHMARKS.md, reactor table).

### io_uring

The `io-uring` reactor runs UDP and TCP echo through the ring
(IORING_OP_RECVMSG/SENDMSG/ACCEPT/READ/WRITE). Measured on this kernel:
io_uring matches epoll on UDP and stalls on TCP; SQPOLL loses on 2
physical cores. The startup autotuner selects the strategy per machine.
On server hardware, register files and buffers and enable multishot
receive; the config keys are `FDS_REACTOR_IO_URING_ENTRIES` and
`FDS_REACTOR_IO_URING_SQ_THREAD`.

### AF_XDP

The `af-xdp` path implements a frame pipeline on an AF_XDP socket:
umem, rx/tx/fill/completion rings, bind, and a validate-and-echo
`process_frame` (Ethernet/IPv4/UDP parse, checksums, MAC swap, TTL
decrement). The receive path is proven end to end on a veth pair with a
driver-mode XDP redirect program. Transmit requires a NIC with an XDP
queue (ixgbe, i40e, ice, mlx5). On a machine with a supported NIC:
`af_xdp.device` and `af_xdp.queue` in `config.json` start a dedicated
forwarding thread.

### DPDK

DPDK is the fallback when the target NIC lacks XDP. It requires
hugepages, UIO/VFIO, and binding the device with `dpdk-devbind.py`.
AF_XDP covers the same ground on a stock kernel.

## Features

- TCP, UDP, and SCTP transports on one epoll reactor
- One worker per logical CPU, pinned, with SO_REUSEPORT flow steering
- recvmmsg and sendmmsg batching (64 datagrams per syscall)
- Zero allocation in the datapath, enforced by a counting allocator
- Preallocated per-core connection tables with hot and cold cache lines
- Checksums in AVX2 with a scalar fallback
- io_uring reactor (feature `io-uring`), opt-in
- AF_XDP frame pipeline (feature `af-xdp`), device-gated
- Runtime configuration through `config.json` and `FDS_*` environment
  variables
- Deterministic fuzz harness for the parsers and checksums
- `mol-core`: atoms, molecules, lock-free rings, buffers, memory
  layout, SIMD checksums, authoring templates

## Configuration

`config.json` is the sole runtime configuration source; every section
has a default. Environment overrides use `FDS_<SECTION>_<KEY>`, for
example `FDS_REACTOR_BUSY_POLL=0`, `FDS_CORE_THREADS=4`,
`FDS_REACTOR_STRATEGY=io-uring`, `FDS_ENGINE_UDP_BIND=0.0.0.0:7777`.

Key knobs:

- `core.threads`: worker count; 0 = one per logical CPU (default).
- `core.pin_cores`: pin worker `i` to logical CPU `i` (default on).
- `reactor.strategy`: `epoll-busy-poll` (default) or `io-uring`.
- `reactor.io_uring_entries`: ring size.
- `reactor.io_uring_sq_thread`: SQPOLL CPU; 0 = off.
- `af_xdp.device` / `af_xdp.queue`: XDP device queue for the frame
  pipeline.
- `udp.incoming_cpu`: default off. On loopback it pins all traffic to
  one worker; enable only with NIC RSS/IRQ affinity.

The schema is in `config/config.schema.json`, generated by `fds-detect`.

## Operations tuning

### NIC

- Disable interrupt coalescing on the datapath queue:
  `ethtool -C eth0 rx-usecs 0 tx-usecs 0 rx-frames 0 tx-frames 0`, and
  `adaptive-rx off adaptive-tx off`. The engine busy-polls; interrupts
  add latency.
- Jumbo frames: `ip link set dev eth0 mtu 9000`. The loopback device
  defaults to MTU 65536, so loopback benchmarks are unaffected.
- Ring sizes: `ethtool -G eth0 rx 4096 tx 4096` so bursts are not
  dropped while the engine is inside a batch.

### Kernel

- Socket buffer maxima: `net.core.rmem_max=16777216` and
  `net.core.wmem_max=16777216`. The engine requests 4 MiB per socket;
  without the caps, `setsockopt` clamps to about 212 KiB and UDP
  bursts drop.
- Receive backlog: `net.core.netdev_max_backlog=65536`.
- TCP fast open: `net.ipv4.tcp_fastopen=3` plus `TcpConfig::fastopen`.
  TFO lets a SYN carry data; do not enable for services that trust the
  source address before the handshake completes.
- TIME_WAIT reuse: `net.ipv4.tcp_tw_reuse=1` applies to the client
  side only. For a server, prefer SO_REUSEADDR and SO_REUSEPORT, which
  the engine sets.
- RPS and XPS: with a single-queue NIC, steer traffic across the
  engine's cores with `rps_cpus` and `xps_cpus` masks. Match the mask
  to the pinned cores.

### Application

- SO_REUSEPORT: one socket per worker on the same port; the kernel
  load-balances by 4-tuple hash. This is the per-core distribution
  mechanism.
- SO_INCOMING_CPU (`udp.incoming_cpu`, default off): with NIC RSS and
  IRQ affinity, pins a flow to the socket on the IRQ core. On loopback
  it pins all traffic to one worker.
- Hugepages: back the mmap'd buffer pools with 2 MiB pages
  (`transparent_hugepage/enabled=always`, defrag `madvise`) to remove
  TLB misses.
- CPU isolation: reserve cores for the engine with
  `isolcpus=2-7 nohz_full=2-7 rcu_nocbs=2-7` on the kernel command
  line, and set `core.threads` to the number of reserved cores.

### Offloads

- Checksum offload: keep the NIC checksums on for bulk traffic. The
  engine always computes its own checksums; the NIC setting does not
  affect the security path.
- TSO/GSO: on for TCP by default. For UDP, the engine drives
  segmentation with `UDP_SEGMENT` (`udp.gso_segment_size`).
- GRO: enable `udp.gro` together with the kernel `UDP_GRO` socket
  option on NIC-heavy workloads. LRO is a single-flow merge; keep it
  off.
- MSG_ZEROCOPY: enable only for large datagrams. The send buffer is
  borrowed until the NIC completes; the engine's batch ring handles
  this. On kernels where the copy path is silent, the engine
  self-disables after a 5 ms grace.

### SCTP

The kernel module and libsctp must be present:

```sh
modprobe sctp
```

The engine-side keys are `SctpConfig` (`init_max_streams`, `max_burst`,
`partial_delivery_point`, `nodelay`). If `socket(AF_SCTP, ...)` fails
at runtime, the transport skips with a log line; check the module.

### Verification

```sh
ethtool -c eth0        # coalescing: rx-usecs/tx-usecs = 0
ethtool -g eth0        # ring sizes
ethtool -k eth0        # offloads
sysctl net.core.rmem_max net.core.wmem_max net.core.netdev_max_backlog
cat /proc/net/softnet_stat    # drops column grows => backlog too small
cat /proc/interrupts          # per-CPU IRQ counts
```

### Quick reference

| Tuning | Setting | FDS config key |
| --- | --- | --- |
| IRQ coalescing off | `ethtool -C rx-usecs 0` | none (host-level) |
| Jumbo frames | `ip link set mtu 9000` | none (host-level) |
| Ring sizes | `ethtool -G rx/tx 4096` | `reactor.max_events` |
| Socket buffer caps | `net.core.rmem_max/wmem_max` | `udp.rcvbuf`/`sndbuf`, `tcp.rcvbuf`/`sndbuf` |
| Backlog | `net.core.netdev_max_backlog` | none (host-level) |
| TCP fast open | `net.ipv4.tcp_fastopen` | `tcp.fastopen` |
| TIME_WAIT reuse | `net.ipv4.tcp_tw_reuse` | none (use `tcp.reuseport`) |
| RPS/XPS steering | `rps_cpus`/`xps_cpus` | `core.threads`, `core.pin_cores` |
| SO_REUSEPORT | per-socket option | `udp.reuseport`, `tcp.reuseport` |
| SO_INCOMING_CPU | per-socket option | `udp.incoming_cpu` |
| Hugepages | THP `always` | none (engine maps via mmap) |
| CPU pinning | `sched_setaffinity` | `core.pin_cores` |
| CPU isolation | `isolcpus`, `nohz_full`, `rcu_nocbs` | `core.threads` |
| Checksum offload | `ethtool -K tx on rx on` | none (engine always computes) |
| TSO/GSO | `ethtool -K tso on gso on` | `udp.gso_segment_size` |
| LRO/GRO | `ethtool -K gro on lro off` | `udp.gro` |
| MSG_ZEROCOPY | `ethtool -K tx-udp-segmentation on` | `udp.zerocopy` |
| SCTP module | `modprobe sctp` | `sctp.*` |

### Kernel build notes

A kernel built with `CONFIG_INIT_ON_ALLOC_DEFAULT_ON=y` zeroes every
page allocated for an skb. That memset is datapath cost; measured at
about 18 percent of context switches under TCP load. Check with
`cat /sys/module/kernel/parameters/init_on_alloc`. Disable with
`init_on_alloc=0` on the kernel command line on a single-tenant host.

A stripped kernel limits measurement: kprobes may be absent, and UDP
MSG_ZEROCOPY may copy silently. A stock mainline kernel restores the
full measurement surface.

## Implementation examples

### 1. Run the engine

```sh
cargo run --release -p fds-core
```

The engine starts one worker per logical CPU. UDP echo is on
127.0.0.1:7777; TCP echo is on 127.0.0.1:7778.

### 2. Write a custom UDP handler

Use the engine as the reference loop and replace the echo handler with
your protocol:

1. Bind the socket in the worker, register it with
   `reactor.register(fd, token, Interest::Readable)`.
2. On a readable event, call `recv_batch` (recvmmsg, preallocated
   array) and process each datagram.
3. Respond with `send_batch` (sendmmsg).
4. Drain until EAGAIN, then return to the poll loop.

The zero-allocation rule: allocate the batch arrays once at startup.

### 3. Write a custom TCP protocol server

The example `examples/full_duplex_channels.rs` is a complete custom
server on the primitives: a reactor loop over a listener and channels,
with the edge-triggered discipline (drain to EAGAIN) and write
backpressure (when the echo write blocks, stop reading until the socket
is writable). Run it:

```sh
cargo run --release -p fds-core --example full_duplex_channels
```

It opens eight parallel TCP connections and streams in both directions
on each. The measured aggregate echo rate is about 1.1 GiB/s on the
reference machine.

### 4. Build a molecule with mol-core

`mol-core` provides the framework: atoms (pure and effectful
transformations), molecules (stateful transformations), lock-free
rings, fixed-capacity buffers, and SIMD checksums. Templates are in
`templates/`.

```rust
use mol::molecule::Molecule;
// A pure atom: adds a constant.
// Compose with `then` (sequential) and `par` (parallel).
let pipeline = add(5).then(double);   // (x + 5) * 2
```

See the crate documentation (`cargo doc -p mol-core`) and the templates
for the authoring pattern.

### 5. Add a checksum atom

The checksum atoms live in `fds-core::checksum` (private) and
`mol-core::simd` (public). `sum_u16` sums big-endian 16-bit words with
AVX2; `checksum_finalize` folds to the one's-complement result. A new
checksum follows the pattern: vectorized loop over array slices, scalar
remainder, bounds by construction.

### 6. Configure the engine

Write `config.json` at the repo root, or override with environment
variables. Every key is optional.

```json
{
  "core": { "threads": 4, "pin_cores": true },
  "reactor": { "strategy": "epoll-busy-poll", "busy_poll": true },
  "engine": { "udp_bind": "0.0.0.0:7777", "tcp_bind": "0.0.0.0:7778" }
}
```

`FDS_CORE_THREADS=4 FDS_REACTOR_STRATEGY=io-uring cargo run --release -p fds-core`

### 7. Bench a custom handler

The benchmark clients measure any echo-capable server:

```sh
cargo run --release -p fds-core -- --bench-udp-against 127.0.0.1:PORT 5
cargo run --release -p fds-core -- --bench-tcp-against 127.0.0.1:PORT 5
```

### 8. Use the io_uring strategy

```sh
FDS_REACTOR_STRATEGY=io-uring cargo run --release -p fds-core
```

The autotuner script compares strategies on the current machine and
kernel:

```sh
bash scripts/autotune.sh
```

### 9. AF_XDP on a supported NIC

With an XDP-capable NIC (ixgbe, i40e, ice, mlx5):

```json
{ "af_xdp": { "device": "eth0", "queue": 0 } }
```

The engine starts a forwarding thread that validates and echoes frames
through the AF_XDP rings.

### 10. Build tooling

```sh
bash build/build.sh --release        # adaptive release build
bash build/build.sh --summary        # detection facts
TARGET_CPU=x86-64-v3 bash build/build.sh --release   # portable build
cargo run -p fds-detect -- --emit-config             # write config.json
```

## Building applications

The library surface supports the application patterns that a network
server needs: custom UDP protocols, custom TCP protocols with full
duplex parallel channels, batched receive and send, connection tables,
metrics, and configuration. Atomos is the proof: an HTTP/1.1 server
with routing, caching, admission control, and operator control, built
on the FDS primitives.

The extension pattern is the same for every protocol: write the
handler, register the sockets with the reactor, drain to EAGAIN, keep
the hot path allocation-free, and preallocate at startup.
