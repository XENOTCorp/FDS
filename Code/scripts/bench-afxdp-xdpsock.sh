#!/usr/bin/env bash
# Compare FDS AF_XDP zero-copy RX with a linux-xdpsock-shaped rxdrop.
# Needs root, a veth pair, and clang/gcc. Skips cleanly without them.
# Usage: sudo bash scripts/bench-afxdp-xdpsock.sh [seconds]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
SECS="${1:-3}"
OUT="$ROOT/bench-results"
mkdir -p "$OUT"

if [[ "$(id -u)" -ne 0 ]]; then
  echo "bench-afxdp-xdpsock: skip (needs root for AF_XDP + veth)"
  exit 0
fi
for t in gcc ip; do
  command -v "$t" >/dev/null || { echo "missing: $t"; exit 1; }
done

echo "== build FDS and xdpsock_rxdrop =="
( cd "$ROOT" && cargo build --release -p fds-engine >/dev/null )
gcc -O2 -o /tmp/xdpsock_rxdrop "$ROOT/scripts/xdpsock_rxdrop.c" || {
  echo "xdpsock_rxdrop compile failed"; exit 1
}
gcc -O2 -o /tmp/af_xdp_flood -x c - <<'EOF' || true
#define _GNU_SOURCE
#include <arpa/inet.h>
#include <linux/if_ether.h>
#include <linux/if_packet.h>
#include <net/if.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>
int main(int argc, char **argv) {
    if (argc < 2) return 2;
    int ifidx = if_nametoindex(argv[1]);
    int fd = socket(AF_PACKET, SOCK_RAW, htons(ETH_P_ALL));
    struct sockaddr_ll sll = {0};
    sll.sll_family = AF_PACKET;
    sll.sll_ifindex = ifidx;
    sll.sll_halen = ETH_ALEN;
    unsigned char frame[64];
    memset(frame, 0xff, 6);
    memset(frame+6, 0x11, 6);
    frame[12]=0x08; frame[13]=0x00;
    for (;;) {
        if (sendto(fd, frame, 64, 0, (struct sockaddr *)&sll, sizeof(sll)) < 0) break;
    }
    return 0;
}
EOF

ip link del veth0 2>/dev/null || true
ip link add veth0 type veth peer name veth1
ip link set veth0 up
ip link set veth1 up

echo "== xdpsock_rxdrop =="
/tmp/xdpsock_rxdrop veth0 0 "$SECS" | tee "$OUT/xdpsock-rxdrop.txt" &
XP=$!
sleep 0.3
timeout "$SECS" /tmp/af_xdp_flood veth1 >/dev/null 2>&1 || true
wait "$XP" || true

echo "== FDS AF_XDP (UDP echo path, counts RX) =="
FDS_AF_XDP_DEVICE=veth0 FDS_AF_XDP_QUEUE=0 FDS_AF_XDP_ZERO_COPY=1 \
  FDS_CORE_THREADS=1 "$ROOT/target/release/fds" >/tmp/fds-afxdp-bench.log 2>&1 &
FP=$!
sleep 0.8
timeout "$SECS" /tmp/af_xdp_flood veth1 >/dev/null 2>&1 || true
kill -INT "$FP" 2>/dev/null || true
wait "$FP" 2>/dev/null || true
grep -E 'af_xdp|stopped|ZeroCopy|Copy' /tmp/fds-afxdp-bench.log | tee "$OUT/fds-afxdp-rx.txt"

ip link del veth0 2>/dev/null || true
echo
echo "== comparison =="
echo "xdpsock: $(cat "$OUT/xdpsock-rxdrop.txt")"
echo "fds:     $(cat "$OUT/fds-afxdp-rx.txt")"
echo "DONE (veth RX only; a NIC with native XDP is required for TX ZC pps)"
