# Same-box baselines: nginx, Seastar vs FDS/Atomos

Ceteris paribus on this exact machine (i5-5200U 2C/4T, kernel 7.2.0_1):
same root files (`index.html` = `<h1>hi</h1>`, `file64k.bin` = 64 KiB),
same `127.0.0.1:<port>`, same wrk (`-t4 -c100 -d5s`), 4 workers each
(nginx/Seastar `--smp 4`; Atomos `workers=4`), same load order.

## Head-to-head (wrk, 5 s)

| Server | cached page req/s | 64 KiB req/s | notes |
| --- | --- | --- | --- |
| **Atomos H1** (FDS epoll) | **85,652–95,870** | **27,398** | in-memory response cache / wire cache |
| h2o 2.2.6 (num-threads 4) | 69,858 | 20,547 | C, sendfile on 64 KiB (loopback re-copy penalty) |
| nginx `return 200` (mem) | 78,679 | — | nginx engine ceiling, no static path |
| nginx `open_file_cache` | 54,719 | **28,185** | tuned static path (sendfile) |
| nginx default static | 10,613 | 9,147 | per-request open/stat (8x penalty here) |
| Caddy 2.11.4 (Go, 4 procs) | 15,242 | 12,852 | Go runtime + per-request allocation overhead |
| Seastar httpd (io_uring demo) | 20,999 | 7,788 | demo app, no sendfile/response cache |

Atomos 64 KiB is the byte path (wire cache, 1.43–1.68 GB/s across runs);
with `ATOMOS_SF_MIN` lowered it switches to the sendfile path (see below).

## What the comparison shows

- **Atomos ≈ nginx's engine ceiling**: 85.7k vs 78.7k req/s (+9%) on the
  cached page — both are in-memory responses; the difference is the
  transport/parser (FDS epoll vs nginx epoll) and the epoll edge-trigger
  busy-poll loop.
- **nginx default static is 8x slower than Atomos cache-hit** because of
  per-request `open`/`stat` (measured: `open_file_cache` alone recovers
  5.2x). Atomos's response cache is the equivalent optimization.
- **On 64 KiB, Atomos now matches nginx tuned** (27.4k vs 28.2k req/s,
  1.68 vs 1.72 GB/s — parity). The old "24.4k" Atomos figure was
  bogus: the wrk target (`big.bin`) did not exist in the bench root, so
  it measured 404 error pages. The honest byte path (pre-encoded wire
  cache, one writev per response) is the equal of nginx's sendfile here.
- **sendfile is implemented (OutBody::File) but loses on this kernel's
  loopback below ~128 KiB**: measured A/B on the same build — 64 KiB:
  byte 27.4k vs sendfile 16.5k req/s; 128 KiB: even (~13k); 256 KiB:
  sendfile wins (8.6k = 2.09 GB/s vs 6.5k = 1.59 GB/s, +31%). This
  kernel's loopback datapath re-copies sendfile pages (same finding as
  the MSG_ZEROCOPY probe), so small transfers pay splice machinery for
  nothing. `ATOMOS_SF_MIN` (default 128 KiB) is the crossover default;
  on a real NIC sendfile wins from far smaller sizes and the threshold
  should be lowered.
- **Latency is not "bad" — it was an unfair comparison**: Atomos's
  earlier 1.31 ms figure was wrk's average under 100-conn load, quoted
  against nginx/Seastar's no-load curl ladders. Measured the same way:
  no-load p50 (curl x200, fresh conns) — **Atomos 273 µs** (cached page)
  / 364 µs (64 KiB) vs nginx 351 µs vs Seastar 585 µs; under identical
  wrk load — Atomos 1.31 ms vs nginx 1.93 ms vs Seastar 5.30 ms.
  Fastest both ways.
- **Seastar trails the field 4.1x on cached pages** (21.0k vs Atomos
  85.7k / nginx tuned 55.9k). It is the only one doing per-request
  cooperative scheduling + io_uring file reads with no response cache
  and no sendfile; its demo httpd is a framework sample, not a tuned
  static server — the point of the row is that Seastar-the-framework
  does not beat plain epoll here out of the box.
- **h2o (the C SOTA-class server) is +23–37% behind Atomos** on the
  cached page (69.9k vs 85.7–95.9k) and +33% on 64 KiB (20.5k vs
  27.4k; h2o uses sendfile, which this loopback re-copies). Caddy
  (Go) is 5.6x behind — the runtime overhead is the whole story there.
- **H2/H3 after the governor fix**: the memory governor was reading
  `/proc/self/status` twice per request (≈60% of H2-path CPU, found by
  perf). Cached RSS (100 ms TTL) + refcount-only body clones: H2 seq
  7.3k → 11.8–12.6k (+74%), H2 mux 15k → 72–78k (+380–420%), H3 5.0k
  → 7.2–8.0k (+44–61%). H2 mux now beats h2o's H1 number on this box.
- All three servers are kernel-dominated on this box (mpstat %sys+%soft
  > %usr; nginx IPC 0.50, Seastar IPC 0.59, Atomos IPC 0.54; 0
  cpu-migrations for all three).

## Method

- `scripts/bench-nginx.sh` (sudo; nginx needs root for runtime paths).
- `scripts/bench-seastar.sh <app_httpd>`; for a byte-fair short URL the
  bench files are symlinked at `/` (`/index.html`, `/file64k.bin`) and
  the script is run with `PAGE=/file/index.html BIG=/file/file64k.bin`.
- `scripts/bench-h2o-caddy.sh` (+ `scripts/h2o-bench.conf`): h2o
  `num-threads 4`, caddy `file-server` (GOMAXPROCS=4).
- The root dir is `/tmp/nginx-bench/root` (same content for all three;
  Seastar's demo serves under the unavoidable `/file/` prefix).
- Atomos 64 KiB runs use the default byte path; the sendfile A/B (same
  build, `ATOMOS_SF_MIN` env override) is in `bench-results/atomos-sendfile.txt`.
- wrk on the same 4 logical CPUs; results in `bench-results/nginx-*`,
  `seastar-*`, `atomos-*`.

## Caveats

- Loopback only; no NIC. Single-shot 5 s samples on a shared desktop —
  ±10% run-to-run noise (Seastar three runs: 19.9–21.0k cached,
  6.7–7.8k 64 KiB; best clean run reported).
- nginx default-static numbers are the stock config; the "fair"
  comparison for a tuned server is the `open_file_cache` row.
- Seastar row is the `httpd` demo app (io_uring backend, `--smp 4`,
  `connection_distribution`), not a tuned static server; it has no
  response cache, no sendfile, and its URLs carry the `/file/` prefix.
