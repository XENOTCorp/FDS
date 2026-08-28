#!/usr/bin/env bash
# AF_XDP over veth; full local bring-up for the fds af_xdp datapath.
# Proves, on this laptop with no XDP-capable NIC:
#   1. XDP steering program compiles (clang, BTF maps) and attaches in
#      driver mode to veth0.
#   2. The engine binds an AF_XDP socket on veth0 queue 0 and registers
#      it in the pinned XSKMAP (BPF_OBJ_GET + BPF_MAP_UPDATE_ELEM).
#   3. Frames sent from veth1 are XDP_REDIRECTed into the socket's RX
#      ring, validated/checksummed and echoed into the TX ring
#      (engine log: "N forwarded").
#
# Known limitation (veth, not fds): veth has no XSK TX path, so the
# echoed frame never leaves the TX ring; a real XDP NIC (ixgbe/mlx5)
# completes the loop. The RX datapath is fully proven here.
#
# Usage: bash scripts/veth-af-xdp.sh   (root; installs nothing)
set -u
cd "$(dirname "$0")/.."
HERE="$(pwd)"
PROG="$HERE/scripts/xdp_redirect.c"
PROBE="$HERE/scripts/af_xdp_probe.c"

NEED="clang bpftool gcc ip"
for t in $NEED; do command -v "$t" >/dev/null || { echo "missing: $t"; exit 1; }; done

sudo bash -c '
set -e
ip link del veth0 2>/dev/null || true
ip link add veth0 type veth peer name veth1
ip link set veth0 up; ip link set veth1 up
ip addr add 10.9.9.1/24 dev veth0
ip addr add 10.9.9.2/24 dev veth1
mountpoint -q /sys/fs/bpf || mount -t bpf bpf /sys/fs/bpf
rm -f /sys/fs/bpf/xdp_redirect /sys/fs/bpf/xskmap
'
clang -O2 -g -Wall -target bpf -c "$PROG" -o /tmp/xdp_redirect.o || exit 1
gcc -O2 -o /tmp/af_xdp_probe "$PROBE" || exit 1

sudo bash -c '
set -e
bpftool prog load /tmp/xdp_redirect.o /sys/fs/bpf/xdp_redirect type xdp
bpftool net attach xdp pinned /sys/fs/bpf/xdp_redirect dev veth0
bpftool net show dev veth0
'
echo "--- engine (root; SIGINT stops cleanly) ---"
sudo bash -c "
  env FDS_AF_XDP_DEVICE=veth0 FDS_AF_XDP_QUEUE=0 \
      FDS_AF_XDP_XSKMAP=/sys/fs/bpf/xskmap FDS_CORE_THREADS=1 \
      $HERE/target/release/fds >/tmp/fds-afxdp.log 2>&1 &
  FPID=\$!
  sleep 1.5
  /tmp/af_xdp_probe veth1 \$(cat /sys/class/net/veth0/address) \$(cat /sys/class/net/veth1/address)
  sleep 0.5
  kill -INT \$FPID
  sleep 1
  grep -E 'af_xdp|stopped' /tmp/fds-afxdp.log
"
echo "--- expect: 'af_xdp bound ... forwarding frames' + 'stopped (N forwarded' ---"
