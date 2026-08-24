# Datapath architecture: kernel bypass, CPU targeting, Atomos rulesets

Design notes for the next stage of FDS/Atomos. Three threads: how to
get off the kernel datapath, how to target AMD/Intel CPUs, and the
Atomos ruleset architecture with proposed extensions. Measured context
in `bench-results/` (root-measurements.txt, reactor-compare.txt,
msg-zerocopy-compare.txt, atomos-h2h3.txt).

## 1. Kernel-bypassed datapaths

The measured wall is the kernel: mpstat shows %sys+%soft at 54-72% of
CPU under loopback load, and three "modern" optimizations (PGO,
io_uring naive, MSG_ZEROCOPY) all regressed on this stripped kernel.
Getting off the kernel path is the only remaining big lever.

### 1a. io_uring — measured: does not beat epoll on this box

`fds-core/src/io_uring_reactor.rs` exists (feature `io-uring`, enabled
by default). Measured against the RUNNING engine per strategy
(bench-results/reactor-compare.txt, 2026-08-24) — the in-process
`--bench`/`--bench-large` modes ignore the strategy env, so the numbers
below come from `--bench-udp-against` / `--bench-tcp-against`:

| strategy | UDP echo (4x60 KiB) | TCP echo (4x60 KiB) |
| --- | --- | --- |
| epoll busy-poll | 10.31 Gbps (99.4%) | 20.22 Gbps |
| io-uring | 10.14 Gbps (99.8%) | stalls (no output) |
| io-uring SQPOLL | 5.65 Gbps (97.8%) | 0.21 Gbps |

SQPOLL's kernel submission thread contends with 4 workers on 2 physical
cores and loses badly; plain io_uring matches epoll on UDP but its TCP
accept/echo path stalls under this load. **epoll-busy-poll stays the
default** — the recipe below is what it would take to flip that on
server-class hardware (registered files/buffers, multishot recv):

- `IORING_REGISTER_FILES` / `IORING_REGISTER_BUFFERS` / multishot recv.
- Config exists: `FDS_REACTOR_IO_URING_ENTRIES`,
  `FDS_REACTOR_IO_URING_SQ_THREAD` (0 = no SQPOLL).

### 1b. AF_XDP — RX proven end-to-end on veth; TX needs a real NIC

`fds-core/src/af_xdp.rs` implements the frame pipeline (Eth/IPv4/UDP
parse, checksum, MAC-swap/TTL echo) on a real AF_XDP socket (umem +
rx/tx/fill/completion rings + bind). Status on this machine
(2026-08-24, `scripts/veth-af-xdp.sh`):

- **Toolchain unblocked**: `xbps-install -Sy clang bpftool` installed;
  the XDP steering program (`scripts/xdp_redirect.c`, BTF-defined
  XSKMAP, no libbpf headers) compiles and attaches in driver mode.
- **XSK bind on veth works as root** (unprivileged BPF is off on this
  kernel: `CONFIG_BPF_UNPRIV_DEFAULT_OFF=y`; `bind` EPERMs as a
  non-root user).
- **Pinned-map registration added** (`XskSocket::register_in_map`,
  `FDS_AF_XDP_XSKMAP`): the engine updates the XSKMAP via
  `BPF_OBJ_GET` + `BPF_MAP_UPDATE_ELEM`. Two kernel quirks found and
  worked around: plain `open()` on a pinned XSKMAP returns EIO (use
  `BPF_OBJ_GET`), and `BPF_MAP_UPDATE_ELEM` takes *pointers* to
  key/value in `bpf_attr`.
- **RX proven**: a crafted L2 frame from veth1 is XDP_REDIRECTed into
  the engine's RX ring, validated, checksummed and echoed into the TX
  ring ("af_xdp thread stopped (1 forwarded, 0 dropped)").
- **TX blocked by veth, not fds**: veth has no XSK TX path (no
  ndo_xsk_wakeup), so the echoed frame stays in the TX ring. A real
  XDP-capable NIC (ixgbe/i40e/ice, mlx5) completes the loop. The
  laptop's only live link is wifi; enp0s25 is a non-XDP ethernet with
  no carrier.

### 1c. DPDK (Rust: dpdk-rs) — the heavy option

Only for real NICs with DPDK PMD support. Needs hugepages, UIO/VFIO,
and binding the device (`dpdk-devbind.py -b vfio-pci 0000:03:00.0`).
Overkill while AF_XDP covers the same ground with a stock kernel; keep
as the fallback if the target NIC lacks XDP. Not runnable here (no
DPDK-PMD NIC with a carrier).

### 1d. Order of attack

1. **DONE**: veth + driver-mode XDP + af_xdp.rs RX pipeline end-to-end
   (proves the code without new hardware); XSK TX needs a real NIC.
2. **DONE (measured)**: io_uring production settings A/B — SQPOLL and
   registered ops do not beat epoll on 2C/4T (reactor-compare.txt);
   re-verify on server hardware.
3. Real NIC (any XDP-capable Intel/NVIDIA server card) for the wire
   numbers, XSK TX, and the full AF_XDP datapath.

## 2. AMD / Intel CPU targeting

### What exists

`build/build.sh` already detects /proc/cpuinfo SIMD flags and emits
`-C target-cpu=native -C target-feature=+avx2,+bmi1,...` (see
`build/detect.sh`); `fds-detect` prints the summary. `TARGET_CPU`
pins a specific uarch for cross-machine builds.

### Portable baseline: x86-64-v3

For a binary that must run on both families without per-machine
recompiles, target the common denominator of Haswell+/Excavator+/Zen+:

```sh
TARGET_CPU=x86-64-v3 bash build/build.sh --release
```

x86-64-v3 = AVX2 + BMI1/BMI2 + FMA + popcnt + lzcnt — everything the
hot path uses (SIMD checksums, memcpy tuning). v4 (AVX-512) is
Intel-skewed (and downclocks on some Xeons) — don't ship v4 as the
baseline. Runtime dispatch (cpuid at startup) is only worth it if you
ship ONE binary for unknown hardware; per-machine builds (the current
design) make it unnecessary.

### Family notes

- **Intel**: AVX-512 on Xeon downclocks (older parts); hybrid Alder
  Lake+ needs affinity that avoids the E-cores for the datapath
  (`taskset`/`sched_setaffinity` — the engine already pins, but the
  pin map must pick P-cores; extend `pin_to_core` with a topology
  query when it matters).
- **AMD Zen**: 2 FP pipes (AVX2/FMA scheduling differs from Intel),
  larger L3, per-CCX NUMA. On EPYC: run one worker per CCX, use
  `numactl`:
  ```sh
  numactl --cpunodebind=0 --membind=0 ./target/release/fds
  ```
  and give the engine `CoreConfig::threads` = cores on node 0.
- **Both**: 64-byte cache lines; the hot/cold ConnTable split and
  padded counters already respect this. `cargo-show-asm` + `perf
  stat -e L1-icache-load-misses` after any uarch change; the
  `build/PROFILES.md` matrix documents the flag stack.

## 3. Atomos ruleset architecture

### Current design (src/kernel/rules.rs)

- JSON `{"rules":[{"id","module","methods","include":["pre/*"],"exclude":[]}]}`
  -> `Ruleset::parse` -> packed glob patterns (`pre/*` compiles to a
  zero-alloc prefix matcher). `assert_disjoint` rejects overlapping
  include/exclude at parse time.
- `match_path`/`match_method` return the first matching `Rule` (no
  allocation; method mask first, then patterns).
- Router holds `rules` and `modules` behind `ArcSwap` -> hot reload via
  the `rules.reload` atom (JSON rules are the ONLY hot-reloadable unit;
  Rust modules are compiled in).
- `pre_module`/`post_module` are GLOBAL hooks (one pre, one post for
  every request); per-request `flags` (FLAG_LOG, FLAG_METRICS_SKIP,
  FLAG_NO_POST) and cache directives (Global/Named TTL) are set by the
  matched module.
- Control surface: Unix-socket atoms (`status`, `rules.reload`,
  `refresh-endpoints`, `server.stop`), molecules as named atom lists.

### Proposed extensions (ranked by value)

1. **Per-rule hooks + per-rule governor.** Today rate limiting is a
   single global `Governor`. Move `governor` config per rule
   (rps, burst, window) and allow `"pre"`/`"post"` module names per
   rule instead of only globally. Small change to `Rule` (add
   `pre: Option<String>`, `post: Option<String>`, `governor: Option<...>`);
   matching already returns the rule, so dispatch can branch on it.
2. **Path parameters.** `include: ["/api/{id}/*"]` — the packed
   prefix matcher extends naturally to a `{name}` segment capture
   (one extra scan; still zero-alloc, capture as a slice range into
   the request path). This is what makes the ruleset usable for
   REST-style endpoints without per-endpoint Rust modules.
3. **Header conditions.** `HeaderRule` exists; expose it in the JSON
   (`"headers": [{"name":"Authorization","prefix":"Bearer "}]`) and
   evaluate after method+path match. Keep the check order: method
   mask -> path -> headers -> rule.
4. **Rule priorities + metrics per rule.** Add `"priority": n`
   (default 0, higher wins) so the "first match" ordering is explicit;
   add per-rule counters (requests, bytes, errors) fed into the same
   `Metrics` structure (cheap: one LineAtomicU64 pair per rule in a
   side table, not in the hot Rule struct).
5. **Dynamic rule toggles.** A `"enabled": false` rule + a
   control atom (`rules.toggle <id>`) — trivially expressed in the
   existing rules.reload flow, avoids full re-parses for
   feature-flag-style toggles.

### Ruleset tuning checklist (what matters for the datapath)

- Keep rules few and include-prefixes long; matching is linear over
  rules, so order the JSON by hit frequency.
- Exclude paths from `static` before adding other modules
  (disjoint include/exclude is enforced — use it).
- Put `metrics` behind its own rule as the bench does (see
  `scripts/bench-h23.sh` rules.json) so instrumentation is opt-in.
- Hot-reload cost is a re-parse + ArcSwap swap; do it off the request
  path (control socket), never from a module.
