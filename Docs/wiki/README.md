# FDS wiki

FDS is a Linux network engine for TCP, UDP, and SCTP. It is written in Rust. The library crate is `fds`. The binary `fds` is the echo engine in `fds-engine`. Applications build their own loops on the primitives.

## Pages

1. [Architecture](architecture.md): workers, data flow, loop invariants, crate map
2. [Datapaths](datapaths.md): epoll, io_uring, AF_XDP zero-copy, DPDK note
3. [Configuration](configuration.md): `config.json` and `FDS_*` keys
4. [Operations](operations.md): NIC, kernel, offloads, SCTP
5. [Examples](examples.md): run, public API, custom UDP, custom TCP, AF_XDP
6. [Build](build.md): profiles, `build.sh`, `fds-detect`
7. [Applications](applications.md): how to build a server on FDS

Also see [Getting started](../getting-started.md), [benchmarks](../benchmarks.md), the [thesis](../paper/thesis.pdf), and the [standard](../standard/standard.md).
