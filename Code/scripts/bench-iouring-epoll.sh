#!/usr/bin/env bash
# Compare the kernel TCP/UDP echo datapaths: epoll vs io_uring.
# Loopback. No extra privileges.
# Usage: bash scripts/bench-iouring-epoll.sh [seconds]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
SECS="${1:-3}"
OUT="$ROOT/bench-results"
mkdir -p "$OUT"

echo "== build =="
cargo build --release -p fds-engine >/dev/null

run_one() {
  local strat="$1"
  local tag="$2"
  pkill -x fds 2>/dev/null || true
  sleep 0.3
  echo "== $tag ($strat) =="
  FDS_REACTOR_STRATEGY="$strat" FDS_CORE_THREADS=2 FDS_REACTOR_BUSY_POLL=0 \
    "$ROOT/target/release/fds" >/tmp/fds-"$tag".log 2>&1 &
  local pid=$!
  sleep 1
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "engine failed to start ($tag):"
    cat /tmp/fds-"$tag".log
    return 1
  fi
  "$ROOT/target/release/fds" --bench-tcp-against 127.0.0.1:7778 "$SECS" | tee "$OUT/${tag}-tcp.txt"
  "$ROOT/target/release/fds" --bench-udp-against 127.0.0.1:7777 "$SECS" | tee "$OUT/${tag}-udp.txt"
  kill -INT "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

run_one epoll-busy-poll epoll
run_one io-uring iouring

echo
echo "== comparison =="
printf '%-10s %-s\n' datapath result
printf '%-10s %-s\n' epoll-tcp "$(cat "$OUT/epoll-tcp.txt")"
printf '%-10s %-s\n' iouring-tcp "$(cat "$OUT/iouring-tcp.txt")"
printf '%-10s %-s\n' epoll-udp "$(cat "$OUT/epoll-udp.txt")"
printf '%-10s %-s\n' iouring-udp "$(cat "$OUT/iouring-udp.txt")"
echo "DONE"
