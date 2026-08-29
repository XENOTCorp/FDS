#!/usr/bin/env bash
# Compare the kernel TCP/UDP echo datapaths: epoll vs io_uring.
# Loopback. No extra privileges. One fresh engine per (strategy, protocol)
# so a TCP flood cannot poison the UDP row.
# Usage: bash scripts/bench-iouring-epoll.sh [seconds]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
SECS="${1:-3}"
OUT="$ROOT/bench-results"
mkdir -p "$OUT"
FDS="$ROOT/target/release/fds"

echo "== build =="
cargo build --release -p fds-engine >/dev/null

run_proto() {
  local strat="$1"
  local tag="$2"
  local proto="$3"
  pkill -x fds 2>/dev/null || true
  sleep 0.4
  echo "== $tag $proto ($strat) =="
  FDS_REACTOR_STRATEGY="$strat" FDS_REACTOR_BUSY_POLL=0 \
    "$FDS" >"$OUT/engine-$tag-$proto.log" 2>&1 &
  local pid=$!
  sleep 1
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "engine failed to start ($tag $proto):"
    cat "$OUT/engine-$tag-$proto.log"
    return 1
  fi
  if [[ "$proto" == "tcp" ]]; then
    "$FDS" --bench-tcp-against 127.0.0.1:7778 "$SECS" | tee "$OUT/${tag}-tcp.txt"
  else
    "$FDS" --bench-udp-against 127.0.0.1:7777 "$SECS" | tee "$OUT/${tag}-udp.txt"
  fi
  kill -INT "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  sleep 2
}

# UDP first, then TCP. Fresh engine each row.
run_proto epoll-busy-poll epoll udp
run_proto epoll-busy-poll epoll tcp
run_proto io-uring iouring udp
run_proto io-uring iouring tcp

echo
echo "== comparison =="
printf '%-10s %-s\n' datapath result
printf '%-10s %-s\n' epoll-udp "$(cat "$OUT/epoll-udp.txt")"
printf '%-10s %-s\n' iouring-udp "$(cat "$OUT/iouring-udp.txt")"
printf '%-10s %-s\n' epoll-tcp "$(cat "$OUT/epoll-tcp.txt")"
printf '%-10s %-s\n' iouring-tcp "$(cat "$OUT/iouring-tcp.txt")"
echo "DONE"
