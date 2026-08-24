#!/usr/bin/env bash
# perf.sh — profiling wrapper for the `fds` binary (crates/fds-core).
#
# Every command cd's to the workspace root and invokes the binary with
# the final `--bench <seconds>` / `--fuzz <iters>` CLI. That arg dispatch
# is wired in src/main.rs at the integration milestone; until then the
# bench subcommands print the "no config at --bench" notice and idle.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

usage() {
    cat <<'EOF'
Usage: perf.sh <command> [args]

  bench [seconds]        run the in-crate UDP loopback benchmark
  stat [seconds]         perf stat (cache/instructions/cycles/branches)
  record [seconds]       perf record -g, then perf report
  asm <symbol>           cargo asm (cargo-asm) for <symbol> in the fds bin
  llvm-mca <file>        print llvm-mca help text
  cachegrind [seconds]   valgrind --tool=cachegrind on the benchmark
  iperf3                 print the raw-throughput iperf3 command
EOF
}

SECONDS="${2:-5}"

case "${1:-}" in
bench)
    echo "perf.sh: cargo run --release -p fds-core -- --bench $SECONDS"
    cargo run --release -p fds-core -- --bench "$SECONDS"
    ;;
stat)
    echo "perf.sh: perf stat -e cache-misses,cache-references,instructions,cycles,branch-misses -- cargo run --release -p fds-core -- --bench $SECONDS"
    perf stat -e cache-misses,cache-references,instructions,cycles,branch-misses \
        cargo run --release -p fds-core -- --bench "$SECONDS"
    ;;
record)
    echo "perf.sh: perf record -g -- cargo run --release -p fds-core -- --bench $SECONDS"
    perf record -g -- cargo run --release -p fds-core -- --bench "$SECONDS"
    echo "perf.sh: perf report"
    perf report
    ;;
asm)
    symbol="${2:-}"
    if [[ -z "$symbol" ]]; then
        echo "perf.sh: asm requires a <symbol> argument" >&2
        usage >&2
        exit 2
    fi
    echo "perf.sh: cargo asm -p fds-core --bin fds --release $symbol"
    cargo asm -p fds-core --bin fds --release "$symbol"
    ;;
llvm-mca)
    file="${2:-}"
    command -v llvm-mca >/dev/null 2>&1 || {
        echo "perf.sh: llvm-mca not installed (install LLVM)" >&2
        exit 1
    }
    echo "perf.sh: llvm-mca help (file arg '${file:-<none>}' accepted for future use)"
    llvm-mca --help
    ;;
cachegrind)
    echo "perf.sh: valgrind --tool=cachegrind -- cargo run --release -p fds-core -- --bench $SECONDS"
    valgrind --tool=cachegrind \
        cargo run --release -p fds-core -- --bench "$SECONDS"
    ;;
iperf3)
    cat <<'EOF'
Raw UDP throughput with iperf3 (loopback; tune -l to the engine datagram):
  server: iperf3 -s -p 5201
  client: iperf3 -c 127.0.0.1 -u -b 0 -l 1400 -t 30 -p 5201
  downlink: iperf3 -c 127.0.0.1 -u -b 0 -l 1400 -t 30 -R -p 5201
For the engine's own numbers use: perf.sh bench <seconds>
EOF
    ;;
*)
    usage
    exit 2
    ;;
esac
