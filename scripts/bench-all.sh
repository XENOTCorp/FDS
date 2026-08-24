#!/usr/bin/env bash
# Full benchmark battery — re-runs every metric in bench-results/.
# Usage: bash scripts/bench-all.sh   (builds both repos first)
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ATOMOS="$ROOT/../Atomos"
cd "$ROOT"

echo "== build =="
(cd "$ATOMOS" && cargo build --release >/dev/null 2>&1) || exit 1
cargo build --release >/dev/null 2>&1 || exit 1

echo "== 1. bench-full.sh (FDS, Atomos H1, netperf/iperf, deep metrics) =="
bash scripts/bench-full.sh || echo "bench-full.sh exited $?"

echo "== 2. engine echo A/B (TCP + UDP against a running engine) =="
pkill -x fds 2>/dev/null; sleep 0.3
./target/release/fds >/dev/null 2>&1 & EP=$!
sleep 1.5
./target/release/fds --bench-tcp-against 127.0.0.1:7778 4 2>&1 | tee bench-results/engine-tcp-echo.txt
./target/release/fds --bench-udp-against 127.0.0.1:7777 4 2>&1 | tee bench-results/engine-udp-echo.txt
./target/release/fds --latency-against 127.0.0.1:7777 2 2>&1 | tail -2 >> bench-results/engine-udp-echo.txt
kill $EP 2>/dev/null

echo "== 3. SCTP =="
./target/release/fds --bench-sctp 3 2>&1 | tee bench-results/sctp-throughput.txt

echo "== 4. H2/H3 (tokio) =="
bash scripts/bench-h23.sh 2000 2>&1 | tee bench-results/atomos-h2h3-live.txt

echo "== 5. bench-atomos.sh (perf counters under wrk) =="
bash scripts/bench-atomos.sh 4 2>&1 | tail -3

echo "DONE"
