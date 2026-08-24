#!/usr/bin/env bash
# bench-atomos.sh — deep CPU/memory metrics for the ATOMOS HTTP server
# (the FDS-backed H1 engine), the counterpart to bench-full.sh's FDS
# phases. Produces bench-results/atomos-*.txt files.
#
# Usage: bash scripts/bench-atomos.sh [seconds]

set -u
SECS="${1:-4}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/bench-results"
mkdir -p "$OUT"
STAMP="$(date -u +%Y-%m-%dT%H:%MZ)"
CPU="$(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | xargs)"
KERN="$(uname -r)"
ATOMOS="$ROOT/../Atomos"
BIN="$ATOMOS/target/release/atomos"

FILE=""
w() { printf '%s\n' "$*" >> "$OUT/$FILE"; }
header() {
  FILE="$1"; : > "$OUT/$FILE"
  w "# $2 — $(date -u +%F\ %T)Z"
  w "# host: $(hostname) | cpu: $CPU | kernel: $KERN"
  w "# tool: $3 | params: $4 | duration: ${SECS}s"
}

pkill -x atomos 2>/dev/null
rm -f "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/atomos.sock"
(cd "$ATOMOS" && cargo build --release) >/dev/null 2>&1 || { echo "atomos build failed"; exit 1; }

start_atomos() { # $1 = port
  (cd "$ATOMOS" && exec "$BIN" --bind "127.0.0.1:$1" --root examples/first_app/static \
    --rules examples/first_app/rules.json) >/dev/null 2>&1 &
  echo $! > /tmp/ba.pid
  for _ in $(seq 1 50); do
    curl -s -o /dev/null "http://127.0.0.1:$1/index.html" && return 0
    sleep 0.1
  done
  return 1
}
stop_atomos() { kill "$(cat /tmp/ba.pid)" 2>/dev/null; rm -f "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/atomos.sock"; }

# --- perf counters under wrk load (incl. branch + dTLB miss rates) ----
start_atomos 18101 || echo "atomos failed to start" >&2
header "atomos-perf-counters.txt" "CPU counters of the atomos server under wrk load" "perf stat -p" "4x100 conns, HTTP keep-alive"
wrk -t4 -c100 -d"$SECS"s http://127.0.0.1:18101/index.html >/dev/null 2>&1 &
WRK=$!
sleep 0.7
perf stat -e cycles,instructions,branches,branch-misses,dTLB-loads,dTLB-load-misses,cache-misses,L1-dcache-load-misses,L1-icache-load-misses,page-faults,context-switches,cpu-migrations \
  -p "$(cat /tmp/ba.pid)" -o /tmp/ba-perf.txt sleep "$SECS" 2>/dev/null
wait "$WRK" 2>/dev/null
grep -E "cycles|instructions|#|branches|dTLB|cache-misses|L1-dcache|L1-icache|page-faults|context-switches|cpu-migrations" /tmp/ba-perf.txt >> "$OUT/$FILE" || w "perf -p failed (permissions)"
stop_atomos

# --- HTTP latency ladder (repeated fresh-connection requests) ---------
start_atomos 18102 || true
header "atomos-latency-http.txt" "HTTP request latency distribution (loopback)" "curl time_total x200" "fresh connection per request"
TMP=/tmp/ba-lat.txt
for _ in $(seq 1 200); do
  curl -s -o /dev/null -w "%{time_total}\n" http://127.0.0.1:18102/index.html
done > "$TMP"
awk '{v[NR]=$1*1e6} END{n=NR; for(i=1;i<=n;i++){s+=v[i]; s2+=v[i]*v[i]} asort(v);
  printf "samples %d — p10 %.1f p20 %.1f p30 %.1f p40 %.1f p50 %.1f p60 %.1f p70 %.1f p80 %.1f p90 %.1f p95 %.1f p99 %.1f p999 %.1f µs\n",
    n, v[int(0.10*n)+1], v[int(0.20*n)+1], v[int(0.30*n)+1], v[int(0.40*n)+1], v[int(0.50*n)+1],
    v[int(0.60*n)+1], v[int(0.70*n)+1], v[int(0.80*n)+1], v[int(0.90*n)+1], v[int(0.95*n)+1], v[int(0.99*n)+1], v[int(0.999*n)+1];
  m=s/n; printf "mean %.1f µs, stdev %.1f µs, jitter(stdev/mean) %.2f\n", m, sqrt(s2/n-m*m), sqrt(s2/n-m*m)/m }' "$TMP" >> "$OUT/$FILE"
stop_atomos

# --- TTFB ladder ------------------------------------------------------
start_atomos 18103 || true
header "atomos-ttfb.txt" "time-to-first-byte distribution (loopback)" "curl time_starttransfer x100" "fresh connection per request"
for _ in $(seq 1 100); do
  curl -s -o /dev/null -w "%{time_starttransfer}\n" http://127.0.0.1:18103/index.html
done | awk '{v[NR]=$1*1e6} END{n=NR; for(i=1;i<=n;i++){s+=v[i]; s2+=v[i]*v[i]} asort(v);
  printf "samples %d — p50 %.1f p90 %.1f p99 %.1f p999 %.1f µs, mean %.1f, stdev %.1f\n",
    n, v[int(n*0.5)+1], v[int(n*0.9)+1], v[int(n*0.99)+1], v[int(n*0.999)+1], s/n, sqrt(s2/n-(s/n)^2) }' >> "$OUT/$FILE"
stop_atomos

# --- ctx switches / migrations per sec during wrk ---------------------
start_atomos 18104 || true
header "atomos-ctx-switches-per-sec.txt" "context switches + cpu migrations /s (atomos server)" "perf stat -p / proc sampling" "during wrk load"
wrk -t4 -c100 -d"$SECS"s http://127.0.0.1:18104/index.html >/dev/null 2>&1 &
WRK=$!
sleep 0.7
perf stat -e context-switches,cpu-migrations -p "$(cat /tmp/ba.pid 2>/dev/null)" -o /tmp/ba-cs.txt sleep 2 2>/dev/null
wait "$WRK" 2>/dev/null
if grep -qE "context-switches|cpu-migrations" /tmp/ba-cs.txt 2>/dev/null; then
  grep -E "context-switches|cpu-migrations" /tmp/ba-cs.txt | sed 's/^ *//' >> "$OUT/$FILE"
  w "per 2 s window; divide by 2 for per-second. Migrations ~0 because"
  w "  workers self-pin (sched_setaffinity) — taskset is not needed."
else
  w "perf -p unavailable; see atomos-perf-counters.txt context-switches total"
fi
stop_atomos

# --- mpstat kernel vs user split under wrk ----------------------------
start_atomos 18105 || true
header "atomos-mpstat-cpu-split.txt" "kernel vs user CPU split (atomos under wrk)" "mpstat -P ALL" "4x100 conns"
wrk -t4 -c100 -d"$SECS"s http://127.0.0.1:18105/index.html >/dev/null 2>&1 &
WRK=$!
sleep 0.7
mpstat -P ALL 1 2 2>/dev/null | grep -E "Average" | head -8 >> "$OUT/$FILE" || w "mpstat unavailable"
wait "$WRK" 2>/dev/null
stop_atomos

# --- sar page faults under wrk ----------------------------------------
start_atomos 18106 || true
header "atomos-page-faults.txt" "page faults (system-wide, atomos under wrk)" "sar -B" "faults/s"
wrk -t4 -c100 -d"$SECS"s http://127.0.0.1:18106/index.html >/dev/null 2>&1 &
WRK=$!
sleep 0.7
sar -B 1 2 2>/dev/null | grep -E "Average" | head -2 >> "$OUT/$FILE" || w "sar -B unavailable"
wait "$WRK" 2>/dev/null
stop_atomos

# --- valgrind massif on the server ------------------------------------
header "atomos-massif-heap.txt" "heap profile of the atomos server" "valgrind --tool=massif" "peak heap (startup preallocation)"
(cd "$ATOMOS" && exec valgrind --tool=massif --massif-out-file=/tmp/ba-massif.out "$BIN" \
  --bind 127.0.0.1:18107 --root examples/first_app/static --rules examples/first_app/rules.json) >/dev/null 2>&1 &
VG=$!
sleep 3
for _ in $(seq 1 20); do curl -s -o /dev/null http://127.0.0.1:18107/index.html; done
kill "$VG" 2>/dev/null
wait "$VG" 2>/dev/null
if [[ -s /tmp/ba-massif.out ]]; then
  awk -F= '/mem_heap_B=/{last=$2} END{printf "peak heap: %d B\n", last}' /tmp/ba-massif.out >> "$OUT/$FILE"
  w "note: server preallocates at startup; per-request allocation would"
  w "  show as sustained growth in the massif curve."
else
  w "valgrind massif failed"
fi

echo "done — atomos metrics in $OUT/atomos-*.txt:"
ls -1 "$OUT" | grep '^atomos-' | sed 's/^/  /'
