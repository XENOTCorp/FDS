# FDS benchmarks

Measured comparisons of the FDS engine against existing network stacks.
All measurements in this document are apples-to-apples: the same
machine, the same load generator, the same payload, the same duration,
and the same loopback path for every row of a table. Raw measurements
are in `Docs/benchmarks/sota-2026-08-26/`.

## Method

- Machine: Intel Core i5-5200U (2 physical cores, 4 logical), kernel
  7.2.0_1 (Void Linux), loopback only. This is the full hardware
  statement. The loopback path is the domain of the comparison.
- The engine runs with its release defaults: epoll edge-triggered,
  event-driven, one worker per logical CPU, pinned, SO_REUSEPORT flow
  steering. The explicit busy-poll spin (`reactor.busy_poll=true`) is
  measured separately where stated. It is not the default because on a
  shared machine the spin starves co-tenants (see the latency note
  below).
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
- Every row below was measured on a quiet machine (no concurrent
  build, test, or benchmark process). Run-to-run spread on this
  two-core box is about 10-15% for throughput and about 10% for
  latency percentiles. Where a value was measured twice, both are
  shown.

## UDP echo throughput

Windowed echo: four client sockets, 64 in-flight 60 KiB datagrams,
5 s. The same client measures every server.

| Server | Sent | Echoed | Completion | Rank | % vs FDS |
| --- | --- | --- | --- | --- | --- |
| libuv | 19.10 Gbps | 19.10 Gbps | 100.00% | 1 | +14.3% |
| tokio (multi-thread) | 16.99 Gbps | 16.99 Gbps | 100.00% | 2 | +1.7% |
| FDS engine (epoll, event-driven) | 16.71 Gbps | 16.69 Gbps | 99.90% | 3 | 0% (baseline) |

The FDS engine is the only row with a drop path: the engine counts a
drop when the echo write would block and discards that burst. The
tokio and libuv servers buffer the echo in memory, which keeps the
completion at 100% at the cost of unbounded memory under a write flood.
Relative to FDS: tokio +1.7%, libuv +14.3%.

## TCP echo throughput

Two tests. The write flood measures how much a server can absorb into
its receive path. The lockstep test measures a full send-and-echo round
trip per frame, which is what a request-response protocol does.

Write flood, four connections, 60 KiB writes, 5 s:

| Server | Client to server | Notes | Rank | % vs FDS |
| --- | --- | --- | --- | --- |
| FDS engine (epoll, event-driven) | 36.68 Gbps | drops echoes on write-block; bounded memory | 1 | 0% (baseline) |
| libuv | 27.29 Gbps | buffers echoes in memory | 2 | -25.6% |
| tokio | stalled | flow control halts both sides; the bench client never reads | n/a | n/a |

Lockstep echo, four connections, 60 KiB frames, 5 s:

| Server | Aggregate echo rate | Rank | % vs FDS |
| --- | --- | --- | --- |
| FDS engine (epoll, event-driven) | 2494 MiB/s (2444, 2494 in two runs) | 1 | 0% (baseline) |
| tokio | 2226 MiB/s (2198, 2226 in two runs) | 2 | -10.7% |
| libuv | 1983 MiB/s (1962, 1983 in two runs) | 3 | -20.5% |

On the lockstep path tokio trails FDS by 10.7% and libuv by 20.5%.

## UDP latency percentiles

Single in-flight 32 B datagram, measured from a second process
(`--latency-against`), 3 s.

| Server | p50 | p90 | p99 | p999 | mean | Rank (p50) | % vs FDS (p50) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| tokio | 14.2 | 19.9 | 31.2 | 77.4 | 15.4 | 1 | -1.4% |
| FDS engine (epoll, event-driven) | 14.4 | 17.7 | 28.1 | 79.3 | 15.7 | 2 | 0% (baseline) |

All values are microseconds. Ranked by p50 (lower is better).
Relative to FDS: tokio is 1.4% faster at p50, 2.4% faster at p999,
and 1.9% faster on the mean. FDS is 12.4% faster at p90 and 11.0%
faster at p99. The engine's in-process loopback latency,
measured without a competing client process, is p50 11.5 µs, p90
17.5 µs, p99 28.2 µs, p999 71.3 µs (`--latency 5`).

The previous release default was an explicit busy-poll spin
(`reactor.busy_poll=true`): every worker polled with a zero timeout,
burning about 3.4 cores while idle and starving the latency probe on
this two-core box. Measured under that default, the same latency row
read p50 21.1 µs with a p99 of 1521 µs and a p999 of 3806 µs, and UDP
echo measured 11.84 Gbps. The event-driven default is now the release
behavior. The spin remains available through configuration for
dedicated-core deployments.

## SCTP throughput

One-way SCTP over loopback, 32 KiB messages, 3 s. The FDS row is
measured in-process: both endpoints live inside the engine process and
traverse the kernel SCTP stack over a loopback socket pair. The lksctp
row uses a separate server process and client process over the same
kernel stack. The message pattern, payload, and duration are identical.

| Stack | Throughput | Messages per second | Rank | % vs FDS |
| --- | --- | --- | --- | --- |
| FDS engine | 13.8 Gbps | 52,651 | 1 | 0% (baseline) |
| lksctp-tools (kernel SCTP, C harness) | 12.24 Gbps | 46,692 | 2 | -11.3% |

SCTP echo RTT through the kernel stack (lksctp harness, 32 B messages):
p50 22.2 µs, p90 27.2 µs, p99 48.1 µs, mean 24.3 µs.

## Reactor strategies (measured on this kernel)

The same engine, three polling strategies, UDP echo 4 x 60 KiB, with
the event-driven default:

| Strategy | UDP echo | TCP flood | Rank (UDP) | % vs epoll (default) |
| --- | --- | --- | --- | --- |
| io_uring (event-driven) | 17.70 Gbps | stalled (accept/echo path) | 1 | +4.3% |
| epoll (event-driven, default) | 16.97 Gbps | 35.24 Gbps | 2 | 0% (baseline) |
| io_uring SQPOLL | 16.68 Gbps | ~0 (stalls) | 3 | -1.7% |

Ranked by UDP echo. On TCP only the epoll row is sound: io_uring
ranks first on UDP but is not admissible because its TCP datapath
stalls (the soundness floor below).

The io_uring reactor matches epoll on UDP and stalls on TCP on this
kernel. SQPOLL's kernel thread contends with the four workers on two
physical cores. The startup autotuner (`Code/scripts/autotune.sh`)
selects the strategy per machine and kernel: the lattice minimum on
this machine is epoll event-driven (15.43 Gbps UDP echo in the autotune
run), with io_uring disqualified by the TCP soundness floor.

## Verified observations (measured)

- MSG_ZEROCOPY on UDP: the feature self-disables after a 5 ms grace and
  falls back to the copy path. This kernel copies loopback datagrams at
  send time, sends no completion notifications, and never references
  the user pages. The disable fires in every ZC-enabled run. With the
  feature enabled the UDP echo row measures in the same band as the
  baseline: 15.1-15.5 Gbps steady-state against a 13.1-17.0 Gbps
  baseline across the same sessions, with cold-start dips in both
  modes. The two modes are not ranked: their bands overlap.
- io_uring TCP: the accept/echo path stalls under the write flood.
- io_uring SQPOLL: the kernel polling thread loses on 2C/4T.

## Stacks not run on this platform

The following stacks require hardware or build tooling that this
machine does not provide. Their published results are not reproduced
here. A measurement on this machine would not be apples-to-apples.

- DPDK and F-Stack: require a NIC with DPDK-capable drivers. The only
  live link here is Wi-Fi. The wired port has no carrier.
- msquic: requires a QUIC build environment and a certificate store.
- Netty: the netty-all artifact is not retrievable through this
  network. The JDK is present.
- quiche: requires BoringSSL, which needs NASM. NASM is not installed.

## HTTP files in the snapshot

FDS is a transport engine, not an HTTP server. The snapshot directory
also holds HTTP/1.1, HTTP/2, and HTTP/3 comparison files measured on
the same machine (nginx, h2o, Caddy, Seastar, axum, Hyper, actix-web,
and related rows). Those files are raw tool output. They are not
ranked in the tables above.
