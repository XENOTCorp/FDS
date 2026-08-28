#!/usr/bin/env bash
# H2/H3 (tokio) datapath bench against atomos-proto. Reproduces
# bench-results/atomos-h2h3.txt. Usage: bash scripts/bench-h23.sh [count]
set -u
COUNT="${1:-1000}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ATOMOS="${ROOT}/../../Atomos"
BIN="$ATOMOS/target/release/atomos-proto"
EX="$ATOMOS/target/release/examples/bench_h23"
PORT=8090

pkill -x atomos-proto 2>/dev/null
sleep 0.3

WORK="$(mktemp -d)"
trap 'pkill -x atomos-proto 2>/dev/null; rm -rf "$WORK"' EXIT
mkdir -p "$WORK/root"
printf '<h1>hi</h1>' >"$WORK/root/index.html"
cat >"$WORK/rules.json" <<'EOF'
{"rules":[{"id":"s","module":"static","methods":["GET","HEAD"],"include":["/*"],"exclude":["/metrics"]},{"id":"m","module":"metrics","methods":["GET"],"include":["/metrics"],"exclude":[]}]}
EOF

(cd "$ATOMOS" && cargo build --release --example bench_h23 >/dev/null 2>&1) || exit 1
(cd "$WORK" && "$BIN" --bind "127.0.0.1:$PORT" --root "$WORK/root" --rules "$WORK/rules.json" \
    >"$WORK/server.log" 2>&1) &
SP=$!
for _ in $(seq 1 50); do
    ss -tln 2>/dev/null | grep -q ":$PORT " && break
    sleep 0.2
done

"$EX" --h2-port "$PORT" --h3-port "$PORT" --count "$COUNT"
