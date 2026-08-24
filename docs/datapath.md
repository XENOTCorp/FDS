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

### 1a. io_uring — already in-tree, needs the production settings

`fds-core/src/io_uring_reactor.rs` exists (feature `io-uring`, enabled
by default) and was A/B'd: 10.25 vs 16.02 Gbps TCP echo — the ring
LOST because the experimental path runs no SQPOLL and no registered
buffers/files. The production recipe:

- `IORING_REGISTER_FILES` — fixed files, skips the file table lookup
  per op (the engine's sockets are long-lived: perfect fit).
- `IORING_REGISTER_BUFFERS` — pre-registered receive buffers; recv
  writes directly into them (kernel->user copy removed on the receive
  side).
- `IORING_SETUP_SQPOLL` — kernel-side submission thread; removes the
  syscall on the submit path. Config exists: `FDS_REACTOR_IO_URING_ENTRIES`,
  `FDS_REACTOR_IO_URING_SQ_THREAD` (currently 0 = no SQPOLL).
- Multishot recv / `IORING_OP_PROVIDE_BUFFERS` for the accept/recv
  steady state.

Expected: the epoll loop is already batched and tight, so io_uring wins
only when registered buffers remove the recv copy AND SQPOLL removes the
submit syscall. Re-run `--bench-tcp-against` after wiring those two;
without them io_uring is a regression on 2C/4T (measured).

### 1b. AF_XDP — raw frames to userspace, no sk_buff

`fds-core/src/af_xdp.rs` implements the frame pipeline (Eth/IPv4/UDP
parse, checksum, MAC-swap/TTL echo). Requirements to make it the
datapath:

1. **A NIC with XDP support.** iwlwifi (this laptop) does NOT support
   XDP. Candidates on real hardware: ixgbe/i40e/ice (Intel),
   mlx5 (NVIDIA), e1000e (no). Test locally with a **veth pair**
   (veth supports generic XDP):
   ```sh
   sudo ip link add veth0 type veth peer name veth1
   sudo ip link set veth0 up; sudo ip link set veth1 up
   # attach a generic-XDP program, or use the XDP_FLAGS_SKB_MODE path
   ```
2. **UMEM + rings.** `XDP_UMEM_REG` (chunked user memory), fill/completion
   rings + rx/tx rings (the af_xdp.rs module builds the frame pipeline
   on top of this). The engine's per-core design maps 1:1 to AF_XDP
   queues: one UMEM + ring set per worker, queue pinned to the core.
3. **XDP program attach.** Load a minimal XDP_PASS/redirect program
   (`bpftool prog load ... xdpgeneric` on veth) so frames reach the
   socket; or use a libbpf-rs/aya-based loader in Rust.
4. **Busy-poll the rings** (rx->process->tx->refill) instead of the
   epoll loop — this is the true "IPC 2.4" path: no syscalls, no skb
   alloc (which is where this kernel's init_on_alloc tax lives).

Checksum/GSO: AF_XDP bypasses GRO/GSO — the driver offloads or you do
it in user space (af_xdp.rs already computes checksums).

### 1c. DPDK (Rust: dpdk-rs) — the heavy option

Only for real NICs with DPDK PMD support. Needs hugepages, UIO/VFIO,
and binding the device (`dpdk-devbind.py -b vfio-pci 0000:03:00.0`).
Overkill while AF_XDP covers the same ground with a stock kernel; keep
as the fallback if the target NIC lacks XDP.

### 1d. Order of attack

1. veth + generic XDP + af_xdp.rs frame pipeline end-to-end on this
   laptop (proves the code without new hardware).
2. io_uring production settings (registered files/buffers, SQPOLL) +
   re-A/B against epoll (`--bench-tcp-against` / `--bench-udp-against`).
3. Real NIC (any XDP-capable Intel/NVIDIA server card) for the wire
   numbers and the AF_XDP full datapath.

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
