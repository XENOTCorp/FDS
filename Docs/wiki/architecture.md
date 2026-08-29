# Architecture

The engine runs one worker thread per logical CPU. Each worker owns its poller, its sockets, its connection table, and its counters. Nothing is shared except a metrics bundle. The hot path performs no allocation and no per-packet syscall beyond the batched transport.

## One worker per logical CPU

Each worker owns:

- its datapath: epoll edge-triggered and event-driven with the syscall transports by default (a busy-poll spin is available for dedicated cores), the io_uring completion-driven datapath, or the AF_XDP zero-copy frame loop when `af_xdp.device` is set;
- its own SO_REUSEPORT UDP socket and TCP listener on the shared bind addresses. The kernel steers flows across workers by 4-tuple hash;
- its own connection table, active-stream slot array, 64 KiB datagram receive batch, and per-core counters.

The worker pins itself with `sched_setaffinity` when `core.pin_cores` is set. If the worker count fits on the physical cores, worker `i` pins to the first SMT sibling of physical core `i` (on a typical SMT CPU, logical 0 and 1 are the same core). Otherwise worker `i` pins to logical CPU `i`.

## Data flow (one worker)

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

The metrics listener is registered on worker 0 only. Metrics are pulled over a Unix socket.

## AF_XDP worker

When `af_xdp.device` is set, the worker does not bind UDP or TCP sockets.
It binds one AF_XDP queue (round-robin over `af_xdp.queues`). The umem
lives on the worker's NUMA node when `af_xdp.numa` is true.

```
NIC queue -> XDP program -> XSKMAP -> AF_XDP RX ring
                                        |
                                        v
                                  umem frame (in place)
                                        |
                              echo: TX ring (same slot)
                              drop: fill ring
```

Each worker owns its socket, umem, and rings. Frame bytes do not cross
a NUMA socket.

## Loop invariants

- Edge-triggered with drain to EAGAIN. After an event fires, the handler must drain the fd until the syscall returns EAGAIN, or no further edge is generated and events are lost. The loop processes each poll batch fully before polling again.
- Event-driven: `wait` blocks in the kernel until an event is ready, waking at most every 100 ms to observe the stop flag. Idle CPU is zero.
- Busy-poll (opt-in, `reactor.busy_poll=true`): `wait(timeout 0)` repeats until the ready list is empty. The engine burns CPU when idle. That is the latency contract for a dedicated core.
- No allocation in the hot path. Receive buffers, event arrays, echo batches, and the connection table are preallocated at startup. The only per-connection allocation is the connection-map entry created at accept and removed at close.

## Cache layout

- Ring `head` and `tail` each occupy a 64-byte line. Producer and consumer do not bounce one line.
- Connection hot state (seq, activity, in-flight, fd) is one line. Cold state (peer, flags) is another line.
- Per-worker metrics (packets, bytes, drops) share one line. Adjacent workers do not share a line.
- Receive buffers are 64-byte aligned. The UDP slab is `udp_rx_slots` × 64 KiB (default 4 × 64 KiB = 256 KiB, L2-resident on the reference CPU) and is advised with `MADV_HUGEPAGE`.
- TCP lookup is a slot index from the epoll token. There is no hash map on the hot path.

## Crate map

| Crate | Role |
| --- | --- |
| `mol` | Atoms, molecules, FIFO rings, LIFO stacks, delayed feedback, buffers, SIMD checksums, layout |
| `fds` | Public API, reactor, TCP, UDP, SCTP, conn, config, metrics, parse |
| `fds-engine` | Binary `fds`: echo loop, CLI, benches |
| `fds-detect` | Hardware detect, emit and validate `config.json` |

### `fds` modules

| Module | Responsibility |
| --- | --- |
| `api` | Driver/callback and AsyncRead/AsyncWrite surface for other programs |
| `reactor` | rustix epoll, edge-triggered, preallocated event array |
| `tcp` | nonblocking listener and stream (accept4, FASTOPEN, NODELAY, writev, splice) |
| `udp` | recvmmsg/sendmmsg, GSO, GRO, MSG_ZEROCOPY |
| `sctp` | libsctp FFI |
| `conn` | hot/cold state, preallocated `ConnTable` |
| `checksum` | crate-private IP/TCP/UDP/SCTP checksums |
| `parse` | bounds-safe IPv4/UDP/TCP parsers |
| `metrics` | per-core line-packed counters, Unix-socket pull |
| `config` | `config.json` plus `FDS_*` |
| `util` | pinning, coarse monotonic ticks |
| `io_uring_reactor` | completion datapath, feature `io-uring`: multishot recv/accept, registered buffers, SEND_ZC |
| `af_xdp` | zero-copy frame datapath, feature `af-xdp`: `XDP_ZEROCOPY`, NUMA-local umem, multiqueue |
