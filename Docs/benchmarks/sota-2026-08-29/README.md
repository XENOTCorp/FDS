# FDS datapath snapshot, 2026-08-29

Host: Intel Core i5-5200U, 4 logical / 2 physical cores, kernel
7.2.0_1 (Void Linux), loopback. rustc 1.98.0. Event-driven
(`FDS_REACTOR_BUSY_POLL=0`). TCP client drains the echo.

Runner: `Code/scripts/bench-datapaths.sh 5`. Isolated epoll vs
io_uring: `Code/scripts/bench-iouring-epoll.sh 5` (fresh engine per
row). AF_XDP: `Code/scripts/bench-afxdp-xdpsock.sh 3` (user+net
namespace; `/tmp` is noexec on this host).

Tables and method: `Docs/benchmarks.md` (section “Datapath comparison
(2026-08-29)”).
