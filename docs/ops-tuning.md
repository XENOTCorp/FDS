# FDS transport engine — ops / system tuning

Operational tuning guide for the `fds` binary (crates/fds-core) on Linux.
The engine is a busy-polling, batched, zero-allocation datapath; the
settings below remove kernel and NIC interference so the measured
loopback/NIC numbers reflect the engine itself.

The crate is a BINARY package: runtime configuration lives in
`config.json` (see `crate::config`), and the table at the end maps each
tuning to its `crate::config` key. Sysctls and `ethtool` settings are
host-level and sit outside the crate. All `sysctl -w` values are
non-persistent unless also written to `/etc/sysctl.conf` (or a file in
`/etc/sysctl.d/`); `ethtool` settings reset on link down/up and on
reboot unless applied via a udev rule or systemd unit.

## 1. NIC tuning

### Interrupt coalescing off

The engine busy-polls (epoll with timeout 0, see `ReactorConfig::busy_poll`);
interrupt-driven delivery just adds latency and IRQ noise. Disable
coalescing on the datapath queue:

```sh
ethtool -C eth0 rx-usecs 0 tx-usecs 0 rx-frames 0 tx-frames 0
```

`rx-usecs 0` forces an interrupt per frame instead of batching — the
right trade when userspace polls anyway. With adaptive coalescing
(`adaptive-rx on`), disable it too:

```sh
ethtool -C eth0 adaptive-rx off adaptive-tx off
```

### Jumbo frames (MTU 9000)

For 9000-byte payloads the engine datagram size is bounded by the MTU;
raise it on both the NIC and the interface:

```sh
ip link set dev eth0 mtu 9000
```

Keep it consistent across the path — any hop at 1500 will fragment or
drop. The loopback device defaults to MTU 65536, so loopback benches
(`--bench`) are unaffected.

### Ring sizes

The NIC rx/tx ring is the kernel-side queue the busy poll drains. Size it
to the worst-case burst (e.g. 2048–4096 entries) so packets are not
dropped while the engine is inside a `recvmmsg` batch:

```sh
ethtool -G eth0 rx 4096 tx 4096
```

Ring memory is per-queue; 4096 × 2 KB descriptors per queue is the
common ceiling. Verify with `ethtool -g eth0` (section 7).

## 2. Kernel tuning

### Socket buffer maxima

`rmem_max`/`wmem_max` cap `SO_RCVBUF`/`SO_SNDBUF`; the engine requests
4 MiB per socket (`UdpConfig::rcvbuf`/`sndbuf`, same for TCP), so the
caps must be at least that:

```sh
sysctl -w net.core.rmem_max=16777216
sysctl -w net.core.wmem_max=16777216
```

Without this, `setsockopt` silently clamps the buffers to the default
(≈212 KiB), which drops UDP bursts.

### Receive backlog

`netdev_max_backlog` bounds the per-CPU packet queue feeding the
protocol stacks (the queue between `net_rx_action` and the socket):

```sh
sysctl -w net.core.netdev_max_backlog=65536
```

Only matters under NIC load; loopback benches are unaffected.

### TCP fast open

`tcp_fastopen` enables TFO (one RTT saved on repeat connections):

```sh
sysctl -w net.ipv4.tcp_fastopen=3   # 1=client, 2=server, 3=both
```

The engine side is gated by `TcpConfig::fastopen` (the TFO listen-queue
length; 0 = off). The kernel sysctl is the master switch; the config key
must also be nonzero. Caveat: TFO allows a SYN carrying data, so it must
not be enabled for services that trust the source address before the
handshake completes.

### TIME_WAIT reuse

`tcp_tw_reuse` lets a new connection reuse a TIME_WAIT socket for
outbound connections only:

```sh
sysctl -w net.ipv4.tcp_tw_reuse=1
```

Caveats (documented in the kernel docs and the engine spec):
- It applies to the **client** side (outbound) only; it does nothing for
  the server-side TIME_WAIT pileup.
- It relies on the TIME_WAIT socket's timestamp being older than the new
  SYN's; with a NAT or with clock skew between peers, sequence/PAWS
  confusion can corrupt connections. Do not enable behind NAT where
  timestamps are unreliable.
- It does not create a security problem by itself, but combined with
  weak ISN randomization it can enable spoofed-connection reuse.
- For a server, prefer `SO_REUSEADDR` + `SO_REUSEPORT` (which the engine
  sets via `TcpConfig::reuseport`) or raise `net.ipv4.tcp_fin_timeout`.
  There is no crate config key for `tcp_tw_reuse`; it is host-level.

### RPS / XPS (per-core steering)

With a single-queue NIC (or a veth/loopback path), one IRQ drives all
traffic. RPS (receive) and XPS (transmit) spread it across the engine's
cores:

```sh
# RPS: steer to cores 2-7 (bitmask 0xfc)
echo fc > /sys/class/net/eth0/queues/rx-0/rps_cpus
# XPS: same mask on the tx queue
echo fc > /sys/class/net/eth0/queues/tx-0/xps_cpus
```

Match the mask to the cores the engine pins to (`CoreConfig::pin_cores`,
`isolcpus` in section 4). RPS adds a little per-packet overhead (the
packet is moved between CPUs); it pays off when one queue saturates a
single core. Modern NICs with RSS + per-queue IRQs are better left to
the hardware (steer with `ethtool -L eth0 combined N` instead).

## 3. Application settings

### SO_REUSEPORT + SO_INCOMING_CPU

`SO_REUSEPORT` lets the engine open one socket per worker on the same
port; the kernel load-balances by 4-tuple hash (per-flow). This is the
per-core distribution mechanism: each worker owns its socket, its
poller and its counters.

`SO_INCOMING_CPU` (`UdpConfig::incoming_cpu`, default **off**) makes
reuseport selection prefer the socket matching the CPU that runs the
receive path — on a NIC with RSS queues + IRQ affinity that pins a flow
to the socket on the IRQ's core (no cross-core ping-pong). On loopback
there is one "queue" and one RX softirq CPU at a time, so enabling it
pins *all* traffic to a single worker — keep it off unless the NIC's
RSS/IRQ affinity is configured to match. `reuseport` stays on in both
cases.

### Hugepages (2 MiB THP)

The engine maps its receive/ring buffers (mol `Buffer` pools) with
`mmap`; backing them with 2 MiB pages removes TLB misses in the hot
loop. Transparent Huge Pages is the zero-friction route:

```sh
sysctl -w vm.nr_overcommit_hugepages=0
echo always > /sys/kernel/mm/transparent_hugepage/enabled
echo madvise > /sys/kernel/mm/transparent_hugepage/defrag
```

`always` + `madvise` defrag gives the kernel freedom to back large
`mmap`s with THP. For deterministic huge pages (no compaction stalls)
reserve explicit 2 MiB pages and pass `MAP_HUGETLB` — that is a
future engine option; today the crate reads nothing for it, so THP
is the documented setting.

### CPU pinning and isolcpus

Pin engine threads to physical cores (`CoreConfig::pin_cores = true`),
and keep the kernel off those cores so nothing preempts the poll loop:

```sh
# kernel cmdline: reserve cores 2-7 for the engine
isolcpus=2-7 nohz_full=2-7 rcu_nocbs=2-7
```

`isolcpus` removes the cores from the scheduler; `nohz_full` stops the
per-core tick; `rcu_nocbs` offloads RCU callbacks. Set
`CoreConfig::threads` to the number of reserved cores (0 = one per
logical CPU, 2x the physical count on hyperthreaded machines) and make
the IRQ affinity (section 2, RPS/XPS) match.

## 4. Offloads

Check current offload state with `ethtool -k eth0` (section 7).

### Checksum offload

The engine computes IP/TCP/UDP checksums in userspace (`crate::checksum`)
and the parser validates them; leaving the NIC checksum offload on is
harmless (the NIC computes on transmit, validates on receive) but the
engine does not rely on it:

```sh
ethtool -K eth0 tx on rx on     # default; fine as-is
```

Keep it on for bulk TCP traffic; the CPU savings are real. The engine's
checksums are the *security* path (the [SEC] standard) and are always
computed regardless of the NIC.

### TSO / GSO

TSO (NIC) / GSO (kernel software) coalesce the transmit path for large
TCP writes. On for TCP by default:

```sh
ethtool -K eth0 tso on gso on
```

For UDP, the engine drives segmentation itself via `UDP_SEGMENT` (GSO),
gated by `UdpConfig::gso_segment_size` — the kernel then hands the NIC
one large buffer and the hardware segments. Set `gso_segment_size` to
the MSS (e.g. 1448 at MTU 9000) and keep `gso on`. For raw throughput
benches measuring the *engine* rather than the NIC, `tso off` gives a
per-segment accounting instead.

### LRO / GRO

GRO (kernel) / LRO (NIC) merge receive segments before the engine sees
them. For a per-packet dataplane, merged super-packets distort the
packet count and the batch ring invariants:

```sh
ethtool -K eth0 gro on      # engine honors UDP_GRO via UdpConfig::gro
ethtool -K eth0 lro off     # LRO is a single-flow merge; keep off
```

`UdpConfig::gro` mirrors the kernel `UDP_GRO` socket option (via
`recvmmsg`'s `MSG_GRO` path); enable both together on NIC-heavy
workloads, keep both off for latency-sensitive or per-packet workloads.
Loopback is unaffected (GRO only fires on real NICs).

### MSG_ZEROCOPY

`MSG_ZEROCOPY` avoids the copy of large datagrams by pinning the user
pages and letting the NIC DMA from them. Enable only for large datagrams
(≥ 1 MTU-ish), where the copy cost dominates:

```sh
ethtool -K eth0 tx-udp-segmentation on   # prerequisite for UDP zc paths
```

Engine keys: `UdpConfig::zerocopy` (socket option) and
`ZeroCopyConfig::udp_zerocopy` (the transport's zc policy). Caveats:
- The send buffer is borrowed until the NIC completes; do not reuse the
  buffer until the completion queue signals (the engine's batch ring
  handles this).
- Not supported on loopback for UDP in older kernels — verify per
  platform; the transport falls back to a copy when `sendmsg` returns
  `EOPNOTSUPP`.
- Costs a page-pin per datagram; below ~1 KB it is slower than copying.

## 5. SCTP

The engine links `libsctp` (feature `sctp`); the kernel module and the
userspace library must both be present:

```sh
modprobe sctp
# verify: dmesg | grep sctp, or check /proc/net/sctp/assocs exists
```

Relevant sysctls:

```sh
sysctl -w net.sctp.auth_enable=0          # default; no SCTP-AUTH
sysctl -w net.sctp.reconf_enable=1        # stream reconfiguration (if used)
sysctl -w net.sctp.association_max_retrans=5
```

The engine-side association tuning (streams, burst, partial delivery)
is `SctpConfig` (`init_max_streams`, `max_burst`,
`partial_delivery_point`); `sctp.nodelay` maps to `SCTP_NODELAY`. If
`socket(AF_SCTP, ...)` fails at runtime the transport skips gracefully
with an `eprintln` — check `modprobe sctp` first.

## 6. Verification

Confirm every setting after boot/link-up:

```sh
ethtool -c eth0        # coalescing: rx-usecs/tx-usecs = 0
ethtool -g eth0        # ring sizes actually in effect
ethtool -k eth0        # offloads: tso/gso/gro on, lro off, tx/rx csum on
ip link show eth0      # mtu 9000
sysctl net.core.rmem_max net.core.wmem_max net.core.netdev_max_backlog
sysctl net.ipv4.tcp_fastopen net.ipv4.tcp_tw_reuse
cat /proc/net/softnet_stat    # drop column grows => backlog too small
cat /proc/interrupts          # per-CPU IRQ counts: pins/RPS steering as expected
cat /sys/class/net/eth0/queues/rx-0/rps_cpus   # steering mask
cat /sys/kernel/mm/transparent_hugepage/enabled
```

`/proc/interrupts` is the quick sanity check for steering: with
`isolcpus` + RPS/XPS the datapath IRQs should land only on the reserved
cores and stay there (watch the delta, not the absolute count).
`/proc/net/softnet_stat` column 2 (drops) growing under load means
`netdev_max_backlog` is too small.

## 7. Quick reference

| Tuning | Setting | FDS config key (`crate::config`) |
| --- | --- | --- |
| IRQ coalescing off | `ethtool -C rx-usecs 0` | none (host-level; complements `ReactorConfig::busy_poll`) |
| Jumbo frames | `ip link set mtu 9000` | none (host-level; bounds `UdpConfig::gso_segment_size`) |
| Ring sizes | `ethtool -G rx/tx 4096` | `ReactorConfig::max_events` (userspace event array) |
| Socket buffer caps | `net.core.rmem_max/wmem_max` | `UdpConfig::rcvbuf`/`sndbuf`, `TcpConfig::rcvbuf`/`sndbuf` |
| Backlog | `net.core.netdev_max_backlog` | none (host-level) |
| TCP fast open | `net.ipv4.tcp_fastopen` | `TcpConfig::fastopen` |
| TIME_WAIT reuse | `net.ipv4.tcp_tw_reuse` | none (host-level; use `TcpConfig::reuseport` instead for servers) |
| RPS/XPS steering | `rps_cpus`/`xps_cpus` masks | `CoreConfig::threads`, `CoreConfig::pin_cores` (match the mask) |
| SO_REUSEPORT | per-socket opt | `UdpConfig::reuseport`, `TcpConfig::reuseport`, `SctpConfig::reuseport` |
| SO_INCOMING_CPU | per-socket opt | `UdpConfig::incoming_cpu` |
| Hugepages | THP `always` | none (engine maps via `mmap`; no key yet) |
| CPU pinning | `sched_setaffinity` | `CoreConfig::pin_cores` |
| CPU isolation | `isolcpus=… nohz_full=…` | `CoreConfig::threads` (reserved cores) |
| Checksum offload | `ethtool -K tx on rx on` | none (engine always computes: `crate::checksum`) |
| TSO/GSO | `ethtool -K tso on gso on` | `UdpConfig::gso_segment_size` (UDP_SEGMENT) |
| LRO/GRO | `ethtool -K gro on lro off` | `UdpConfig::gro` |
| MSG_ZEROCOPY | `ethtool -K tx-udp-segmentation on` | `UdpConfig::zerocopy`, `ZeroCopyConfig::udp_zerocopy` |
| SCTP module | `modprobe sctp` | `SctpConfig::*` (nodelay, init_max_streams, max_burst, partial_delivery_point) |
| init_on_alloc off | `init_on_alloc=0` boot param | none (kernel-wide; see §8) |
| H2/H3 (tokio) | builder flags in `h2serve.rs`/`h3serve.rs` | `Config::http2`, `Config::http3`, `Config::workers` (see §9) |

## 8. This kernel's datapath tax: init_on_alloc (measured)

Measured on this box (2026-08-24, see `bench-results/root-measurements.txt`):
the `sched:sched_switch` call graph shows ~18% of context switches under
TCP load originate in `tcp_sendmsg -> skb_page_frag_refill ->
alloc_pages -> clear_highpages_kasan_tagged`. The kernel is built with
`CONFIG_INIT_ON_ALLOC_DEFAULT_ON=y` — **every page allocated for an skb
is zeroed before use** (a hardening feature that prevents
uninitialized-memory leaks). That memset is pure datapath cost.

Check the runtime state:

```sh
cat /sys/module/kernel/parameters/init_on_alloc   # Y/N
zcat /proc/config.gz | grep INIT_ON_ALLOC
```

Disable at boot (tradeoff: uninitialized memory can leak kernel data to
userspace; acceptable on a bench/dev box, not on a multitenant host):

```sh
# GRUB (Debian/Ubuntu/Void-with-grub):
#   edit /etc/default/grub, add init_on_alloc=0 to GRUB_CMDLINE_LINUX_DEFAULT
sudo grub-mkconfig -o /boot/grub/grub.cfg && sudo reboot
# EFISTUB/systemd-boot: append init_on_alloc=0 to the kernel cmdline entry
# rEFInd: add the option in /boot/refind_linux.conf
```

Verify after boot: `cat /sys/module/kernel/parameters/init_on_alloc` → `N`,
and re-run `perf record -g -e sched:sched_switch` — the alloc_pages chain
should drop out of the top of the report.

### This kernel is stripped (measurement limits)

`bpftrace -l` shows no kprobes and only a handful of net tracepoints;
`perf record -e sched:sched_switch` works, but `kprobe:udp_sendmsg`
does not exist; and UDP `MSG_ZEROCOPY` silently copies (verified by the
mutation probe — `bench-results/msg-zerocopy-compare.txt`). If you want
the full measurement surface, boot a stock mainline kernel
(CONFIG_KPROBES=y, net/skb/udp tracepoints, working UDP zerocopy) and
re-run the battery. This is the single highest-value kernel change for
both *measuring* and *running* faster on this laptop.

### SCTP module vs the modprobe blacklist

`/etc/modprobe.d/xenot-blacklist.conf` maps `install sctp /bin/true`
(your hardening), so `modprobe sctp` silently no-ops. Options:

```sh
# session-only (what we used): bypass the install rule
sudo insmod /lib/modules/$(uname -r)/kernel/net/sctp/sctp.ko.zst
# permanent: load at boot without touching the blacklist
echo sctp | sudo tee /etc/modules-load.d/sctp.conf
# or remove the line from /etc/modprobe.d/xenot-blacklist.conf
```

## 9. H2/H3 (tokio) tunables

The H2/H3 paths (Atomos `src/net/h2serve.rs`, `h3serve.rs`) wrap the
`h2` 0.4 / `h3` 0.0.8 crates. The builder flags below are the knobs;
apply them in `h2serve::handle` / `h3serve::handle_conn`. Current code
sets only `max_concurrent_streams(256)` on h2 and nothing on h3.

### h2::server::Builder

```rust
h2::server::Builder::new()
    .max_concurrent_streams(256)          // in-flight streams per conn
    .initial_window_size(1 << 20)         // per-stream flow-control window
    .initial_connection_window_size(16 << 20) // connection window (throughput!)
    .max_frame_size(16 * 1024)            // 16 KiB data frames
    .max_header_list_size(64 * 1024)      // HPACK header block cap
    .max_send_buffer_size(1 << 20)        // per-stream send buffering
    .header_table_size(4096)              // HPACK dynamic table (bytes)
    .handshake(io).await
```

Throughput on high-BDP loopback/link is bounded by the flow-control
windows: raise `initial_connection_window_size` first. `header_table_size`
trades memory for header compression on repetitive request sets.

### h3 / quinn

h3 0.0.8 exposes few builder options (max_field_section_size etc.); the
real knobs are quinn's transport config (`quinn::TransportConfig`):

```rust
let mut tc = quinn::TransportConfig::default();
tc.max_bi_streams(256u16.into());        // bidirectional streams
tc.max_uni_streams(256u16.into());
tc.send_window(16 << 20);                // per-conn send window
tc.receive_window(16 << 20);             // per-conn recv window
tc.stream_receive_window(1 << 20);       // per-stream recv window
tc.initial_mtu(1452);
tc.congestion_controller_factory(Arc::new(quinn::congestion::CubicConfig::default()));
```

`quinn::congestion` has NewReno / Cubic / BBR factories; Cubic or BBR
beats NewReno on real links. Enable 0-RTT on the client (quinn
`enable_0rtt`) to cut H3 handshakes; the server must persist session
tickets (TlsHold already issues tickets).

### Preconditions for the tokio path to be fast

1. **Pin tokio workers.** `atomos-proto` runs `current_thread` today —
   every accepted connection and stream is one thread. A
   `multi_thread` runtime with `worker_threads = workers` and pinned
   affinity (match `Config::cpu_pin`) is the first precondition for
   H2/H3 scaling past one core.
2. **Never block in `serve_one`.** The h2/h3 handlers are spawned per
   stream on the runtime; a blocking call (fs, lock, slow module)
   stalls every stream on that worker. The H1 fast path's zero-alloc
   rule applies here too: `serve_one` already reuses `BytesMut` bodies
   but still allocates per response (`Bytes::copy_from_slice`).
3. **Warm the tables.** HPACK/QPACK compress best after the dynamic
   table is warm (measured: H2 wire 149 B/req -> 12 B/req steady).
   Keep response header sets small and constant; pre-encode static
   responses as `Bytes::from_static`.
4. **Sized flow-control windows** (above) are the throughput
   precondition; defaults are conservative.

## 10. Verification (H2/H3)

```sh
bash scripts/bench-h23.sh 2000        # FDS script; boots atomos-proto + benches
curl --http2-prior-knowledge -s http://127.0.0.1:8090/metrics   # h2 counters
curl -s http://127.0.0.1:8090/metrics | grep -E 'h2|h3'         # prometheus text
```
