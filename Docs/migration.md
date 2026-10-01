# Ownership API migration (pre-1.0)

The ownership hardening changes intentionally reject previously accepted
memory-unsafe usage. This is not a full safety audit of every backend.

## Pools and connection tables

- `Pool::new()` now requires `T: Default` and initializes every value
  before allocation. For other types, use `Pool::new_with(|index| value)`.
- `Pool::initialize` and `ConnTable::initialize` require `&mut self` and
  replace/drop the previous value. Prefer initialization at construction.
- `ConnTable::new()` already creates inactive connections; a separate
  initialization loop is unnecessary.
- Pool values are dropped at teardown. Dropping a guard returns its index
  without destroying the value, allowing reuse.
- Prefer `PoolGuard`/`ConnectionSlot` for safe exclusive access. Guard
  sharing now correctly requires the payload to be `Sync`.
- `Pool::get_mut`, `Pool::release_index`, `ConnTable::conn_mut`, and
  `ConnTable::release_slot` are now **unsafe**. Raw-index callers must
  prove exclusive ownership, no live aliases, and exactly-once release.

```rust
let pool = mol::Pool::<String, 4>::new_with(|i| format!("slot {i}"));
let mut value = pool.try_alloc().unwrap();
value.push_str(" used");
drop(value); // returns the initialized value for reuse
```

## SPSC rings

`SpscRing` is no longer `Sync`; sharing an `Arc<SpscRing>` previously
allowed multiple safe producers/consumers to race over payload memory.
Single-threaded `try_push`/`try_pop` usage is unchanged. For threads, call
`split(&mut self)` and move its unique producer and consumer endpoints
into scoped threads. Endpoint operations require mutable access. Use
`MpmcRing` when multiple producers or consumers are needed.

Both ring types now require power-of-two capacity **at least 2**. A
one-slot MPMC queue cannot distinguish full/empty sequence epochs; a
one-slot SPSC queue has no usable capacity.

## Additional review changes

- `tcp::readv`/`writev` process at most the first 16 buffers in a single
  syscall and return a contiguous prefix length. Retrying callers must
  advance by bytes, not skip an entire partially processed iovec group.
  `writev` now uses `sendmsg(MSG_NOSIGNAL)`.
- `mol::zeroed`, raw io_uring `push`/`register_buffers`, and UDP
  `send_to_zerocopy` are now unsafe APIs with explicit validity/lifetime
  contracts. `Copy` is not proof that an all-zero value is valid.
- The echo engine rejects UDP zero-copy instead of reusing buffers based
  on notification counts or timeouts. Its epoll TCP path now retains
  unsent bytes in a reusable per-connection buffer.
- AF_XDP frame copies retain one checkout identity; all copies become
  invalid after successful transmit/drop. Stale, duplicate, cross-socket,
  and out-of-chunk access panics before memory is exposed. Invalid ring
  sizes/frame counts are rejected instead of silently rounded.
- io_uring readiness uses independent request generations and actual CQE
  flags; finite timeouts now wait. The driver rejects duplicate fds and
  application tokens and does not allow `modify` to register a missing
  token. Datapath instances are single-run; `new_legacy` and engine
  `FDS_IOU_LEGACY=1` explicitly avoid modern operations. Runtime multishot
  rejection fails closed instead of switching live-operation modes.
- `Reactor::poll_busy` now returns one dispatchable batch; the old method
  consumed edges through a no-op handler. Event access is clamped to the
  latest successful poll, and half-close readiness is exposed.
- Unknown JSON keys, invalid startup limits, malformed CLI values, and
  missing explicit config paths fail. Only a missing implicit config may
  use defaults. The historical SQPOLL field specifies idle milliseconds,
  not a CPU ID.
- Metrics binding does not unlink existing files or active endpoints.
  Remove verified stale socket paths explicitly. Dropping an older server
  does not unlink a replacement with a different device/inode identity.
- UDP sockets no longer set SO_REUSEADDR unconditionally. Disabling
  reuseport now produces an exclusive binding.
- Array molecule composition and `Delay` now accept non-`Copy` payloads.
  Purity, totality, and user-step allocation behavior are semantic
  obligations, not guarantees of the marker traits.

## Build and measurement behavior

- `fds-engine --no-default-features` actually disables optional library
  features. SCTP APIs are absent without `sctp`; `--bench-sctp` returns
  an explicit unsupported error in that configuration.
- The benchmark harness no longer substitutes `std::net` when FDS setup
  fails. Errors and parser/checksum smoke-test panics are real failures.
- `TARGET_CPU` no longer inherits the build host's SIMD feature list.
  `RUSTFLAGS_EXTRA` remains an explicit way to customize code generation.
