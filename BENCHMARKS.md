# FDS benchmarks

Measured comparisons of the FDS engine against existing network stacks.
All measurements in this document are apples-to-apples: the same
machine, the same load generator, the same payload, the same duration,
and the same loopback path for every row of a table. Raw measurements
are in `bench-results/`.

## Method

- Machine: Intel Core i5-5200U (2 physical cores, 4 logical), kernel
  7.2.0_1 (Void Linux), loopback only. This is the full hardware
  statement; the loopback path is the domain of the comparison.
- Load generators: the FDS benchmark client (windowed UDP echo, TCP
  write flood), a lockstep TCP echo client, `wrk` for HTTP/1.1,
  `h2load` for HTTP/2, and `curl` for latency percentiles.
- Payloads: 32 B (latency), 60 KiB (echo throughput), 64 KiB (HTTP
  file), 32 KiB SCTP messages.
- Durations: 5 s for throughput, 3 s for latency sampling.
- Percentiles are measured over the full sample set and reported as
  p10, p50, p90, p95, p99, p999.
- Each server runs out of the box with its default settings, except
  where a row states otherwise. nginx uses `open_file_cache` (its tuned
  static path), h2o uses `num-threads: 4`, and Seastar uses
  `--smp 4` with `connection_distribution`.

## UDP echo throughput

Windowed echo: four client sockets, 64 in-flight 60 KiB datagrams,
5 s. The same client measures every server.

| Server | Sent | Echoed | Completion |
| --- | --- | --- | --- |
| FDS engine (epoll) | 11.17 Gbps | 11.16 Gbps | 99.91% |
| tokio (multi-thread) | 15.82 Gbps | 15.82 Gbps | 100.00% |
| libuv | 18.83 Gbps | 18.83 Gbps | 100.00% |

The FDS engine is the only row with a drop path: the engine counts a
drop when the echo write would block and discards that burst. The
tokio and libuv servers buffer the echo in memory, which keeps the
completion at 100% at the cost of unbounded memory under a write flood.

## TCP echo throughput

Two tests. The write flood measures how much a server can absorb into
its receive path. The lockstep test measures a full send-and-echo round
trip per frame, which is what a request-response protocol does.

Write flood, four connections, 60 KiB writes, 5 s:

| Server | Client to server | Notes |
| --- | --- | --- |
| FDS engine (epoll) | 11.85 Gbps | drops echoes on write-block; bounded memory |
| libuv | 26.77 Gbps | buffers echoes in memory |
| tokio | stalled | flow control halts both sides; the bench client never reads |

Lockstep echo, four connections, 60 KiB frames, 5 s:

| Server | Aggregate echo rate |
| --- | --- |
| tokio | 724.7 MiB/s |
| libuv | 568.3 MiB/s |
| FDS engine (epoll) | 544.2 MiB/s |

The FDS engine's drop-on-write-block discipline favors the write flood
over the lockstep pattern. The example server in
`examples/full_duplex_channels.rs` applies write backpressure instead
of dropping and sustains 1112 MiB/s over eight full-duplex channels
with 8 KiB frames.

## UDP latency percentiles

Single in-flight 32 B datagram, measured from a second process
(`--latency-against`), 3 s.

| Server | p10 | p50 | p90 | p99 | p999 | mean | stdev |
| --- | --- | --- | --- | --- | --- | --- | --- |
| tokio | 13.4 | 13.6 | 17.5 | 29.6 | 94.3 | 15.0 | 8.4 |
| FDS engine (epoll) | 19.4 | 21.0 | 25.7 | 2228.0 | 5978.3 | 73.2 | 441.0 |

The FDS engine's tail comes from the drop path under load: the engine
drops the echo when the write would block, and the client re-sends.
The engine's in-process loopback latency, measured without a competing
client process, is p50 11.8 µs, p90 17.8 µs, p99 28.3 µs, p999 53.9 µs
(`--latency 5`).

## SCTP throughput

One-way SCTP over loopback, 32 KiB messages, 3 s.

| Stack | Throughput | Messages per second |
| --- | --- | --- |
| FDS engine | 14.0 Gbps | 53,243 |
| lksctp-tools (kernel SCTP, C harness) | 12.51 Gbps | 47,720 |

SCTP echo RTT through the kernel stack (lksctp harness, 32 B messages):
p50 22.5 µs, p90 27.3 µs, p99 50.7 µs, mean 24.7 µs.

## Reactor strategies (measured on this kernel)

The same engine, three polling strategies, UDP echo 4 x 60 KiB:

| Strategy | UDP echo | TCP echo |
| --- | --- | --- |
| epoll busy-poll | 10.31 Gbps | 20.22 Gbps |
| io_uring | 10.14 Gbps | stalled (accept/echo path) |
| io_uring SQPOLL | 5.65 Gbps | 0.21 Gbps |

The io_uring reactor matches epoll on UDP and stalls on TCP on this
kernel. SQPOLL's kernel thread contends with the four workers on two
physical cores. The startup autotuner (`scripts/autotune.sh`) selects
the strategy per machine and kernel: the lattice minimum on this
machine is epoll busy-poll (UDP echo 13.82 Gbps in a quiet run).

## Verified regressions (measured)

Three optimizations measured slower than the baseline on this kernel.
Each row states the mechanism.

- MSG_ZEROCOPY on UDP: 8.18 Gbps sent / 7.86 Gbps echoed, against
  10.38 / 10.37 baseline. The kernel copies the datagram at send time
  and never references the user pages; the feature self-disables after
  a 5 ms grace and falls back to the copy path.
- io_uring TCP: the accept/echo path stalls under the write flood.
- io_uring SQPOLL: the kernel polling thread loses on 2C/4T.

## Stacks not run on this platform

The following stacks require hardware or build tooling that this
machine does not provide. Their published results are not reproduced
here; a measurement on this machine would not be apples-to-apples.

- DPDK and F-Stack: require a NIC with DPDK-capable drivers. The only
  live link here is Wi-Fi; the wired port has no carrier.
- msquic: requires a QUIC build environment and a certificate store.
- Netty: the netty-all artifact is not retrievable through this
  network; the JDK is present.
- quiche: requires BoringSSL, which needs NASM; NASM is not installed.

## HTTP

HTTP/1.1, HTTP/2, and HTTP/3 measurements are in the Atomos repository:
[BENCHMARKS.md](../Atomos/BENCHMARKS.md). The Atomos H1 engine is the
FDS epoll transport; its comparisons against nginx, h2o, Caddy,
Seastar, axum, Hyper, and nghttpd are measured with the same method as
above.
