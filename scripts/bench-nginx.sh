#!/usr/bin/env bash
# nginx baseline, ceteris paribus with the atomos/fds benches:
# same root files, same 127.0.0.1:8090, same wrk invocations, 4 workers
# pinned to the same logical CPUs, same kernel. Outputs bench-results/nginx-*.
# Usage: bash scripts/bench-nginx.sh
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PORT=8090
WORK=/tmp/nginx-bench

pkill -x nginx 2>/dev/null; sleep 0.3

rm -rf "$WORK" && mkdir -p "$WORK/root"
printf '<h1>hi</h1>' >"$WORK/root/index.html"
head -c 65536 /dev/urandom >"$WORK/root/file64k.bin"
# The script may run under sudo (nginx needs root): make the tree
# readable by the nginx worker user (nobody).
chmod -R a+rX "$WORK"

cat >"$WORK/nginx.conf" <<EOF
worker_processes 4;
worker_cpu_affinity 0001 0010 0100 1000;
error_log $WORK/error.log crit;
pid $WORK/nginx.pid;
events { worker_connections 8192; }
http {
    access_log off;
    sendfile on;
    tcp_nopush on;
    keepalive_timeout 65;
    # Tuned static path: cached file handles (the default per-request
    # open/stat costs ~8x on this kernel — measured).
    open_file_cache max=4096 inactive=60s;
    open_file_cache_valid 60s;
    open_file_cache_min_uses 1;
    server {
        listen 127.0.0.1:$PORT;
        root $WORK/root;
    }
}
EOF

nginx -c "$WORK/nginx.conf" 2>&1 || { echo "nginx start failed"; exit 1; }
sleep 0.5
for i in $(seq 1 20); do curl -s -o /dev/null "http://127.0.0.1:$PORT/" && break; sleep 0.2; done
echo "nginx UP (pid $(cat $WORK/nginx.pid))"

echo "== wrk: cached page (5s, 4x100) =="
wrk -t4 -c100 -d5s "http://127.0.0.1:$PORT/" 2>&1 | tee "$ROOT/bench-results/nginx-reqs-per-sec.txt"
echo "== wrk: 64KB file (5s, 4x100) =="
wrk -t4 -c100 -d5s "http://127.0.0.1:$PORT/file64k.bin" 2>&1 | tee "$ROOT/bench-results/nginx-reqs-per-sec-64kb.txt"

echo "== HTTP latency ladder (curl x200) =="
gawk 'BEGIN{for(i=0;i<200;i++){cmd="curl -s -o /dev/null -w %{time_total} http://127.0.0.1:'$PORT'/";cmd|getline v;close(cmd);a[i]=v*1e6}}END{n=asort(a,b);printf "samples %d — p50 %.1f p90 %.1f p99 %.1f p999 %.1f µs\n",n,b[int(n*.5)],b[int(n*.9)],b[int(n*.99)],b[int(n*.999)]}' | tee "$ROOT/bench-results/nginx-latency-http.txt"

echo "== TTFB (curl time_starttransfer x100) =="
gawk 'BEGIN{for(i=0;i<100;i++){cmd="curl -s -o /dev/null -w %{time_starttransfer} http://127.0.0.1:'$PORT'/";cmd|getline v;close(cmd);a[i]=v*1e6}}END{n=asort(a,b);printf "samples %d — p50 %.1f p90 %.1f p99 %.1f µs\n",n,b[int(n*.5)],b[int(n*.9)],b[int(n*.99)]}' | tee "$ROOT/bench-results/nginx-ttfb.txt"

echo "== mpstat during wrk =="
NGINX_PID=$(cat "$WORK/nginx.pid")
mpstat -P ALL 1 3 >"$WORK/mpstat.txt" 2>&1 &
MP=$!
wrk -t4 -c100 -d3s "http://127.0.0.1:$PORT/" >/dev/null 2>&1
wait $MP
grep -E "Average" "$WORK/mpstat.txt" | head -5 | tee "$ROOT/bench-results/nginx-mpstat-cpu-split.txt"

echo "== perf stat under wrk (all nginx pids) =="
NGINX_PIDS=$(pgrep -x nginx | tr '\n' ',')
perf stat -p "$NGINX_PIDS" -e cycles,instructions,branches,branch-misses,dTLB-loads,dTLB-load-misses,cache-misses,L1-dcache-load-misses,L1-icache-load-misses,page-faults,context-switches,cpu-migrations -- sleep 4 >"$ROOT/bench-results/nginx-perf-counters.txt" 2>&1 &
PF=$!
wrk -t4 -c100 -d4s "http://127.0.0.1:$PORT/" >/dev/null 2>&1
wait $PF
grep -E "insn per cycle|branch-misses|dTLB-load-misses|cpu-migrations|page-faults" "$ROOT/bench-results/nginx-perf-counters.txt" | head -8

pkill -x nginx 2>/dev/null
echo "nginx bench DONE"
