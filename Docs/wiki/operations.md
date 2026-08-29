# Operations

Host and kernel settings that affect the datapath. The engine does not apply these. You apply them on the host.

## NIC

The examples use `eth0`. Replace it with your interface name.

Disable interrupt coalescing on the datapath queue:

```sh
ethtool -C eth0 rx-usecs 0 tx-usecs 0 rx-frames 0 tx-frames 0
ethtool -C eth0 adaptive-rx off adaptive-tx off
```

With busy-poll enabled the engine spins on the ready queue. Interrupts then add latency.

Jumbo frames:

```sh
ip link set dev eth0 mtu 9000
```

Loopback defaults to MTU 65536. Loopback benches do not need this change.

Ring sizes so bursts are not dropped while the engine is inside a batch:

```sh
ethtool -G eth0 rx 4096 tx 4096
```

## Kernel

Socket buffer maxima. The engine requests 4 MiB per socket. Without the caps, `setsockopt` clamps to about 212 KiB and UDP bursts drop.

```sh
sysctl -w net.core.rmem_max=16777216
sysctl -w net.core.wmem_max=16777216
sysctl -w net.core.netdev_max_backlog=65536
```

TCP fast open: `net.ipv4.tcp_fastopen=3` plus `tcp.fastopen` in config. A SYN can carry data. Do not enable this for a service that trusts the source address before the handshake completes.

TIME_WAIT reuse (`net.ipv4.tcp_tw_reuse=1`) applies to the client side only. For a server, use SO_REUSEADDR and SO_REUSEPORT. The engine sets both.

With a single-queue NIC, steer traffic across the engine cores with `rps_cpus` and `xps_cpus` masks. Match the mask to the pinned cores.

## Application

- SO_REUSEPORT: one socket per worker on the same port. The kernel load-balances by 4-tuple hash. This is the per-core distribution mechanism.
- SO_INCOMING_CPU (`udp.incoming_cpu`, default off): with NIC RSS and IRQ affinity, pins a flow to the socket on the IRQ core. On loopback it pins all traffic to one worker.
- Hugepages: back mmap buffer pools with 2 MiB pages (`transparent_hugepage/enabled=always`, defrag `madvise`) to remove TLB misses. The 4 MiB UDP receive slab is advised with `MADV_HUGEPAGE`.
- CPU isolation: reserve cores with `isolcpus=2-7 nohz_full=2-7 rcu_nocbs=2-7` on the kernel command line. Set `core.threads` to the number of reserved cores.

## Offloads

- Checksum offload: keep NIC checksums on for bulk traffic. The engine always computes its own checksums. The NIC setting does not change the security path.
- TSO/GSO: on for TCP by default. For UDP, the engine drives segmentation with `UDP_SEGMENT` (`udp.gso_segment_size`).
- GRO: enable `udp.gro` with the kernel `UDP_GRO` socket option on NIC-heavy workloads. LRO is a single-flow merge. Keep LRO off.
- MSG_ZEROCOPY: enable only for large datagrams. The send buffer is borrowed until the NIC completes. The engine batch ring handles this. On kernels where the copy path is silent, the engine disables the feature after a 5 ms grace.

## SCTP

The kernel module and libsctp must be present:

```sh
modprobe sctp
```

Engine keys are in `SctpConfig` (`init_max_streams`, `max_burst`, `partial_delivery_point`, `nodelay`). If `socket(..., IPPROTO_SCTP)` fails at runtime, the transport skips with a log line. Check the module.

## Verification

```sh
ethtool -c eth0
ethtool -g eth0
ethtool -k eth0
sysctl net.core.rmem_max net.core.wmem_max net.core.netdev_max_backlog
cat /proc/net/softnet_stat
cat /proc/interrupts
```

If the drops column in `softnet_stat` grows, the backlog is too small.

## Quick reference

| Tuning | Setting | FDS config key |
| --- | --- | --- |
| IRQ coalescing off | `ethtool -C rx-usecs 0` | none (host) |
| Jumbo frames | `ip link set mtu 9000` | none (host) |
| Ring sizes | `ethtool -G rx/tx 4096` | `reactor.max_events` |
| Socket buffer caps | `net.core.rmem_max` / `wmem_max` | `udp.rcvbuf` / `sndbuf`, `tcp.rcvbuf` / `sndbuf` |
| Backlog | `net.core.netdev_max_backlog` | none (host) |
| TCP fast open | `net.ipv4.tcp_fastopen` | `tcp.fastopen` |
| TIME_WAIT reuse | `net.ipv4.tcp_tw_reuse` | none (use `tcp.reuseport`) |
| RPS/XPS steering | `rps_cpus` / `xps_cpus` | `core.threads`, `core.pin_cores` |
| SO_REUSEPORT | per-socket option | `udp.reuseport`, `tcp.reuseport` |
| SO_INCOMING_CPU | per-socket option | `udp.incoming_cpu` |
| Hugepages | THP `always` | none (engine maps with mmap) |
| CPU pinning | `sched_setaffinity` | `core.pin_cores` |
| CPU isolation | `isolcpus`, `nohz_full`, `rcu_nocbs` | `core.threads` |
| Checksum offload | `ethtool -K tx on rx on` | none (engine always computes) |
| TSO/GSO | `ethtool -K tso on gso on` | `udp.gso_segment_size` |
| LRO/GRO | `ethtool -K gro on lro off` | `udp.gro` |
| MSG_ZEROCOPY | `ethtool -K tx-udp-segmentation on` | `udp.zerocopy` |
| SCTP module | `modprobe sctp` | `sctp.*` |

## Kernel build notes

A kernel built with `CONFIG_INIT_ON_ALLOC_DEFAULT_ON=y` zeroes every page allocated for an skb. That memset is datapath cost. Measured at about 18 percent of context switches under TCP load. Check with `cat /sys/module/kernel/parameters/init_on_alloc`. Disable with `init_on_alloc=0` on the kernel command line on a single-tenant host.

A stripped kernel limits measurement: kprobes may be absent, and UDP MSG_ZEROCOPY may copy silently. A stock mainline kernel restores the full measurement surface.
