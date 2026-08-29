# Examples

All `cargo` commands run from `Code/`.

## 1. Run the engine

```sh
cargo run --release -p fds-engine
```

The engine starts one worker per logical CPU. UDP echo binds 127.0.0.1:7777. TCP echo binds 127.0.0.1:7778.

## 2. Write a custom UDP handler

Use the engine as the reference loop. Replace the echo handler with your protocol.

1. Bind the socket in the worker. Register it with `reactor.register(fd, token, Interest::Readable)`.
2. On a readable event, call `recv_batch` (recvmmsg, preallocated array) and process each datagram.
3. Respond with `send_batch` (sendmmsg).
4. Drain until EAGAIN. Then return to the poll loop.

Allocate the batch arrays once at startup. Do not allocate on the hot path.

## 3. Write a custom TCP protocol server

`examples/full_duplex_channels.rs` is a complete custom server on the primitives. It runs a reactor loop over a listener and channels. It drains to EAGAIN. When an echo write blocks, it stops reading until the socket is writable.

```sh
cargo run --release -p fds --example full_duplex_channels
```

It opens eight parallel TCP connections and streams in both directions on each. The measured aggregate echo rate is about 1.1 GiB/s on the reference machine.

## 4. Build a molecule

The `mol` crate provides atoms (pure and effectful transformations), molecules (stateful transformations), lock-free rings, fixed-capacity buffers, and SIMD checksums. Templates are in `Code/templates/`.

```rust
use mol::molecule::Molecule;
// A pure atom: adds a constant.
// Compose with `then` (sequential) and `par` (parallel).
let pipeline = add(5).then(double);   // (x + 5) * 2
```

See the crate documentation (`cargo doc -p mol`) and the templates for the authoring pattern.

## 5. Add a checksum atom

Checksum atoms live in `fds::checksum` (crate-private) and `mol::simd` (public). `sum_u16` sums big-endian 16-bit words with AVX2. `checksum_finalize` folds to the one's-complement result. A new checksum follows the same pattern: vectorized loop over array slices, scalar remainder, bounds by construction.

## 6. Configure the engine

Write `Code/config.json`, or override with environment variables. Every key is optional.

```json
{
  "core": { "threads": 4, "pin_cores": true },
  "reactor": { "strategy": "epoll-busy-poll", "busy_poll": false },
  "engine": { "udp_bind": "0.0.0.0:7777", "tcp_bind": "0.0.0.0:7778" }
}
```

```sh
FDS_CORE_THREADS=4 FDS_REACTOR_STRATEGY=io-uring cargo run --release -p fds-engine
```

## 7. Bench a custom handler

The benchmark clients measure any echo-capable server:

```sh
cargo run --release -p fds-engine -- --bench-udp-against 127.0.0.1:PORT 5
cargo run --release -p fds-engine -- --bench-tcp-against 127.0.0.1:PORT 5
```

## 8. Use the io_uring strategy

```sh
FDS_REACTOR_STRATEGY=io-uring cargo run --release -p fds-engine
```

The autotuner compares strategies on the current machine and kernel:

```sh
bash scripts/autotune.sh
```

## 9. Adopt the public API

`fds::api` is the surface other programs use. Two shapes sit on one
core:

- Driver/callback: register fds, poll, read `Event`s. `EpollDriver` and
  `IoUringDriver` implement `Driver`.
- Async: `poll_read` / `poll_write` / `poll_accept` / `poll_recv_from`
  in the `std::task::Poll` shape. Drive them with `noop_context()` or
  with any runtime that calls `poll_*`.

```rust
use fds::api::{AsyncAccept, Driver, EpollDriver, Interest, TcpListener, noop_context};
use std::os::fd::AsRawFd;
use std::task::Poll;

let mut driver = EpollDriver::new(64).unwrap();
let mut listener = TcpListener::bind("127.0.0.1:0".parse().unwrap()).unwrap();
driver.register(listener.as_raw_fd(), 0, Interest::Readable).unwrap();
let mut ctx = noop_context();
loop {
    driver.poll(Some(std::time::Duration::from_millis(100))).unwrap();
    for ev in driver.events() {
        if ev.token == 0 && ev.readable {
            if let Poll::Ready(Ok(Some(_stream))) = listener.poll_accept(&mut ctx) {
                // handle the stream
            }
        }
    }
    driver.clear_events();
}
```

## 10. AF_XDP on a supported NIC

Use an XDP-capable NIC (ixgbe, i40e, ice, mlx5). Replace `eth0` with your
device name. Each worker binds one queue. Native zero-copy is the default.

```json
{
  "af_xdp": {
    "device": "eth0",
    "queues": [0, 1],
    "zero_copy": true,
    "numa": true,
    "xskmap": "/sys/fs/bpf/xskmap"
  }
}
```

Bring up a local veth pair:

```sh
bash scripts/veth-af-xdp.sh
```
