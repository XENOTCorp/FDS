# Datapaths

## Kernel datapath (default)

The engine runs on the kernel socket path: epoll readiness, recvmmsg and sendmmsg batches of 64 datagrams, readv and writev on TCP. This is the default. On the reference machine it is the fastest strategy. See [benchmarks](../benchmarks.md).

## io_uring

The `io-uring` reactor runs UDP and TCP echo through the ring (IORING_OP_RECVMSG/SENDMSG/ACCEPT/READ/WRITE). On this kernel, io_uring matches epoll on UDP and stalls on TCP. SQPOLL loses on two physical cores. The startup autotuner selects the strategy per machine.

On server hardware, register files and buffers and enable multishot receive. Config keys: `FDS_REACTOR_IO_URING_ENTRIES` and `FDS_REACTOR_IO_URING_SQ_THREAD`.

## AF_XDP

The `af-xdp` path is a frame pipeline on an AF_XDP socket: umem, rx/tx/fill/completion rings, bind, and a validate-and-echo `process_frame`. The receive path is proven on a veth pair with a driver-mode XDP redirect program. Transmit needs a NIC with an XDP queue (ixgbe, i40e, ice, mlx5). Set `af_xdp.device` and `af_xdp.queue` in `config.json` to start a forwarding thread.

## DPDK

DPDK is not in this tree. Use it when the target NIC lacks XDP. It needs hugepages, UIO/VFIO, and `dpdk-devbind.py`. AF_XDP covers the same ground on a stock kernel.
