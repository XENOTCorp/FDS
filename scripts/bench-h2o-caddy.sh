#!/usr/bin/env bash
# h2o + caddy baselines, ceteris paribus with the nginx/seastar/atomos
# benches: same root files, same wrk invocations, 4 workers each
# (h2o num-threads 4; caddy default GOMAXPROCS=4 on this box).
#
# Usage: bash scripts/bench-h2o-caddy.sh
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/bench-results"
WORK=/tmp/nginx-bench/root
STAMP="$(date -u +%Y-%m-%dT%H:%MZ)"
CPU="$(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | xargs)"
KERN="$(uname -r)"

run_one() { # $1=name $2=port $3=up_url $4=extra
  local NAME="$1" PORT="$2" UP="$3"
  echo "== $NAME =="
  local BIN
  case "$NAME" in
    h2o) BIN="/usr/bin/h2o -c $ROOT/scripts/h2o-bench.conf" ;;
    caddy) BIN="caddy file-server --root $WORK --listen 127.0.0.1:$PORT --access-log off" ;;
  esac
  # shellcheck disable=SC2086
  $BIN >/tmp/$NAME-bench.log 2>&1 &
  local SP=$!
  local UPOK=0
  for _ in $(seq 1 40); do
    if curl -s -o /dev/null "http://127.0.0.1:$PORT$UP" 2>/dev/null; then UPOK=1; break; fi
    sleep 0.25
  done
  [[ "$UPOK" -eq 1 ]] || { echo "$NAME never answered"; kill $SP 2>/dev/null; return 1; }
  curl -s -o /dev/null -w "up check: %{http_code}\n" "http://127.0.0.1:$PORT$UP"

  echo "== wrk: cached page (5s, 4x100) =="
  wrk -t4 -c100 -d5s "http://127.0.0.1:$PORT/" 2>&1 | tee "$OUT/$NAME-reqs-per-sec.txt"
  echo "== wrk: 64KB file (5s, 4x100) =="
  wrk -t4 -c100 -d5s "http://127.0.0.1:$PORT/file64k.bin" 2>&1 | tee "$OUT/$NAME-reqs-per-sec-64kb.txt"

  echo "== HTTP latency ladder (curl x200) =="
  gawk 'BEGIN{for(i=0;i<200;i++){cmd="curl -s -o /dev/null -w %{time_total} http://127.0.0.1:'$PORT'/" ;cmd|getline v;close(cmd);a[i]=v*1e6}}END{n=asort(a,b);s=0;for(i=1;i<=n;i++)s+=b[i];printf "samples %d — p50 %.1f p90 %.1f p99 %.1f p999 %.1f µs (mean %.1f)\n",n,b[int(n*.5)],b[int(n*.9)],b[int(n*.99)],b[int(n*.999)],s/n}' | tee "$OUT/$NAME-latency-http.txt"

  echo "== mpstat during wrk =="
  mpstat -P ALL 1 3 >/tmp/mpstat-$NAME.txt 2>&1 &
  local MP=$!
  wrk -t4 -c100 -d3s "http://127.0.0.1:$PORT/" >/dev/null 2>&1
  wait $MP
  grep -E "Average" /tmp/mpstat-$NAME.txt | head -3 | tee "$OUT/$NAME-mpstat-cpu-split.txt"

  echo "== perf stat under wrk =="
  perf stat -p $SP -e cycles,instructions,branches,branch-misses,dTLB-loads,dTLB-load-misses,cache-misses,L1-dcache-load-misses,L1-icache-load-misses,page-faults,context-switches,cpu-migrations -- sleep 4 >"$OUT/$NAME-perf-counters.txt" 2>&1 &
  local PF=$!
  wrk -t4 -c100 -d4s "http://127.0.0.1:$PORT/" >/dev/null 2>&1
  wait $PF
  grep -E "insn per cycle|branch-misses #|dTLB-load-misses #|cpu-migrations|page-faults" "$OUT/$NAME-perf-counters.txt" | head -6

  kill $SP 2>/dev/null
  echo "$NAME bench DONE"
}

echo "# h2o + caddy same-box baseline — $STAMP"
echo "# host: $(hostname) | cpu: $CPU | kernel: $KERN"
echo "# wrk -t4 -c100 -d5s; root $WORK"

run_one h2o 8095 / || true
run_one caddy 8096 / || true
