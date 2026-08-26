#!/usr/bin/env bash
# Reactor autotune (thesis ch. 17: the optimization lattice is finite, so
# its minimum is computable — compute it on THIS kernel, at startup
# time, and run the winner).
#
# Lattice: { epoll-busy-poll, io-uring, io-uring+SQPOLL } x {tcp, udp}.
# Each candidate runs the real engine (bench-against modes measure the
# actual running server loop, not the in-process bench path) and is
# scored by echo throughput. The winner is the empirical lattice
# minimum; the recommendation is printed and recorded.
#
# Usage: bash scripts/autotune.sh [seconds]   (seconds per candidate, default 2)
#
# NOTE: run on a quiet machine. The epoll busy-poll datapath is
# CPU-sensitive; concurrent builds/tests skew the lattice (measured:
# io-uring "wins" by 22% on UDP while the box is under load, then loses
# on an idle box). Rerun with `mpstat -P ALL 1` idle.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
SECS="${1:-2}"

echo "== build =="
cargo build --release >/dev/null 2>&1 || exit 1
mkdir -p bench-results
OUT="bench-results/autotune.txt"
: > "$OUT"

run_candidate() {
  # $1 = label, $2 = FDS_REACTOR_STRATEGY, $3 = FDS_REACTOR_IO_URING_SQ_THREAD
  local label="$1" strat="$2" sq="$3"
  pkill -x fds 2>/dev/null; sleep 0.4
  # time out the engine: io-uring has stalled on this kernel before.
  FDS_REACTOR_STRATEGY="$strat" FDS_REACTOR_IO_URING_SQ_THREAD="$sq" \
    timeout $((SECS + 6)) ./target/release/fds >/dev/null 2>&1 &
  local pid=$!
  sleep 1.2
  local udp tcp
  udp=$(./target/release/fds --bench-udp-against 127.0.0.1:7777 "$SECS" 2>/dev/null \
        | grep -oP '(?<=echoed )[0-9.]+' | head -1)
  tcp=$(./target/release/fds --bench-tcp-against 127.0.0.1:7778 "$SECS" 2>/dev/null \
        | grep -oP '[0-9.]+(?= Gbps client)' | head -1)
  udp="${udp:-0}"; tcp="${tcp:-0}"
  kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
  # The score is the engine's own echo datapath (UDP); TCP is reported.
  echo "$label $udp $tcp"
}

echo "== lattice measurement (${SECS}s per candidate) =="
declare -a rows
rows+=("$(run_candidate "epoll-busy-poll" "epoll" "0")")
rows+=("$(run_candidate "io-uring" "io-uring" "0")")
rows+=("$(run_candidate "io-uring+SQPOLL" "io-uring" "1")")

echo "strategy  udp_echo_gbps  tcp_gbps"
best=""
best_score=-1
# Admissibility floor: a strategy whose TCP datapath is broken (the
# io-uring stall on this kernel) is not a realization of the full
# engine, so it is disqualified regardless of UDP. Admissible =
# tcp >= 0.5 * max_tcp; winner = max udp among admissible (the
# engine's own echo datapath is UDP, so that is the cost coordinate
# the lattice minimizes after soundness).
max_tcp=0
for row in "${rows[@]}"; do
  read -r _ _ tcp <<< "$row"
  if awk "BEGIN{exit !($tcp > $max_tcp)}"; then max_tcp="$tcp"; fi
done
for row in "${rows[@]}"; do
  read -r label udp tcp <<< "$row"
  printf "%-16s %-13s %s\n" "$label" "$udp" "$tcp"
  if awk "BEGIN{exit !($tcp < $max_tcp * 0.5)}"; then
    echo "  (disqualified: TCP datapath below soundness floor)"
    continue
  fi
  if awk "BEGIN{exit !($udp > $best_score)}"; then
    best_score="$udp"
    best="$label"
  fi
done

case "$best" in
  "epoll-busy-poll") HINT="FDS_REACTOR_STRATEGY=epoll" ;;
  "io-uring") HINT="FDS_REACTOR_IO_URING_SQ_THREAD=0" ;;
  "io-uring+SQPOLL") HINT="FDS_REACTOR_IO_URING_SQ_THREAD=1" ;;
esac

{
  echo "autotune: $(date -Is)"
  echo "lattice minimum (thesis ch. 17) on this kernel: $best (udp echo $best_score Gbps)"
  echo "run with: $HINT"
} | tee -a "$OUT"
echo "recorded in $OUT"
