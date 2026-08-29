# FDS benchmarks

Measured comparisons of the FDS engine against existing network stacks.
Raw files: `Docs/benchmarks/2026-08-28/` (stack ranking) and
`Docs/benchmarks/2026-08-29/` (datapath comparison). Transport
only: TCP, UDP, SCTP. No HTTP. No Atomos.

The FDS / libuv / tokio ranking (UDP echo, UDP latency, TCP lockstep,
TCP write flood) is from one pass on 2026-08-28: 20 s idle first, one
server at a time, TCP flood last so a stalled flood cannot poison later
rows. SCTP, in-process latency, reactor strategies, and MSG_ZEROCOPY
were measured later the same day, one bench at a time, with idle
between each run.

## Method

- Machine: Intel Core i5-5200U (2 physical cores, 4 logical), kernel
  7.2.0_1 (Void Linux), loopback only. This is the full hardware
  statement. The loopback path is the domain of the comparison.
- The engine runs with its release defaults: epoll edge-triggered,
  event-driven, one worker per logical CPU, pinned, SO_REUSEPORT flow
  steering, UDP recvmmsg/sendmmsg batch of 4 (D-1/D-4: 4 × 60 KiB
  stays in this CPU's 256 KiB L2; a 64-slot batch misses the 3 MiB L3).
  Override with `FDS_UDP_RX_SLOTS`. The explicit busy-poll spin
  (`reactor.busy_poll=true`) is measured separately where stated. It is
  not the default because on a shared machine the spin starves
  co-tenants.
- Load generators: the FDS benchmark client (windowed UDP echo, TCP
  write flood, lockstep TCP echo, UDP latency, in-process latency,
  SCTP) and the lksctp C harness for kernel SCTP. The 2026-08-26
  snapshot also used `wrk` / `h2load` / `curl` for HTTP. Those HTTP
  files were not re-run.
- Payloads: 32 B (latency), 60 KiB (echo throughput), 32 KiB SCTP
  messages.
- Durations: 5 s for throughput, 3 s for latency sampling, 3 s for
  SCTP, 2 s for SCTP RTT.
- Percentiles are measured over the full sample set and reported as
  p10, p50, p90, p95, p99, p999.
- Each server runs out of the box with its default settings, except
  where a row states otherwise.
- Run-to-run spread on this two-core box is about 10–15% for
  throughput and about 10% for latency percentiles. Concatenating
  several floods on this CPU without idle between them drops later
  rows into the noise floor; those concatenated captures are not used
  in the tables. Where a value was measured twice, both are shown.

## UDP recvmmsg slot lattice (D-4)

Same client, 60 KiB, 5 s, 4 FDS workers. Slot count is how many
datagrams one `recvmmsg`/`sendmmsg` may take. On this CPU, 4 × 60 KiB
= 240 KiB, which fits the 256 KiB L2. Larger n copies a working set
that misses L3; syscall amortization is already saturated because the
copy dominates `t_s`.

| Slots | Sent | Echoed | Completion |
| --- | --- | --- | --- |
| 1 | 18.31 Gbps | 18.28 Gbps | 99.83% |
| 2 | 20.72 Gbps | 20.69 Gbps | 99.86% |
| 4 (default) | 21.14 Gbps | 21.12 Gbps | 99.89% |
| 8 | 19.84 Gbps | 19.82 Gbps | 99.87% |
| 16 | 17.93 Gbps | 17.91 Gbps | 99.88% |
| 32 | 17.58 Gbps | 17.55 Gbps | 99.80% |
| 64 (previous default) | 17.72 Gbps | 17.70 Gbps | 99.89% |

The engine default is 4. `FDS_UDP_RX_SLOTS` overrides. A later
three-run confirm of the default on the same binary measured sent
19.84 / 21.37 / 21.33 Gbps (`fds-udp-4slot.txt`).

## UDP echo throughput

Windowed echo: four client sockets, 64 in-flight 60 KiB datagrams,
5 s. The same client measures every server. Ranking files:
`fds-udp.txt`, `libuv-udp.txt`, `tokio-udp.txt`.

| Server | Sent | Echoed | Completion | Rank | % vs FDS |
| --- | --- | --- | --- | --- | --- |
| FDS engine (epoll, event-driven, 4-slot) | 24.03 Gbps | 24.00 Gbps | 99.87% | 1 | 0% (baseline) |
| libuv | 18.86 Gbps | 18.86 Gbps | 100.00% | 2 | −21.4% |
| tokio (multi-thread) | 16.84 Gbps | 16.84 Gbps | 100.00% | 3 | −29.8% |

The FDS engine is the only row with a drop path: the engine counts a
drop when the echo write would block and discards that burst. The
tokio and libuv servers buffer the echo in memory, which keeps the
completion at 100% at the cost of unbounded memory under a write flood.

The ranking FDS row (24.03 Gbps) sits above the lattice-day 4-slot
confirm (21.37 Gbps peak of three). That gap is inside the 10–15%
spread stated above. The ranking table uses the ranking-pass files,
not the confirm.

## TCP echo throughput

Two tests. The write flood measures how much a server can absorb into
its receive path. The lockstep test measures a full send-and-echo round
trip per frame, which is what a request-response protocol does.

Write flood, four connections, 60 KiB writes, 5 s. FDS from the
ranking pass (`fds-tcp.txt`). libuv from an isolated retry
(`libuv-tcp.txt`); the first ranking attempt reset the libuv process.

| Server | Client to server | Notes | Rank | % vs FDS |
| --- | --- | --- | --- | --- |
| FDS engine (epoll, event-driven) | 36.31 Gbps | drops echoes on write-block; bounded memory | 1 | 0% (baseline) |
| libuv | 23.43 Gbps | buffers echoes in memory | 2 | −35.5% |
| tokio | stalled | flow control halts both sides; the bench client never reads | n/a | n/a |

Lockstep echo, four connections, 60 KiB frames, 5 s. Ranked by the
higher of two runs:

| Server | Aggregate echo rate | Rank | % vs FDS |
| --- | --- | --- | --- |
| FDS engine (epoll, event-driven) | 2315.3 MiB/s (1972.9, 2315.3 in two runs) | 1 | 0% (baseline) |
| tokio | 2176.0 MiB/s (2176.0, 2031.9 in two runs) | 2 | −6.0% |
| libuv | 2035.7 MiB/s (2035.7, 2034.1 in two runs) | 3 | −12.1% |

## UDP latency percentiles

Single in-flight 32 B datagram, measured from a second process
(`--latency-against`), 3 s. Ranked by p50 (lower is better). All
values are microseconds.

| Server | p50 | p90 | p99 | p999 | mean | Rank (p50) | % vs FDS (p50) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| FDS engine (epoll, event-driven) | 13.8 | 17.6 | 24.3 | 49.7 | 14.7 | 1 | 0% (baseline) |
| tokio | 14.1 | 22.4 | 35.5 | 106.4 | 16.7 | 2 | +2.2% |
| libuv | 14.5 | 16.4 | 24.9 | 53.7 | 14.6 | 3 | +5.1% |

Relative to FDS at p50: tokio is 2.2% slower, libuv 5.1% slower. libuv
wins p90 (16.4 vs 17.6) and the mean (14.6 vs 14.7). FDS wins p99 and
p999 against both.

In-process loopback latency (`--latency 5`, no competing client
process), measured isolated: p50 17.4 µs, p90 24.2 µs, p99 92.7 µs,
p999 1084.6 µs (`fds-inproc-lat.txt`). That tail is worse than the
cross-process ranking row above; the in-process probe shares the same
process as the workers on this two-core box.

The previous release default was an explicit busy-poll spin
(`reactor.busy_poll=true`): every worker polled with a zero timeout,
burning about 3.4 cores while idle and starving the latency probe on
this two-core box. Measured under that default, the same latency row
read p50 21.1 µs with a p99 of 1521 µs and a p999 of 3806 µs, and UDP
echo measured 11.84 Gbps. The event-driven default is now the release
behavior. The spin remains available through configuration for
dedicated-core deployments.

## SCTP throughput

One-way SCTP over loopback, 32 KiB messages, 3 s. Each row was run
alone. The FDS row is in-process: both endpoints live inside the
engine process and traverse the kernel SCTP stack over a loopback
socket pair. The lksctp row uses a separate server process and client
process over the same kernel stack. The message pattern, payload, and
duration are identical.

| Stack | Throughput | Messages per second | Rank | % vs FDS |
| --- | --- | --- | --- | --- |
| FDS engine | 12.1 Gbps | 46,078 | 1 | 0% (baseline) |
| lksctp-tools (kernel SCTP, C harness) | 11.10 Gbps | 42,330 | 2 | −8.3% |

SCTP echo RTT through the kernel stack (lksctp harness, 32 B messages):
p50 23.9 µs, p90 32.2 µs, p99 69.9 µs, mean 26.9 µs.

## Reactor strategies (measured on this kernel)

The same engine, three polling strategies. Each strategy was a fresh
engine, UDP echo alone, then a later fresh engine for TCP flood
alone. UDP values are sent rate.

| Strategy | UDP echo | TCP flood | Rank (UDP) | % vs epoll (default) |
| --- | --- | --- | --- | --- |
| epoll (event-driven, default) | 19.94 Gbps (echoed 19.90, 99.83%) | 31.35 Gbps | 1 | 0% (baseline) |
| io_uring (event-driven) | 16.53 Gbps (echoed 16.51, 99.87%) | stalled | 2 | −17.1% |
| io_uring SQPOLL | 11.66 Gbps (echoed 11.64, 99.80%) | stalled | 3 | −41.5% |

On TCP only the epoll row is sound: io_uring and SQPOLL produced no
stdout before the 22 s capture ended (`io_uring-tcp.txt`,
`sqpoll-tcp.txt`). SQPOLL's kernel thread contends with the four
workers on two physical cores.

This reactor table is not the UDP ranking table. The ranking FDS UDP
row (24.03 Gbps) is the default epoll engine on the ranking pass.
The 19.94 Gbps epoll row here is a later isolated run of the same
binary.

The startup autotuner (`Code/scripts/autotune.sh`) selects the
strategy per machine and kernel. On this machine the lattice minimum
is epoll event-driven. The 2026-08-28 capture disqualified io_uring
on a write-only TCP flood that never drained the echo. That client
is no longer the TCP bench: see the 2026-08-29 table below.

## Datapath comparison (2026-08-29)

Same machine, kernel 7.2.0_1, loopback. Event-driven (`busy_poll=0`).
One worker per logical CPU. Fresh engine per row. Idle 2 s between
rows. Throughput 5 s. Latency and SCTP 3 s. TCP client writes and
reads (drain-echo) so the io_uring high watermark cannot stall.
Raw files: `Docs/benchmarks/2026-08-29/`. Runner:
`Code/scripts/bench-datapaths.sh`.

UDP echo and TCP drain-echo, isolated engines:

| Strategy | UDP sent / echoed | TCP client→server | % vs epoll (UDP) |
| --- | --- | --- | --- |
| epoll (event-driven) | 18.77 / 18.74 Gbps (99.84%) | 9.78 Gbps | 0% (baseline) |
| io_uring (event-driven) | 14.32 / 14.30 Gbps (99.87%) | 5.79 Gbps | −23.7% |

io_uring TCP completes. The 2026-08-28 “stalled” TCP row used a
write-only flood. Drain-echo is slower than that flood on epoll
(9.78 Gbps vs 31.35 Gbps) because the client also reads the echo.
The two TCP methods are not ranked against each other.

Cross-process UDP latency (`--latency-against`, 32 B, 3 s):

| Strategy | p50 | p90 | p99 | p999 | mean | notes |
| --- | --- | --- | --- | --- | --- | --- |
| epoll | 21.5 µs | 27.2 µs | 91.4 µs | 1707.5 µs | 27.8 µs | 107274 samples |
| io_uring | 84.8 µs | 113.3 µs | 889.1 µs | 2412.6 µs | 109.9 µs | 27265 samples; 13092 engine drops; last recv timed out |

Dual-stack (`[::]:7777` / `[::]:7778`, `ipv6_only=false`). Functional
check, not a ranking: IPv4 and IPv6 clients on one engine, UDP then
TCP, so later rows share that instance.

| Client | UDP sent / echoed | TCP client→server |
| --- | --- | --- |
| 127.0.0.1 | 14.07 / 14.03 Gbps (99.68%) | 6.82 Gbps |
| ::1 | 13.75 / 13.72 Gbps (99.79%) | 8.08 Gbps |

Userspace TCP (`--bench-ustack 5`): lossless TSO 2.00 Gbps on the
in-process wire. 10% data-frame loss recovered 8000/8000 bytes via
RACK/RTO.

In-process (no second process): UDP `--bench` 62.7 kpps / 83.7 MB/s;
`--bench-large` 60 KiB send 14.53 Gbps, recv 12.53 Gbps; `--latency`
p50 26.4 µs; `--latency-tcp` p50 33.8 µs; `--bench-sctp` 6.4 Gbps.

AF_XDP vs xdpsock (veth, user+net namespace): both sockets bound in
copy mode. RX 0 pps on both. veth delivers frames to an XSK only
when an XDP redirect program is attached. BPF attach needs
`CAP_BPF`. TX zero-copy pps needs a NIC with native XDP.

SQPOLL was not re-run: the extra kernel thread still contends with
four workers on two physical cores.

## Verified observations (measured)

- MSG_ZEROCOPY on UDP: the feature self-disables after a 5 ms grace and
  falls back to the copy path. This kernel copies loopback datagrams at
  send time, sends no completion notifications, and never references
  the user pages. The disable fires in every ZC-enabled run. Isolated
  one-shot with `FDS_UDP_ZEROCOPY=1`: 16.45 Gbps sent, 16.14 Gbps
  echoed, 98.07% completion (`fds-udp-zc.txt`). Same-day copy-path
  isolated epoll: 19.94 Gbps. The two modes are not ranked: ZC also
  collapsed on a second run of the same engine instance (0.63 Gbps
  sent, 0.00 Gbps echoed in `fds-udp-zc.txt` from the characterization
  pass; the isolated one-shot overwrote that file with the 16.45 Gbps
  single run). `fds-udp-zc-x3.txt` keeps the three-run spread: 5.15 /
  21.26 / 17.96 Gbps with ZC versus 20.81 / 19.33 / 17.48 Gbps copy
  path.
- io_uring TCP: a write-only flood stalled the 2026-08-28 client
  (`io_uring-tcp.txt` in that snapshot). The 2026-08-29 drain-echo
  client completes: 5.79 Gbps vs epoll 9.78 Gbps.
- io_uring SQPOLL: the kernel polling thread loses on 2C/4T. Not
  re-run on 2026-08-29.
- AF_XDP on veth without an XDP redirect program: bind succeeds
  (copy mode); RX stays at 0 pps for both FDS and xdpsock.

## Stacks not run on this platform

The following stacks need hardware or build tooling this machine
does not provide. They are not measured here.

- DPDK and F-Stack: require a NIC with DPDK-capable drivers. The only
  live link here is Wi-Fi. The wired port has no carrier.
- msquic: requires a QUIC build environment and a certificate store.
- Netty: the netty-all artifact is not retrievable through this
  network. The JDK is present.
- quiche: requires BoringSSL, which needs NASM. NASM is not installed.

## HTTP files in the 2026-08-26 snapshot

FDS is a transport engine, not an HTTP server. The 2026-08-26 snapshot
directory also holds HTTP/1.1, HTTP/2, and HTTP/3 comparison files
measured on the same machine (nginx, h2o, Caddy, Seastar, axum, Hyper,
actix-web, and related rows). Those files are raw tool output. They
are not ranked in the tables above. The 2026-08-28 and 2026-08-29
snapshots have no HTTP files.
