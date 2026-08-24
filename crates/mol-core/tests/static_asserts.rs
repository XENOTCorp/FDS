//! Compile-time size/alignment checks (standard [CACHE], [ALLOC]): shared
//! structures never share cache lines unintentionally, and ring/buffer
//! layouts are exactly the computed sizes (no hidden padding surprises).
//! A wrong number here is a compile error, not a runtime failure.

use mol::{Buffer, CachePadded, HotCold, MpmcRing, PaddedCounter, Pool, SpscRing};
use static_assertions::{const_assert, const_assert_eq};

// CachePadded: a value alone on a 64-byte cache line (the false-sharing
// antidote) — the type itself must be exactly one line.
const_assert_eq!(core::mem::align_of::<CachePadded<u64>>(), 64);
const_assert_eq!(core::mem::size_of::<CachePadded<u64>>(), 64);
const_assert_eq!(core::mem::size_of::<PaddedCounter>(), 64);

// HotCold is a plain repr(C) pair: no reordering, no hidden padding.
const_assert_eq!(core::mem::size_of::<HotCold<u64, u64>>(), 16);
const_assert_eq!(core::mem::align_of::<HotCold<u64, u64>>(), 8);

// SpscRing<u64, CAP>: CAP × 8 bytes of slots + two atomics (head, tail).
const_assert_eq!(core::mem::size_of::<SpscRing<u64, 8>>(), 80);
const_assert_eq!(core::mem::size_of::<SpscRing<u64, 1024>>(), 8208);

// MpmcRing<u64, CAP>: CAP cells of (seq, data) + two atomics (head, tail).
const_assert_eq!(core::mem::size_of::<MpmcRing<u64, 8>>(), 144);

// Buffer<N>: N bytes of storage + a usize length, usize-aligned.
const_assert!(core::mem::size_of::<Buffer<1500>>() >= 1500);
const_assert_eq!(core::mem::size_of::<Buffer<1500>>() % 8, 0);

// Pool<T, N>: the arena box pointer + the free-list MPMC ring of
// indices (the arena itself lives on the heap — see `Pool` docs).
const_assert_eq!(core::mem::size_of::<Pool<u64, 4>>(), 88);

#[test]
fn cache_padded_elements_never_share_a_line() {
    // Two adjacent CachePadded values are a full line apart — verified at
    // runtime for the array case (no false sharing between elements).
    let a = [CachePadded::new(0u64), CachePadded::new(0u64)];
    let distance = (&a[1] as *const _ as usize) - (&a[0] as *const _ as usize);
    assert_eq!(distance, 64);
    assert_eq!(mol::cache_line_size(), 64);
}
