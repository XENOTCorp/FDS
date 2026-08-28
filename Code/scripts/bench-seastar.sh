#!/usr/bin/env bash
# Seastar httpd baseline, ceteris paribus with the atomos/fds benches:
# same root files, same wrk invocations, --smp 4. The seastar demo
# serves files under /file/<path> (directory_handler from "/"), so the
# URLs carry the /file/ prefix; same content, unavoidable prefix (noted).
# For a short path, symlink the bench files at / and set PAGE/BIG:
#   sudo ln -sf /tmp/nginx-bench/root/index.html /index.html
#   sudo ln -sf /tmp/nginx-bench/root/file64k.bin /file64k.bin
#   PAGE=/file/index.html BIG=/file/file64k.bin bash scripts/bench-seastar.sh ...
# Usage: bash scripts/bench-seastar.sh /path/to/httpd
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${1:?usage: bench-seastar.sh /path/to/httpd}"
PORT=8093
PAGE="${PAGE:-/file/tmp/nginx-bench/root/index.html}"
BIG="${BIG:-/file/tmp/nginx-bench/root/file64k.bin}"

pkill -x httpd 2>/dev/null; sleep 0.3

"$BIN" --smp 4 --port "$PORT" --load-balancing-algorithm connection_distribution --prometheus_port 0 >/tmp/seastar-httpd.log 2>&1 &
SP=$!
for i in $(seq 1 40); do
    curl -s -o /dev/null "http://127.0.0.1:$PORT$PAGE" 2>/dev/null && break
    sleep 0.5
done
curl -s -o /dev/null -w "up check: %{http_code}\n" "http://127.0.0.1:$PORT$PAGE"

echo "== wrk: cached page (5s, 4x100) =="
wrk -t4 -c100 -d5s "http://127.0.0.1:$PORT$PAGE" 2>&1 | tee "$ROOT/bench-results/seastar-reqs-per-sec.txt"
echo "== wrk: 64KB file (5s, 4x100) =="
wrk -t4 -c100 -d5s "http://127.0.0.1:$PORT$BIG" 2>&1 | tee "$ROOT/bench-results/seastar-reqs-per-sec-64kb.txt"

echo "== HTTP latency ladder (curl x200) =="
gawk 'BEGIN{for(i=0;i<200;i++){cmd="curl -s -o /dev/null -w %{time_total} http://127.0.0.1:'$PORT$PAGE'";cmd|getline v;close(cmd);a[i]=v*1e6}}END{n=asort(a,b);printf "samples %d; p50 %.1f p90 %.1f p99 %.1f p999 %.1f µs\n",n,b[int(n*.5)],b[int(n*.9)],b[int(n*.99)],b[int(n*.999)]}' | tee "$ROOT/bench-results/seastar-latency-http.txt"

echo "== mpstat during wrk =="
mpstat -P ALL 1 3 >/tmp/mpstat-s.txt 2>&1 &
MP=$!
wrk -t4 -c100 -d3s "http://127.0.0.1:$PORT$PAGE" >/dev/null 2>&1
wait $MP
grep -E "Average" /tmp/mpstat-s.txt | head -3 | tee "$ROOT/bench-results/seastar-mpstat-cpu-split.txt"

echo "== perf stat under wrk =="
perf stat -p "$SP" -e cycles,instructions,branches,branch-misses,dTLB-loads,dTLB-load-misses,cache-misses,L1-dcache-load-misses,L1-icache-load-misses,page-faults,context-switches,cpu-migrations -- sleep 4 >"$ROOT/bench-results/seastar-perf-counters.txt" 2>&1 &
PF=$!
wrk -t4 -c100 -d4s "http://127.0.0.1:$PORT$PAGE" >/dev/null 2>&1
wait $PF
grep -E "insn per cycle|branch-misses #|dTLB-load-misses #|cpu-migrations|page-faults" "$ROOT/bench-results/seastar-perf-counters.txt" | head -6

kill $SP 2>/dev/null
echo "seastar bench DONE"
