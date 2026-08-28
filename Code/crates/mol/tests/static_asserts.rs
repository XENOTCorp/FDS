//! Compile-time size/alignment checks (standard [CACHE], [ALLOC]): shared
//! structures never share cache lines unintentionally, and ring/buffer
//! layouts are exactly the computed sizes (no hidden padding surprises).
//! A wrong number here is a compile error, not a runtime failure.

use mol::{Buffer, CachePadded, HotCold, MpmcRing, PaddedCounter, Pool, SpscRing};
use static_assertions::{const_assert, const_assert_eq};

// CachePadded: a value alone on a 64-byte cache line (the false-sharing
// antidote); the type itself must be exactly one line.
const_assert_eq!(core::mem::align_of::<CachePadded<u64>>(), 64);
const_assert_eq!(core::mem::size_of::<CachePadded<u64>>(), 64);
const_assert_eq!(core::mem::size_of::<PaddedCounter>(), 64);

// HotCold: each half sits on its own cache line (false-sharing split).
const_assert_eq!(core::mem::align_of::<HotCold<u64, u64>>(), 64);
const_assert_eq!(core::mem::size_of::<HotCold<u64, u64>>(), 128);

// SpscRing: payload plus two cache-padded indices (head and tail).
const_assert_eq!(core::mem::size_of::<SpscRing<u64, 8>>() % 64, 0);
const_assert!(core::mem::size_of::<SpscRing<u64, 8>>() >= 64 + 128);
const_assert!(core::mem::size_of::<SpscRing<u64, 1024>>() >= 8192 + 128);

// MpmcRing: cells plus two cache-padded indices.
const_assert_eq!(core::mem::size_of::<MpmcRing<u64, 8>>() % 64, 0);
const_assert!(core::mem::size_of::<MpmcRing<u64, 8>>() >= 128 + 128);

// Buffer<N>: 64-byte aligned so SIMD loads hit aligned addresses.
const_assert_eq!(core::mem::align_of::<Buffer<1500>>(), 64);
const_assert!(core::mem::size_of::<Buffer<1500>>() >= 1500);

// Pool<T, N>: arena box pointer + the free-list MPMC ring of indices
// (the arena itself lives on the heap; see Pool docs).
const_assert_eq!(core::mem::align_of::<Pool<u64, 4>>(), 64);

#[test]
fn cache_padded_elements_never_share_a_line() {
    // Two adjacent CachePadded values are a full line apart; verified at
    // runtime for the array case (no false sharing between elements).
    let a = [CachePadded::new(0u64), CachePadded::new(0u64)];
    let distance = (&a[1] as *const _ as usize) - (&a[0] as *const _ as usize);
    assert_eq!(distance, 64);
    assert_eq!(mol::cache_line_size(), 64);
}
