# Datapaths

FDS has three datapaths. The kernel socket path is the default. io_uring
is the completion-driven socket path. AF_XDP is the kernel-bypass frame
path. DPDK is not in this tree.

## Kernel datapath (default)

The engine runs on the kernel socket path: epoll readiness, recvmmsg and
sendmmsg with a D-1/D-4 batch (default 4 datagrams on the reference CPU
so 4 × 60 KiB stays in L2; override `FDS_UDP_RX_SLOTS`). TCP uses a
reusable 64 KiB buffer per connection and retains short-write tails,
pausing reads on backpressure. Historical rankings refer to older engine
semantics; remeasure the current code. See [benchmarks](../benchmarks.md).

## io_uring

The `io-uring` reactor (feature `io-uring`) runs UDP and TCP echo through
the ring. On a kernel 6.0 or later that accepts registered buffers, the
reactor uses the modern path:

- `IORING_OP_ACCEPT_MULTI` for the listener (one submission, many
  accepts)
- `IORING_OP_RECVMSG_MULTI` into a provided-buffer group (one
  submission, many receives)
- ordinary `IORING_OP_SEND` against the provided/registered buffer pool;
  the current path does not submit SEND_ZC
- one send in flight per stream, so asynchronous execution and short-write
  retries cannot reorder TCP bytes; submission batching spans connections
- high/low watermarks cancel and re-arm receive so a write flood cannot
  grow an unbounded send queue
- submission batching: `io_uring_enter` runs when the submission queue
  reaches the flush threshold, not on every opcode

On an older kernel the reactor falls back to single-shot Accept, Read,
and Write. `FDS_IOU_LEGACY=1` explicitly selects this path without a
registered pool. Runtime multishot rejection fails closed: switching modes
while modern requests still reference buffers is unsafe. Each datapath
instance may run only once; construct a new instance to restart it.

SQPOLL (`reactor.io_uring_sq_thread`) starts a kernel submission thread.
On a two-core machine that thread can starve the workers. Leave SQPOLL
off unless the extra core is free.

Config keys: `FDS_REACTOR_STRATEGY=io-uring`,
`FDS_REACTOR_IO_URING_ENTRIES`, and `FDS_REACTOR_IO_URING_SQ_THREAD`.

The TCP accept-echo path is covered by
`datapath_tcp_write_flood_echoes` in `fds::io_uring_reactor`.

## AF_XDP

The `af-xdp` path is a first-class worker datapath. When
`af_xdp.device` is set, each worker binds one queue of that device and
runs the zero-copy frame loop instead of the kernel socket path.

The socket binds with `XDP_ZEROCOPY` so the NIC DMA accesses UMEM directly. If
the driver rejects zero-copy, the socket falls back to `XDP_COPY`.
`XDP_USE_NEED_WAKEUP` is always set. `kick` wakes the kernel only when
the fill or TX ring sets `XDP_RING_NEED_WAKEUP`.

Receive checks a frame out of the umem. The handler processes the frame
in place. Echo transmits the same umem slot. Drop returns the slot to
the fill ring. Completions recycle transmitted slots. Socket identities
and checkout generations reject stale, duplicate, and cross-socket frame
handles before exposing mutable memory.

Multiqueue: `af_xdp.queues` lists the queue ids. Worker `i` binds
`queues[i % len]`. Each worker has its own umem and rings.

NUMA: when `af_xdp.numa` is true, the worker reads its node after pin
and binds the umem with `mbind`. The data plane stays on that node.
There is no cross-socket bounce for frame bytes.

An attached XDP program must steer frames into a pinned XSKMAP
(`af_xdp.xskmap`) or the socket binds and receives nothing.

```
NIC queue  ->  XDP program  ->  XSKMAP  ->  AF_XDP RX ring
                                              |
                                              v
                                        umem frame (in place)
                                              |
                                    echo: TX ring (same slot)
                                    drop: fill ring
                                              |
                                              v
                                        completion ring -> fill ring
```

Set the device in `config.json`:

```json
{
  "af_xdp": {
    "device": "eth0",
    "queues": [0, 1],
    "zero_copy": true,
    "numa": true,
    "xskmap": "/sys/fs/bpf/xskmap"
  }
}
```

Bring up a local veth pair:

```sh
bash scripts/veth-af-xdp.sh
```

Transmit on a physical NIC needs a driver with an XDP queue (ixgbe,
i40e, ice, mlx5).

## IPv6 and dual-stack

UDP and TCP sockets bind IPv4 or IPv6. An IPv6 bind with `udp.ipv6_only`
or `tcp.ipv6_only` set to false (the default) is dual-stack: `[::]:port`
accepts IPv4-mapped clients. IPv4-mapped peers are presented as IPv4
addresses so echo replies use the same family the client used.

AF_XDP `process_frame` echoes IPv4 and IPv6 UDP. Userspace TCP
(`fds::ustack`) speaks both families.

## Userspace TCP

`fds::ustack::TcpStack` is a packet-in / packet-out TCP. It does the
three-way handshake, software TSO (MSS chop), RACK loss detection
(RFC 8985) and RTO retransmission. Set `engine.userspace_tcp` with an
AF_XDP device to run it on the XDP worker. Tests drive two stacks over
a simulated wire, including a loss case.

```sh
./target/release/fds --bench-ustack 1
```

## DPDK

DPDK is not in this tree. AF_XDP native zero-copy is the in-tree
kernel-bypass path. Use DPDK when the target NIC has no XDP queue. DPDK
needs hugepages, UIO or VFIO, and `dpdk-devbind.py`.
