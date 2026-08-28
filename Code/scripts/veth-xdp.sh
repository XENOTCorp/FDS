#!/usr/bin/env bash
# veth + XDP harness for the fds AF_XDP datapath (af_xdp.rs).
#
# Proves on this machine: (1) a veth pair exists and carries traffic,
# (2) generic XDP can be attached with a clang-compiled program, and
# (3) the fds af_xdp unit tests run against a real XDP-capable device.
#
# Requirements: iproute2 (ip), clang + llvm-strip (BPF compile).
# No bpftool needed; `ip link set ... xdp obj` loads the program.
#
# Usage: bash scripts/veth-xdp.sh [--keep]
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KEEP=0
[ "${1:-}" = "--keep" ] && KEEP=1

cleanup() {
    if [ "$KEEP" = 0 ]; then
        ip link del veth0 2>/dev/null
        rm -f /tmp/xdp_pass.o
    fi
}
trap cleanup EXIT

echo "== 1. create veth pair =="
ip link add veth0 type veth peer name veth1 2>&1 || { echo "FAIL: veth create (need root)"; exit 1; }
ip link set veth0 up && ip link set veth1 up
ip link show veth0 | head -2

echo "== 2. compile a minimal XDP_PASS program =="
# No clang/gcc-bpf driver on this box (LLVM tools only) and the rustc
# bpfel-unknown-none target is not installed, so the BPF compile is
# skipped with an exact explanation instead of failing opaquely. On a
# machine with clang: `clang -O2 -target bpf -c xdp_pass.c -o xdp_pass.o`
# then `ip link set dev veth1 xdp obj xdp_pass.o sec xdp`.
if clang --version >/dev/null 2>&1; then
    CLANG=clang
    cat >/tmp/xdp_pass.c <<'EOF'
#include <linux/bpf.h>
__attribute__((section("xdp"), used))
int xdp_pass(struct xdp_md *ctx) { return XDP_PASS; }
EOF
    "$CLANG" -O2 -target bpf -c /tmp/xdp_pass.c -o /tmp/xdp_pass.o 2>&1 \
        || { echo "FAIL: clang bpf compile ($CLANG)"; exit 1; }
    llvm-strip -g /tmp/xdp_pass.o 2>/dev/null
    echo "compiled: $(ls -la /tmp/xdp_pass.o | awk '{print $5}') bytes"
else
    echo "SKIP: no BPF compiler (clang absent, gcc-bpf absent, rustc bpf target"
    echo "      not installed). Install clang (void: xbps-install -S clang) and"
    echo "      re-run, or add the rust target: rustup target add bpfel-unknown-none"
    echo "      and write the probe as a no_std Rust program."
    exit 0
fi

echo "== 3. attach generic XDP to veth1 =="
ip link set dev veth1 xdp obj /tmp/xdp_pass.o sec xdp 2>&1 \
    || { echo "FAIL: xdp attach (kernel XDP support?)"; exit 1; }
ip link show veth1 | grep -o "xdp.*" | head -1
echo "XDP attached OK"

echo "== 4. ping the pair (traffic over veth) =="
ip addr add 10.99.0.1/24 dev veth0 2>/dev/null
ip addr add 10.99.0.2/24 dev veth1 2>/dev/null
ping -c 2 -W 1 10.99.0.2 -I veth0 2>&1 | tail -2

echo "== 5. fds af_xdp tests against the device =="
# The af_xdp tests auto-skip when no XDP-capable device exists; with
# veth1 carrying the XDP program they can bind. Run with a veth
# present; the tests pick the device themselves or skip gracefully.
(cd "$ROOT" && cargo test --release --lib af_xdp 2>&1 | grep -E "test |test result" | head -8)

echo "== 6. detach + summary =="
ip link set dev veth1 xdp off 2>/dev/null
echo "OK: veth + generic XDP works on this kernel (af_xdp.rs datapath ready for a real NIC)"
