# Same-box baselines: nginx, Seastar vs FDS/Atomos

Ceteris paribus on this exact machine (i5-5200U 2C/4T, kernel 7.2.0_1):
same root files (`index.html` = `<h1>hi</h1>`, `file64k.bin` = 64 KiB),
same `127.0.0.1:<port>`, same wrk (`-t4 -c100 -d5s`), 4 workers each
(nginx/Seastar `--smp 4`; Atomos `workers=4`), same load order.

## Head-to-head (wrk, 5 s)

| Server | cached page req/s | 64 KiB req/s | notes |
| --- | --- | --- | --- |
| **Atomos H1** (FDS epoll) | **85,652** | 24,430 | in-memory response cache |
| nginx `return 200` (mem) | 78,679 | — | nginx engine ceiling, no static path |
| nginx `open_file_cache` | 54,719 | **28,185** | tuned static path |
| nginx default static | 10,613 | 9,147 | per-request open/stat (8x penalty here) |
| Seastar httpd (io_uring demo) | 20,999 | 7,788 | demo app, no sendfile/response cache |

## What the comparison shows

- **Atomos ≈ nginx's engine ceiling**: 85.7k vs 78.7k req/s (+9%) on the
  cached page — both are in-memory responses; the difference is the
  transport/parser (FDS epoll vs nginx epoll) and the epoll edge-trigger
  busy-poll loop.
- **nginx default static is 8x slower than Atomos cache-hit** because of
  per-request `open`/`stat` (measured: `open_file_cache` alone recovers
  5.2x). Atomos's response cache is the equivalent optimization.
- **On 64 KiB, nginx tuned (28.2k) beats Atomos (24.4k)** — sendfile's
  kernel zero-copy wins for larger bodies over Atomos's read+write.
- Latency: Atomos cached p50 ~1.31 ms under 100-conn load (wrk avg);
  nginx tuned p50 351 µs / TTFB 319 µs; Seastar p50 585 µs (curl,
  lighter load).
- **Seastar trails the field 4.1x on cached pages** (21.0k vs Atomos
  85.7k / nginx tuned 55.9k). It is the only one doing per-request
  cooperative scheduling + io_uring file reads with no response cache
  and no sendfile; its demo httpd is a framework sample, not a tuned
  static server — the point of the row is that Seastar-the-framework
  does not beat plain epoll here out of the box.
- All three servers are kernel-dominated on this box (mpstat %sys+%soft
  > %usr; nginx IPC 0.50, Seastar IPC 0.59, Atomos IPC 0.54; 0
  cpu-migrations for all three).

## Method

- `scripts/bench-nginx.sh` (sudo; nginx needs root for runtime paths).
- `scripts/bench-seastar.sh <app_httpd>`; for a byte-fair short URL the
  bench files are symlinked at `/` (`/index.html`, `/file64k.bin`) and
  the script is run with `PAGE=/file/index.html BIG=/file/file64k.bin`.
- The root dir is `/tmp/nginx-bench/root` (same content for all three;
  Seastar's demo serves under the unavoidable `/file/` prefix).
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
