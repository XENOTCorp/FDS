#!/usr/bin/env bash
# Latency-distribution ladder, identical methodology for every server:
# curl x200, fresh connection per request, cached page. Reports p10/p50/
# p90/p99/p999/max/mean/stdev, then the under-load wrk avg/stdev/max.
# Ceteris paribus with the same-box benches (bench-results/nginx-* etc).
# Usage: bash scripts/bench-latency.sh
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK=/tmp/nginx-bench
STAMP="$(date -u +%Y-%m-%dT%H:%MZ)"

ladder() {
  local URL="$1"
  gawk -v url="$URL" 'BEGIN{
    for(i=0;i<200;i++){cmd="curl -s -o /dev/null -w %{time_total} " url; cmd|getline v; close(cmd); a[i]=v*1e6}
  } END{
    n=asort(a,b); s=0; for(i=1;i<=n;i++) s+=b[i]; m=s/n; sd=0;
    for(i=1;i<=n;i++) sd+=(b[i]-m)^2;
    printf "p10 %.1f p50 %.1f p90 %.1f p99 %.1f p999 %.1f max %.1f mean %.1f stdev %.1f (n=%d)\n",
      b[int(n*.10)], b[int(n*.50)], b[int(n*.90)], b[int(n*.99)], b[int(n*.999)], b[n], m, sqrt(sd/(n-1)), n
  }'
}

wrkrow() { wrk -t4 -c100 -d5s "$1" 2>&1 | grep -E "Latency|Requests/sec" | sed 's/^/  /'; }

pkill -x nginx atomos h2o caddy httpd 2>/dev/null; sleep 0.4

# --- nginx (tuned: open_file_cache) ---
printf '%s\n' 'alexosage' | sudo -S bash -c "
rm -rf $WORK && mkdir -p $WORK/root
printf '<h1>hi</h1>' > $WORK/root/index.html
head -c 65536 /dev/urandom > $WORK/root/file64k.bin
chmod -R a+rX $WORK
cat > $WORK/nginx.conf <<'NEOF'
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
    open_file_cache max=4096 inactive=60s;
    open_file_cache_valid 60s;
    open_file_cache_min_uses 1;
    server { listen 127.0.0.1:8090; root $WORK/root; }
}
NEOF
nginx -c $WORK/nginx.conf
" 2>/dev/null
sleep 0.6
echo "== nginx tuned =="
echo -n "no-load: "; ladder "http://127.0.0.1:8090/"
wrkrow "http://127.0.0.1:8090/"

# --- h2o ---
pkill -x nginx 2>/dev/null; sleep 0.3
(h2o -c "$ROOT/scripts/h2o-bench.conf" >/tmp/h2o-lad.log 2>&1 &)
sleep 1
echo "== h2o =="
echo -n "no-load: "; ladder "http://127.0.0.1:8095/"
wrkrow "http://127.0.0.1:8095/"

# --- caddy ---
pkill -x h2o 2>/dev/null; sleep 0.3
(caddy file-server --root $WORK/root --listen 127.0.0.1:8096 --access-log off >/tmp/caddy-lad.log 2>&1 &)
sleep 1
echo "== caddy =="
echo -n "no-load: "; ladder "http://127.0.0.1:8096/"
wrkrow "http://127.0.0.1:8096/"

# --- seastar httpd ---
pkill -x caddy 2>/dev/null; sleep 0.3
(/home/xenot/seastar/build/apps/httpd/httpd --smp 4 --port 8093 --load-balancing-algorithm connection_distribution --prometheus_port 0 >/tmp/seastar-lad.log 2>&1 &)
for _ in $(seq 1 40); do curl -s -o /dev/null "http://127.0.0.1:8093/file/index.html" 2>/dev/null && break; sleep 0.25; done
echo "== seastar httpd =="
echo -n "no-load: "; ladder "http://127.0.0.1:8093/file/index.html"
wrkrow "http://127.0.0.1:8093/file/index.html"

# --- atomos (default build) ---
pkill -x httpd 2>/dev/null; sleep 0.3
(cd /home/xenot/Projects/Atomos && exec ./target/release/atomos --bind 127.0.0.1:8091 --root $WORK/root --rules examples/first_app/rules.json >/tmp/atomos-lad.log 2>&1 &)
sleep 1.2
echo "== atomos H1 =="
echo -n "no-load: "; ladder "http://127.0.0.1:8091/"
wrkrow "http://127.0.0.1:8091/"

pkill -x atomos 2>/dev/null
echo "ladders done — $STAMP"
