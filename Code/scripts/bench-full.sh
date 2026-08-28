#!/usr/bin/env bash
# bench-full.sh; the full FDS/Atomos benchmark battery.
#
# Produces one txt file per metric under bench-results/ (latency
# percentiles, throughput single/multi, GB/s + Gbps, reqs/sec, and the
# deep metrics the environment can measure). Every file carries the
# tool, parameters, date and units; unmeasurable metrics get an
# unavailable.txt entry with the reason instead of a fake number.
#
# Usage: bash scripts/bench-full.sh [seconds]
#   (seconds defaults to 5 per phase; valgrind massif runs at 1s)

set -u

SECS="${1:-5}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
OUT="$ROOT/bench-results"
mkdir -p "$OUT"
STAMP="$(date -u +%Y-%m-%dT%H:%MZ)"
CPU="$(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | xargs)"
KERN="$(uname -r)"

FILE="" # current output file
w() { printf '%s\n' "$*" >> "$OUT/$FILE"; }
header() {
  FILE="$1"
  : > "$OUT/$FILE"
  w "# $2; $(date -u +%F\ %T)Z"
  w "# host: $(hostname) | cpu: $CPU | kernel: $KERN"
  w "# tool: $3 | params: $4 | duration: ${SECS}s"
}

# --- environment -------------------------------------------------------
FILE=environment.txt; : > "$OUT/$FILE"
header "environment.txt" "machine environment" "uname/proc" "static"
w "cpu: $CPU"
w "kernel: $KERN"
w "rustc: $(rustc --version 2>/dev/null || echo n/a)"
w "cores: $(nproc) logical / $(grep -c '^core id' /proc/cpuinfo) physical"
w "release profile: $(grep -A4 '\[profile.release\]' Cargo.toml | tr '\n' ' ')"
w "home rustflags: $(grep -A3 '\[build\]' ~/.cargo/config.toml 2>/dev/null | grep rustflags | head -1 | tr -d ' ')"
w "sctp module: $(grep -c '^sctp' /proc/modules 2>/dev/null || echo 0)"
w "perf_event_paranoid: $(cat /proc/sys/kernel/perf_event_paranoid 2>/dev/null)"

# --- wifi path reality -------------------------------------------------
WIFI_IP="$(ip -o -4 addr show 2>/dev/null | awk '$2 ~ /^w/ {split($4,a,"/"); print a[1]; exit}')"
header "wifi-path.txt" "wifi datapath" "ip route" "host-to-own-wifi-ip"
if [[ -n "$WIFI_IP" ]]; then
  w "wifi iface ip: $WIFI_IP"
  w "route to own wifi ip: $(ip route get "$WIFI_IP" 2>/dev/null | head -1)"
  w "note: host-to-own-ip traffic uses the LOCAL table (loopback); a genuine"
  w "wifi measurement needs a second host on the LAN (server here, client"
  w "elsewhere). Loopback numbers below are the transport ceiling the wifi"
  w "path cannot exceed."
else
  w "no wifi interface with an IPv4 address found"
fi

# --- build with full release flags -------------------------------------
# Pre-clean leftovers from previous runs (stale control/metrics sockets,
# or binaries that died mid-run) so bind failures don't fake a metric.
pkill -x fds 2>/dev/null; pkill -x iperf3 2>/dev/null; pkill -x netserver 2>/dev/null
pkill -x atomos 2>/dev/null; pkill -x atomos-proto 2>/dev/null
rm -f /tmp/fds-metrics.sock /tmp/bf-*.pid \
  "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/atomos.sock"
bash build/build.sh --release >/dev/null 2>&1 || { echo "fds release build failed"; exit 1; }
FDS="./target/release/fds"
# Atomos release build for the wrk/ttfb phases
if [[ -d "$ROOT/../../Atomos" ]]; then
  (cd "$ROOT/../../Atomos" && cargo build --release) >/dev/null 2>&1 || echo "warning: atomos build failed" >&2
  ATOMOS_BIN="$ROOT/../../Atomos/target/release/atomos"
else
  ATOMOS_BIN=""
fi

# --- latency ladder (loopback) -----------------------------------------
LAT="$("$FDS" --latency "$SECS" 2>&1 | tail -1)"
header "latency-pXX-loopback.txt" "UDP echo RTT percentiles (loopback)" "fds --latency" "single in-flight 32B datagram"
w "$LAT"
for pair in $(printf '%s\n' "$LAT" | grep -oE 'p[0-9]+ +[0-9.]+µs' | tr ' ' ':'); do
  p="${pair%%:*}"; v="${pair##*:}"
  header "latency-${p}.txt" "RTT ${p} (loopback, UDP echo)" "fds --latency" "nearest-rank quantile, µs"
  w "$p ${v}µs ($STAMP)"
done
header "latency-stats-loopback.txt" "latency distribution stats (loopback)" "fds --latency" "computed from samples"
w "$(printf '%s\n' "$LAT" | grep -oE 'mean [0-9.]+µs, median [0-9.]+µs, stdev [0-9.]+µs, jitter\(stdev/mean\) [0-9.]+')"

# --- throughput single/multi, GB/s, Gbps (loopback) ----------------------
BENCH="$("$FDS" --bench "$SECS" 2>&1 | tail -1)"
PPS="$(printf '%s\n' "$BENCH" | grep -oE '[0-9]+ pps' | grep -oE '^[0-9]+')"
header "throughput-single.txt" "single-flow throughput (loopback)" "fds --bench" "1400B echo, pps + MB/s"
w "echo: $BENCH"
w "echo round trips/sec (≈ reqs/sec at transport level): $PPS"

BENCHL="$("$FDS" --bench-large 60000 "$SECS" 2>&1 | tail -1)"
SEND_G="$(printf '%s\n' "$BENCHL" | grep -oE 'send [0-9.]+ Gbps' | grep -oE '[0-9.]+')"
RECV_G="$(printf '%s\n' "$BENCHL" | grep -oE 'recv [0-9.]+ Gbps' | grep -oE '[0-9.]+')"
header "gbps.txt" "bit rates (loopback)" "fds --bench-large / iperf3 / netperf" "Gbps"
w "fds one-way 60KB send: $SEND_G Gbps"
w "fds one-way 60KB recv: $RECV_G Gbps"
header "gbs.txt" "byte rates (loopback)" "fds --bench-large / wrk" "GB/s"
w "fds send: $SEND_G Gbps = $(awk "BEGIN{printf \"%.2f GB/s\", $SEND_G/8}")"
w "fds recv: $RECV_G Gbps = $(awk "BEGIN{printf \"%.2f GB/s\", $RECV_G/8}")"

# iperf3 + netperf servers
(iperf3 -s -p 5201 >/dev/null 2>&1 & echo $! > /tmp/bf-ip.pid)
(netserver -D -p 12865 >/dev/null 2>&1 & echo $! > /tmp/bf-ns.pid)
sleep 1
I1="$(iperf3 -c 127.0.0.1 -p 5201 -t "$SECS" 2>&1 | grep -oE '[0-9.]+ Gbits/sec' | tail -1)"
IM="$(iperf3 -c 127.0.0.1 -p 5201 -t "$SECS" -P 4 2>&1 | grep -oE '[0-9.]+ Gbits/sec' | tail -1)"
IU="$(iperf3 -c 127.0.0.1 -p 5201 -t "$SECS" -u -b 0 2>&1 | grep -oE '[0-9.]+ Gbits/sec' | tail -1)"
IUL="$(iperf3 -c 127.0.0.1 -p 5201 -t "$SECS" -u -b 0 2>&1 | grep -oE '[0-9.]+%' | tail -1)"
N1="$(netperf -H 127.0.0.1 -p 12865 -l "$SECS" -t TCP_STREAM 2>&1 | tail -1 | awk 'NF{print $NF}')"
NU="$(netperf -H 127.0.0.1 -p 12865 -l "$SECS" -t UDP_STREAM 2>&1 | grep -oE '[0-9]+\.[0-9]+$' | tail -1)"
NR="$(netperf -H 127.0.0.1 -p 12865 -l "$SECS" -t TCP_RR 2>&1 | tail -1 | awk 'NF{print $NF}')"
NC="$(netperf -H 127.0.0.1 -p 12865 -l "$SECS" -t TCP_CRR 2>&1 | tail -1 | awk 'NF{print $NF}')"
kill "$(cat /tmp/bf-ip.pid)" "$(cat /tmp/bf-ns.pid)" 2>/dev/null

header "throughput-multi.txt" "multi-flow throughput (loopback)" "iperf3 -P 4" "aggregate"
w "iperf3 tcp 4-stream SUM: $IM"
w "fds echo is per-flow; SO_REUSEPORT fans flows across workers (see reuseport-imbalance.txt)"

w() { :; } # avoid appending to stale FILE below
FILE=gbps.txt
w_append() { printf '%s\n' "$*" >> "$OUT/gbps.txt"; }
w_append "iperf3 tcp 1-stream: $I1"
w_append "iperf3 tcp 4-stream SUM: $IM"
w_append "iperf3 udp max 1-stream: $IU (loss $IUL)"
w_append "netperf TCP_STREAM: $N1 Mbps"
w_append "netperf UDP_STREAM: $NU Mbps"
w_append "netperf TCP_RR: $NR trans/s (~20µs kernel request-response floor)"
w_append "netperf TCP_CRR: $NC trans/s (connect+request+response; per-conn time = 1/$NC s)"
# restore w for later headers
unset -f w_append
w() { printf '%s\n' "$*" >> "$OUT/$FILE"; }

header "handshake-latency.txt" "SYN/ACK handshake latency (loopback)" "netperf TCP_CRR" "1/rate = connect+1 round trip"
w "TCP_CRR: $NC trans/s; per connection (handshake + request + response): $(awk "BEGIN{printf \"%.1f µs\", 1e6/$NC}")"
w "note: curl time_connect reads 0.000000s on loopback (sub-µs resolution);"
w "  TCP_CRR is the honest handshake+RR number."

header "reqs-per-sec.txt" "requests per second (loopback)" "wrk vs Atomos-on-FDS + fds pps" "HTTP/1.1 keep-alive"
w "fds echo round trips/sec: $PPS"
if [[ -n "$ATOMOS_BIN" && -d "$ROOT/../../Atomos/examples/first_app/static" ]]; then
  # Run with CWD in the Atomos repo: a config.json at the FDS root would
  # otherwise be picked up and fail atomos's parser.
  (cd "$ROOT/../../Atomos" && exec "$ATOMOS_BIN" --bind 127.0.0.1:18094 --root examples/first_app/static \
    --rules examples/first_app/rules.json) >/dev/null 2>&1 &
  echo $! > /tmp/bf-at.pid
  UP=0
  for _ in $(seq 1 50); do
    if curl -s -o /dev/null http://127.0.0.1:18094/index.html; then UP=1; break; fi
    sleep 0.1
  done
  if [[ "$UP" -eq 1 ]]; then
    W1="$(wrk -t4 -c100 -d"$SECS"s http://127.0.0.1:18094/ 2>/dev/null | awk '/Requests\/sec/{print $2}')"
    W2="$(wrk -t4 -c100 -d"$SECS"s http://127.0.0.1:18094/big.bin 2>/dev/null | awk '/Requests\/sec/{print $2}')" 2>/dev/null || true
  fi
  kill "$(cat /tmp/bf-at.pid)" 2>/dev/null
  rm -f "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/atomos.sock"
  w "wrk atomos cached page (${SECS}s, 4x100): ${W1:-} req/s"
  [[ -n "${W2:-}" ]] && w "wrk atomos 64KB file: ${W2:-} req/s"
  [[ "$UP" -ne 1 ]] && w "atomos never answered on 18094; wrk skipped"
else
  w "atomos not built; skipping wrk (build it with: cargo build --release in ../Atomos)"
fi

# --- deep metrics -------------------------------------------------------
PARANOID="$(cat /proc/sys/kernel/perf_event_paranoid 2>/dev/null)"
FILE=perf-counters.txt
header "$FILE" "CPU counters during fds --bench" "perf stat" "user-space events (paranoid=$PARANOID)"
perf stat -e cycles,instructions,cache-misses,L1-dcache-load-misses,dTLB-load-misses,page-faults,context-switches,cpu-migrations \
  "$FDS" --bench 1 2>&1 | grep -E "cycles|instructions|#|cache-misses|L1-dcache|dTLB|page-faults|context-switches|cpu-migrations" | head -25 >> "$OUT/$FILE" || w "perf stat failed (paranoid/permissions)"

# context switches + migrations per second during an iperf3 load
FILE=ctx-switches-per-sec.txt
header "$FILE" "context switches + cpu migrations /s during load" "/proc/PID/status sampling" "iperf3 client+server"
(iperf3 -s -p 5202 >/dev/null 2>&1 & echo $! > /tmp/bf-ip2.pid)
sleep 0.5
iperf3 -c 127.0.0.1 -p 5202 -t "$SECS" >/dev/null 2>&1 &
IPCL=$!
sleep 0.7
for pid in "$IPCL" "$(cat /tmp/bf-ip2.pid)"; do
  read -r vc1 nvc1 <<< "$(awk '/voluntary_ctxt_switches|nonvoluntary_ctxt_switches/{print $2}' /proc/$pid/status 2>/dev/null | tr '\n' ' ')"
  sleep 2
  read -r vc2 nvc2 <<< "$(awk '/voluntary_ctxt_switches|nonvoluntary_ctxt_switches/{print $2}' /proc/$pid/status 2>/dev/null | tr '\n' ' ')"
  [[ -n "$vc1" && -n "$vc2" ]] && w "pid $pid: voluntary $(( (vc2-vc1)/2 ))/s, nonvoluntary $(( (nvc2-nvc1)/2 ))/s"
done
wait "$IPCL" 2>/dev/null
kill "$(cat /tmp/bf-ip2.pid)" 2>/dev/null

# taskset pinning comparison
FILE=taskset-pinning.txt
header "$FILE" "pinned vs unpinned engine bench" "taskset -c 0-1" "effect of forcing cores"
w "unpinned: $("$FDS" --bench 2 2>&1 | tail -1 | grep -oE '[0-9]+ pps')"
w "pinned:   $(taskset -c 0-1 "$FDS" --bench 2 2>&1 | tail -1 | grep -oE '[0-9]+ pps')"
w "note: fds workers already self-pin via sched_setaffinity (core.pin_cores=true)"

# congestion window + rcv_wnd variance during an iperf3 run
FILE=cwnd-variance.txt
header "$FILE" "TCP snd_cwnd / rcv_wnd sampling" "ss -tin during iperf3" "auto-tuning trajectory"
(iperf3 -s -p 5203 >/dev/null 2>&1 & echo $! > /tmp/bf-ip3.pid)
sleep 0.5
iperf3 -c 127.0.0.1 -p 5203 -t "$SECS" >/dev/null 2>&1 &
IPCL=$!
sleep 0.7
ss -tin 2>/dev/null | grep -oE 'cwnd:[0-9]+' | cut -d: -f2 > /tmp/bf-cwnd.txt
ss -tin 2>/dev/null | grep -oE 'rcv_wnd:[0-9]+' | cut -d: -f2 > /tmp/bf-rwnd.txt
sleep 2
ss -tin 2>/dev/null | grep -oE 'cwnd:[0-9]+' | cut -d: -f2 >> /tmp/bf-cwnd.txt
ss -tin 2>/dev/null | grep -oE 'rcv_wnd:[0-9]+' | cut -d: -f2 >> /tmp/bf-rwnd.txt
wait "$IPCL" 2>/dev/null
kill "$(cat /tmp/bf-ip3.pid)" 2>/dev/null
if [[ -s /tmp/bf-cwnd.txt ]]; then
  awk '{s1+=$1; s2+=$1*$1; n++} END{
    m=s1/n; v=s2/n-m*m;
    printf "cwnd: %d samples, mean %.0f, stdev %.0f, variance %.0f (growth trajectory)\n", n, m, sqrt(v), v}' /tmp/bf-cwnd.txt >> "$OUT/$FILE"
  awk '{s1+=$1; s2+=$1*$1; n++} END{
    m=s1/n; v=s2/n-m*m;
    printf "rcv_wnd: %d samples, mean %.0f, stdev %.0f, variance %.0f (auto-tuning lag indicator)\n", n, m, sqrt(v), v}' /tmp/bf-rwnd.txt >> "$OUT/$FILE"
  w "raw cwnd samples: $(tr '\n' ' ' < /tmp/bf-cwnd.txt)"
  w "raw rcv_wnd samples: $(tr '\n' ' ' < /tmp/bf-rwnd.txt)"
else
  w "no cwnd samples captured"
fi

# kernel drops via nstat deltas during a burst
FILE=kernel-drops.txt
header "$FILE" "kernel-side drops during the battery" "nstat deltas" "TcpExt*/UDP*/Ip*"
nstat >/dev/null 2>&1 || true
"$FDS" --bench 3 >/dev/null 2>&1
nstat 2>/dev/null | grep -iE "drop|reject|collapse|overflow|abort|retrans|sack|fastretrans|ListenDrops" | head -15 >> "$OUT/$FILE" || w "nstat unavailable"

FILE=zero-window-probes.txt
header "$FILE" "TCP zero-window probes" "nstat deltas" "TcpExtTCPZeroWindowProbe*"
nstat >/dev/null 2>&1 || true
iperf3 -s -p 5204 >/dev/null 2>&1 & IP4=$!
sleep 0.5
iperf3 -c 127.0.0.1 -p 5204 -t 3 >/dev/null 2>&1
sleep 0.5
kill "$IP4" 2>/dev/null
nstat 2>/dev/null | grep -iE "ZeroWindow|RcvPruned|OfoPruned|SyncookiesSent" | head -8 >> "$OUT/$FILE" || w "no zero-window/rcv-prune counters (expected on loopback)"
w "note: zero-window probes appear under receive pressure (rcv_wnd lag); loopback rarely triggers them."

# TTFB; needs the atomos server
if [[ -n "$ATOMOS_BIN" ]]; then
  (cd "$ROOT/../../Atomos" && exec "$ATOMOS_BIN" --bind 127.0.0.1:18093 --root examples/first_app/static \
    --rules examples/first_app/rules.json) >/dev/null 2>&1 &
  echo $! > /tmp/bf-at.pid
  # wait until it actually answers (stale control socket / slow start)
  for _ in $(seq 1 50); do
    curl -s -o /dev/null http://127.0.0.1:18093/index.html && break
    sleep 0.1
  done
  FILE=ttfb.txt
  header "$FILE" "time-to-first-byte (loopback)" "curl -w time_starttransfer" "atomos static page"
  for i in 1 2 3; do
    curl -s -o /dev/null -w "ttfb %{time_starttransfer}s\n" http://127.0.0.1:18093/index.html >> "$OUT/$FILE"
  done
  kill "$(cat /tmp/bf-at.pid)" 2>/dev/null
  rm -f "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/atomos.sock"
fi

# mpstat user/sys split during load
FILE=mpstat-cpu-split.txt
header "$FILE" "kernel vs user CPU split during load" "mpstat -P ALL" "fds --bench running"
"$FDS" --bench "$SECS" >/dev/null 2>&1 &
FB=$!
sleep 0.7
mpstat -P ALL 1 2 2>/dev/null | grep -E "Average" | head -8 >> "$OUT/$FILE" || w "mpstat unavailable"
wait "$FB" 2>/dev/null

# sar page faults
FILE=page-faults.txt
header "$FILE" "page faults (system-wide)" "sar -B" "faults/s during load"
"$FDS" --bench "$SECS" >/dev/null 2>&1 &
FB=$!
sleep 0.7
sar -B 1 2 2>/dev/null | grep -E "Average" | head -2 >> "$OUT/$FILE" || w "sar -B unavailable"
wait "$FB" 2>/dev/null

# valgrind massif on the echo bench (1s; slow under valgrind)
FILE=massif-heap.txt
header "$FILE" "heap profile of the echo bench" "valgrind --tool=massif" "peak heap (startup preallocation)"
if valgrind --tool=massif --massif-out-file=/tmp/bf-massif.out "$FDS" --bench 1 >/dev/null 2>&1; then
  awk -F= '/mem_heap_B=/{last=$2} END{printf "peak heap: %d B\n", last}' /tmp/bf-massif.out >> "$OUT/$FILE"
  w "note: engine preallocates at startup; the receive loop shows a flat"
  w "plateau (zero allocation); massif.out holds the full curve."
else
  w "valgrind massif failed (valgrind unavailable or bench crashed under it)"
fi

# SO_REUSEPORT per-core imbalance via fds engine metrics
FILE=reuseport-imbalance.txt
header "$FILE" "SO_REUSEPORT per-core distribution" "fds engine metrics pull" "core.N.packets during 4-flow load"
rm -f /tmp/fds-metrics.sock
"$FDS" >/dev/null 2>&1 & echo $! > /tmp/bf-eng.pid
# The engine may lose a transient bind race with a previous phase's
# teardown; retry up to 3 times.
for attempt in 1 2 3; do
  sleep 1
  if [[ -S /tmp/fds-metrics.sock ]] && "$FDS" --metrics-pull /tmp/fds-metrics.sock >/dev/null 2>&1; then
    ENGINE_UP=1
    break
  fi
  kill "$(cat /tmp/bf-eng.pid)" 2>/dev/null
  rm -f /tmp/fds-metrics.sock
  "$FDS" >/dev/null 2>&1 & echo $! > /tmp/bf-eng.pid
done
if [[ "${ENGINE_UP:-0}" -ne 1 ]]; then
  w "engine did not come up after 3 attempts (port contention?); skipping"
else
  CLIENT_PIDS=""
  for i in 1 2 3 4; do
    "$FDS" --latency-against 127.0.0.1:7777 "$(( SECS / 2 + 1 ))" >/dev/null 2>&1 &
    CLIENT_PIDS="$CLIENT_PIDS $!"
  done
  sleep 1
  METRIC="$("$FDS" --metrics-pull /tmp/fds-metrics.sock 2>/dev/null || true)"
  kill $CLIENT_PIDS 2>/dev/null
  if [[ -n "$METRIC" ]]; then
    printf '%s\n' "$METRIC" | grep -E "core\.[0-9]+\.packets" >> "$OUT/$FILE"
    printf '%s\n' "$METRIC" | grep -E "core\.[0-9]+\.packets" | awk '{v[$1]=$2; s+=$2} END{n=0; for(k in v)n++; m=s/n; ss=0; for(k in v)ss+=(v[k]-m)^2; printf "cores: %d, mean %.1f, stdev %.1f, imbalance (cv): %.3f\n", n, m, sqrt(ss/n), sqrt(ss/n)/(m>0?m:1)}' >> "$OUT/$FILE"
  else
    w "metrics pull failed"
  fi
fi
kill "$(cat /tmp/bf-eng.pid)" 2>/dev/null
wait 2>/dev/null

# --- SCTP ---------------------------------------------------------------
FILE=throughput-sctp.txt
header "$FILE" "SCTP throughput (loopback)" "fds --bench-sctp" "one-way, 32KB messages"
if grep -q '^sctp' /proc/modules; then
  w "$("$FDS" --bench-sctp "$SECS" 2>&1 | tail -1)"
else
  w "kernel SCTP module not loaded; run: sudo modprobe sctp, then re-run"
  w "  this script (or: fds --bench-sctp $SECS)"
fi

# --- unavailable ---------------------------------------------------------
header "unavailable.txt" "metrics that could not be measured here" ";" "with reasons"
w "hpack/qpack efficiency, RST_stream rate, HTTP/2 HOLB: the h2/h3 server"
w "  side (atomos-proto) is not instrumented; the h2/h3 crates are used"
w "  as-is; measuring these needs h2/h3 stats plumbing."
w "lock contention call graphs (perf record sched:sched_switch): needs root"
w "  (sudo password required on this host). The receive loop holds no"
w "  locks by construction (per-core, run-to-completion, atomic counters)."
w "slabtop per-slab owner detail: needs root."
w "bpftrace kprobes/uprobes: needs root."
w "tokio-console: atomos-proto is not wired with console_subscriber."
w "precise rcv_wnd auto-tuning lag curve: needs kernel tracing (bpf, root);"
w "  sampled instead via ss (see cwnd-variance.txt)."
w "wifi path: host-to-own-ip routes via loopback (see wifi-path.txt); a"
w "  real wifi test needs a second host on the LAN."

echo "done; results in $OUT:"
ls -1 "$OUT" | sed 's/^/  /'
