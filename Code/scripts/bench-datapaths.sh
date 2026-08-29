#!/usr/bin/env bash
# FDS transport datapath battery (2026-08-29).
# In-process benches, dual-stack echo, epoll vs io_uring, userspace TCP,
# and AF_XDP vs xdpsock. Writes Code/bench-results/ (gitignored) and
# copies the measured lines to Docs/benchmarks/YYYY-MM-DD/.
# Usage: bash scripts/bench-datapaths.sh [throughput-seconds]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REPO="$(cd "$ROOT/.." && pwd)"
cd "$ROOT"
SECS="${1:-5}"
LAT_SECS=3
OUT="$ROOT/bench-results"
SNAP="$REPO/Docs/benchmarks/$(date -u +%F)"
mkdir -p "$OUT" "$SNAP"
FDS="$ROOT/target/release/fds"

idle() { sleep 2; }

save() {
  local name="$1"
  shift
  printf '%s\n' "$@" > "$OUT/$name"
  cp "$OUT/$name" "$SNAP/$name"
}

echo "== build =="
cargo build --release -p fds-engine >/dev/null
pkill -x fds 2>/dev/null || true

{
  echo "# host environment; $(date -u +%F\ %T)Z"
  echo "cpu: $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | xargs)"
  echo "kernel: $(uname -r)"
  echo "cores: $(nproc) logical"
  echo "rustc: $(rustc --version)"
  echo "memlock_kB: $(ulimit -l)"
  echo "duration_throughput_s: $SECS"
  echo "duration_latency_s: $LAT_SECS"
  echo "tcp_client: drain-echo (nonblocking write+read)"
  echo "busy_poll: 0 (event-driven)"
} | tee "$OUT/environment.txt"
cp "$OUT/environment.txt" "$SNAP/environment.txt"

echo "== 1. userspace TCP (TSO + RACK) =="
"$FDS" --bench-ustack "$SECS" | tee "$OUT/ustack.txt"
cp "$OUT/ustack.txt" "$SNAP/ustack.txt"
idle

echo "== 2. in-process UDP / large / latency / TCP-latency / SCTP =="
"$FDS" --bench "$SECS" | tee "$OUT/fds-bench.txt"
cp "$OUT/fds-bench.txt" "$SNAP/fds-bench.txt"
idle
"$FDS" --bench-large 60000 "$SECS" | tee "$OUT/fds-bench-large.txt"
cp "$OUT/fds-bench-large.txt" "$SNAP/fds-bench-large.txt"
idle
"$FDS" --latency "$LAT_SECS" | tee "$OUT/fds-inproc-lat.txt"
cp "$OUT/fds-inproc-lat.txt" "$SNAP/fds-inproc-lat.txt"
idle
"$FDS" --latency-tcp "$LAT_SECS" | tee "$OUT/fds-inproc-tcp-lat.txt"
cp "$OUT/fds-inproc-tcp-lat.txt" "$SNAP/fds-inproc-tcp-lat.txt"
idle
"$FDS" --bench-sctp "$LAT_SECS" | tee "$OUT/fds-sctp.txt"
cp "$OUT/fds-sctp.txt" "$SNAP/fds-sctp.txt"
idle

echo "== 3. dual-stack ([::] bind, V4 and V6 clients) =="
pkill -x fds 2>/dev/null || true
sleep 0.3
FDS_ENGINE_UDP_BIND='[::]:7777' FDS_ENGINE_TCP_BIND='[::]:7778' \
  FDS_REACTOR_BUSY_POLL=0 FDS_UDP_IPV6_ONLY=0 FDS_TCP_IPV6_ONLY=0 \
  "$FDS" >"$OUT/engine-dualstack.log" 2>&1 &
DPID=$!
sleep 1
if ! kill -0 "$DPID" 2>/dev/null; then
  echo "dual-stack engine failed to start:" | tee "$OUT/dualstack-error.txt"
  cat "$OUT/engine-dualstack.log"
  cp "$OUT/engine-dualstack.log" "$SNAP/dualstack-error.txt"
else
  "$FDS" --bench-udp-against 127.0.0.1:7777 "$SECS" | tee "$OUT/dualstack-udp-v4.txt"
  "$FDS" --bench-tcp-against 127.0.0.1:7778 "$SECS" | tee "$OUT/dualstack-tcp-v4.txt"
  "$FDS" --bench-udp-against '[::1]:7777' "$SECS" | tee "$OUT/dualstack-udp-v6.txt"
  "$FDS" --bench-tcp-against '[::1]:7778' "$SECS" | tee "$OUT/dualstack-tcp-v6.txt"
  kill -INT "$DPID" 2>/dev/null || true
  wait "$DPID" 2>/dev/null || true
  for f in dualstack-udp-v4.txt dualstack-tcp-v4.txt dualstack-udp-v6.txt dualstack-tcp-v6.txt; do
    cp "$OUT/$f" "$SNAP/$f"
  done
fi
idle

echo "== 4. epoll vs io_uring =="
bash "$ROOT/scripts/bench-iouring-epoll.sh" "$SECS"
for f in epoll-tcp.txt epoll-udp.txt iouring-tcp.txt iouring-udp.txt; do
  [[ -f "$OUT/$f" ]] && cp "$OUT/$f" "$SNAP/$f"
done
idle

echo "== 5. engine latency-against (epoll, then io_uring) =="
for strat in epoll-busy-poll io-uring; do
  tag="${strat%%-busy-poll}"
  tag="${tag//-}"
  pkill -x fds 2>/dev/null || true
  sleep 0.3
  FDS_REACTOR_STRATEGY="$strat" FDS_REACTOR_BUSY_POLL=0 \
    "$FDS" >"$OUT/engine-lat-$tag.log" 2>&1 &
  LPID=$!
  sleep 1
  if kill -0 "$LPID" 2>/dev/null; then
    "$FDS" --latency-against 127.0.0.1:7777 "$LAT_SECS" | tee "$OUT/lat-against-$tag.txt"
    cp "$OUT/lat-against-$tag.txt" "$SNAP/lat-against-$tag.txt"
    kill -INT "$LPID" 2>/dev/null || true
    wait "$LPID" 2>/dev/null || true
  else
    echo "engine failed ($strat)" | tee "$OUT/lat-against-$tag.txt"
    cat "$OUT/engine-lat-$tag.log"
    cp "$OUT/lat-against-$tag.txt" "$SNAP/lat-against-$tag.txt"
  fi
  idle
done

echo "== 6. AF_XDP vs xdpsock =="
bash "$ROOT/scripts/bench-afxdp-xdpsock.sh" "$LAT_SECS" | tee "$OUT/afxdp-xdpsock.txt"
cp "$OUT/afxdp-xdpsock.txt" "$SNAP/afxdp-xdpsock.txt"
for f in xdpsock-rxdrop.txt xdpsock-rxdrop.err fds-afxdp-rx.txt afxdp-skip.txt fds-afxdp-bench.log; do
  [[ -f "$OUT/$f" ]] && cp "$OUT/$f" "$SNAP/$f"
done

pkill -x fds 2>/dev/null || true
echo
echo "snapshot: $SNAP"
echo "DONE"
