#!/usr/bin/env bash
# PGO loop for the fds binary (needs llvm-profdata).
#
# baseline IPC -> instrument -> run real workloads -> merge -> recompile
# with the profile -> after IPC. RUSTFLAGS env is used: empirically it
# overrides build.rustflags here (cargo 1.97) and, unlike
# `--config build.rustflags`, it reliably invalidates the fingerprint so
# every crate rebuilds instrumented.
#
# Usage: bash scripts/pgo.sh
#   writes the comparison to bench-results/pgo-compare.txt

set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
mkdir -p bench-results
PGO=/tmp/pgo-data
rm -rf "$PGO"
mkdir -p "$PGO"

COMMON="-C target-cpu=native -C relocation-model=pic -C link-arg=-fuse-ld=mold"

echo "== baseline (pre-PGO) IPC =="
perf stat -e cycles,instructions,branches,branch-misses,dTLB-loads,dTLB-load-misses \
  ./target/release/fds --bench 3 2>&1 | grep -E "cycles|instructions|#|branches|dTLB" | head -10 > /tmp/pgo-before.txt

echo "== step 1: instrumented build =="
RUSTFLAGS="$COMMON -C profile-generate=$PGO" cargo build --release -p fds-engine

echo "== step 2: collect profiles from real workloads =="
./target/release/fds --bench 5 >/dev/null
./target/release/fds --bench-large 60000 3 >/dev/null
./target/release/fds --latency 3 >/dev/null
./target/release/fds --latency-tcp 3 >/dev/null

echo "== step 3: merge =="
llvm-profdata merge -o "$PGO/merged.profdata" "$PGO"

echo "== step 4: profile-use rebuild =="
RUSTFLAGS="$COMMON -C profile-use=$PGO/merged.profdata" cargo build --release -p fds-engine

echo "== after (PGO) IPC =="
perf stat -e cycles,instructions,branches,branch-misses,dTLB-loads,dTLB-load-misses \
  ./target/release/fds --bench 3 2>&1 | grep -E "cycles|instructions|#|branches|dTLB" | head -10 > /tmp/pgo-after.txt

{
  echo "# PGO comparison; $(date -u +%F\ %T)Z"
  echo "# host: $(hostname) | cpu: $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | xargs)"
  echo
  echo "## before (baseline release)"
  cat /tmp/pgo-before.txt
  echo
  echo "## after (PGO)"
  cat /tmp/pgo-after.txt
  echo
  echo "## targets"
  echo "  IPC >= 2.4, branch-miss < 5% of branches, dTLB-miss < 5% of loads"
  echo "  (loopback datapath is kernel-dominated per mpstat: %sys+%soft > %usr,"
  echo "   so user-space IPC is bounded by the syscall/softirq share)"
} > bench-results/pgo-compare.txt
echo "done; see bench-results/pgo-compare.txt"
