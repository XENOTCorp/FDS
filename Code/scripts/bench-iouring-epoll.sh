#!/usr/bin/env bash
# One fresh, single-worker engine per row; never kill unrelated servers.
# Usage: bash scripts/bench-iouring-epoll.sh [seconds, 1..3600]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
SECS="${1:-3}"
if [[ $# -gt 1 || ! "$SECS" =~ ^[1-9][0-9]{0,3}$ ]] || (( SECS > 3600 )); then
  echo 'usage: bench-iouring-epoll.sh [seconds, 1..3600]' >&2
  exit 2
fi
OUT="$ROOT/bench-results"
mkdir -p "$OUT"
FDS="$ROOT/target/release/fds"
pid=""

cleanup() {
  [[ -n "$pid" ]] || return 0
  kill -INT "$pid" 2>/dev/null || true
  for _ in {1..100}; do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  kill -KILL "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  pid=""
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

echo '== build =='
cargo build --release --locked -p fds-engine --no-default-features --features io-uring,af-xdp >/dev/null

run_proto() {
  local strategy="$1" tag="$2" protocol="$3"
  echo "== $tag $protocol ($strategy, one worker) =="
  # Exclusive ports fail if another service owns the endpoints. Disable
  # metrics and AF_XDP so this script cannot replace an endpoint or use
  # a device selected in the caller's config.
  FDS_CORE_THREADS=1 FDS_UDP_REUSEPORT=0 FDS_TCP_REUSEPORT=0 \
    FDS_ENGINE_UDP_BIND=127.0.0.1:7777 FDS_ENGINE_TCP_BIND=127.0.0.1:7778 \
    FDS_METRICS_SOCKET_PATH='' FDS_AF_XDP_DEVICE='' FDS_UDP_ZEROCOPY=0 \
    FDS_REACTOR_STRATEGY="$strategy" FDS_REACTOR_BUSY_POLL=0 \
    "$FDS" >"$OUT/engine-$tag-$protocol.log" 2>&1 &
  pid=$!
  sleep 1
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "engine failed to start; see $OUT/engine-$tag-$protocol.log" >&2
    return 1
  fi
  local address=127.0.0.1:7777
  [[ "$protocol" == tcp ]] && address=127.0.0.1:7778
  timeout --signal=INT --kill-after=5 "$((SECS + 10))" \
    "$FDS" "--bench-$protocol-against" "$address" "$SECS" | tee "$OUT/$tag-$protocol.txt"
  cleanup
}

run_proto epoll-busy-poll epoll udp
run_proto epoll-busy-poll epoll tcp
run_proto io-uring iouring udp
run_proto io-uring iouring tcp

echo '== comparison =='
for row in epoll-udp iouring-udp epoll-tcp iouring-tcp; do
  printf '%-12s %s\n' "$row" "$(< "$OUT/$row.txt")"
done
