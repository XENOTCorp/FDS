#!/usr/bin/env bash
# Compare FDS AF_XDP RX with a linux-xdpsock-shaped rxdrop.
# Prefers host root + veth. Without root, uses a user+net namespace
# when the kernel allows it. Compiles helpers into target/bench
# because /tmp is noexec on some hosts. Skips only when neither root
# nor a user namespace can bind AF_XDP.
# Usage: bash scripts/bench-afxdp-xdpsock.sh [seconds]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
SECS="${1:-3}"
OUT="$ROOT/bench-results"
BIN="$ROOT/target/bench"
mkdir -p "$OUT" "$BIN"

# Re-enter as mapped root in a fresh netns when the caller is unprivileged.
if [[ "$(id -u)" -ne 0 ]]; then
  if unshare --user --net --map-root-user true 2>/dev/null; then
    echo "bench-afxdp-xdpsock: no host root; using user+net namespace"
    exec unshare --user --net --map-root-user -- "$ROOT/scripts/bench-afxdp-xdpsock.sh" "$SECS"
  fi
  echo "bench-afxdp-xdpsock: skip (needs root, or unprivileged user namespaces)"
  echo "skip: no root and no user namespace" > "$OUT/afxdp-skip.txt"
  exit 0
fi

for t in gcc ip; do
  command -v "$t" >/dev/null || { echo "missing: $t"; exit 1; }
done

echo "== build FDS and helpers =="
( cd "$ROOT" && cargo build --release -p fds-engine >/dev/null )
gcc -O2 -o "$BIN/xdpsock_rxdrop" "$ROOT/scripts/xdpsock_rxdrop.c"
gcc -O2 -o "$BIN/af_xdp_flood" -x c - <<'EOF'
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
    if (fd < 0) return 1;
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

# Fit umem into RLIMIT_MEMLOCK (8 MiB on many unprivileged accounts).
MEMLOCK_KB="$(ulimit -l)"
FRAMES=4096
RING=256
if [[ "$MEMLOCK_KB" != "unlimited" ]]; then
  MAX_BYTES=$((MEMLOCK_KB * 1024 / 2))
  FRAMES=$((MAX_BYTES / 4096))
  if (( FRAMES < 256 )); then FRAMES=256; fi
  # Floor to a power of two.
  P=256
  while (( P * 2 <= FRAMES )); do P=$((P * 2)); done
  FRAMES=$P
fi
echo "umem: ${FRAMES} x 4096 B (memlock ${MEMLOCK_KB} kB), ring ${RING}"

ip link set lo up 2>/dev/null || true
ip link del veth0 2>/dev/null || true
ip link add veth0 type veth peer name veth1
ip link set veth0 up
ip link set veth1 up

echo "== xdpsock_rxdrop =="
set +e
"$BIN/xdpsock_rxdrop" veth0 0 "$SECS" "$FRAMES" "$RING" \
  >"$OUT/xdpsock-rxdrop.txt" 2>"$OUT/xdpsock-rxdrop.err" &
XP=$!
sleep 0.3
timeout "$SECS" "$BIN/af_xdp_flood" veth1 >/dev/null 2>&1
wait "$XP"
XDP_RC=$?
set -e
cat "$OUT/xdpsock-rxdrop.err" || true
cat "$OUT/xdpsock-rxdrop.txt" || true

echo "== FDS AF_XDP (UDP echo path, counts RX) =="
set +e
FDS_AF_XDP_DEVICE=veth0 FDS_AF_XDP_QUEUE=0 FDS_AF_XDP_ZERO_COPY=1 \
  FDS_AF_XDP_NUM_FRAMES="$FRAMES" FDS_AF_XDP_RING_SIZE="$RING" \
  FDS_CORE_THREADS=1 "$ROOT/target/release/fds" \
  >"$OUT/fds-afxdp-bench.log" 2>&1 &
FP=$!
sleep 0.8
timeout "$SECS" "$BIN/af_xdp_flood" veth1 >/dev/null 2>&1
kill -INT "$FP" 2>/dev/null
wait "$FP" 2>/dev/null
set -e
grep -E 'af_xdp|stopped|ZeroCopy|Copy|error' "$OUT/fds-afxdp-bench.log" \
  | tee "$OUT/fds-afxdp-rx.txt" || true

ip link del veth0 2>/dev/null || true
echo
echo "== comparison =="
echo "xdpsock: $(tr '\n' ' ' < "$OUT/xdpsock-rxdrop.txt" 2>/dev/null)"
echo "xdpsock-err: $(tr '\n' ' ' < "$OUT/xdpsock-rxdrop.err" 2>/dev/null)"
echo "fds:     $(tr '\n' ' ' < "$OUT/fds-afxdp-rx.txt" 2>/dev/null)"
if [[ "$XDP_RC" -ne 0 ]]; then
  echo "NOTE: xdpsock bind/umem failed (rc $XDP_RC). veth RX only; a NIC with native XDP is required for TX ZC pps."
fi
echo "DONE (veth RX only; a NIC with native XDP is required for TX ZC pps)"
