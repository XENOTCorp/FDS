# Applications

The `fds` crate is the library surface. The `fds` binary in `fds-engine` is a reference echo loop. Applications build their own loops on the primitives.

## What the library covers

- custom UDP protocols
- custom TCP protocols with full-duplex parallel channels
- batched receive and send
- per-core connection tables with hot and cold cache lines
- metrics over a Unix socket
- `config.json` plus `FDS_*` overrides

HTTP servers, DNS, FTP, and other protocols use the same surface.

## Extension pattern

The pattern is the same for every protocol:

1. Write the handler.
2. Register the sockets with the reactor.
3. Drain each fd to EAGAIN.
4. Keep the hot path allocation-free.
5. Preallocate buffers, event arrays, and tables at startup.

See [examples](examples.md) for a UDP handler sketch and a complete TCP example (`full_duplex_channels`).

## Ownership

One worker owns its poller, sockets, connection table, and counters. Do not share those across threads. The only shared object is the metrics bundle.

Pin workers with `core.pin_cores`. Use SO_REUSEPORT so the kernel steers flows by 4-tuple hash.

## Cache and layout

Keep hot state (sequence numbers, activity, in-flight, fd) on one 64-byte line. Keep cold state (peer, flags) on another. Put per-worker counters on their own line. Align receive buffers to 64 bytes.

TCP lookup must use the epoll token as a slot index. Do not put a hash map on the hot path.

## Configuration

Ship a `config.json` next to the process working directory, or pass a path as the first argument to your binary if you reuse `fds::config::Config::from_file`. Environment overrides use `FDS_<SECTION>_<KEY>`.

Schema: `Code/config/config.schema.json`. Generate it with `fds-detect --generate-schema`.
