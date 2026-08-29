# Configuration

`config.json` in `Code/` is the runtime configuration file. Every section has a default. Environment overrides use `FDS_<SECTION>_<KEY>`. Examples: `FDS_REACTOR_BUSY_POLL=0`, `FDS_CORE_THREADS=4`, `FDS_REACTOR_STRATEGY=io-uring`, `FDS_ENGINE_UDP_BIND=0.0.0.0:7777`.

The schema is `Code/config/config.schema.json`. `fds-detect` generates it.

## Keys

- `core.threads`: worker count. 0 means one per logical CPU (default).
- `core.pin_cores`: pin workers; unique physical cores when the count fits, else logical CPU `i` (default on).
- `reactor.strategy`: `epoll-busy-poll` (default token; busy poll is a separate flag) or `io-uring`.
- `reactor.busy_poll`: explicit spin for dedicated cores (default off).
- `reactor.io_uring_entries`: ring size.
- `reactor.io_uring_sq_thread`: SQPOLL CPU. 0 means off.
- `af_xdp.device`: NIC name. Empty means the kernel socket path (default).
- `af_xdp.queue`: queue id when `queues` is empty.
- `af_xdp.queues`: per-worker queue ids. Workers take queues round-robin.
- `af_xdp.zero_copy`: bind with `XDP_ZEROCOPY`. Falls back to `XDP_COPY` when the driver rejects it (default on).
- `af_xdp.ring_size`: per-ring entry count, power of two (default 256).
- `af_xdp.num_frames`: umem frame count (default 4096).
- `af_xdp.numa`: bind each worker's umem to its NUMA node with `mbind` (default off).
- `af_xdp.xskmap`: pinned XSKMAP path. Empty means do not register.
- `udp.ipv6_only` / `tcp.ipv6_only`: `IPV6_V6ONLY`. Default false is dual-stack.
- `engine.userspace_tcp`: run userspace TCP (RACK, TSO) on the AF_XDP worker.
- `udp.incoming_cpu`: default off. On loopback it pins all traffic to one worker. Enable only with NIC RSS and IRQ affinity.

Example:

```json
{
  "core": { "threads": 4, "pin_cores": true },
  "reactor": { "strategy": "epoll-busy-poll", "busy_poll": false },
  "engine": { "udp_bind": "0.0.0.0:7777", "tcp_bind": "0.0.0.0:7778" }
}
```

```sh
cd Code
FDS_CORE_THREADS=4 FDS_REACTOR_STRATEGY=io-uring cargo run --release -p fds-engine
```
