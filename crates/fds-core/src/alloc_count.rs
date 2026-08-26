//! Allocation-counting global allocator, test-only (the
//! zero-allocation datapath is a *typed* claim, and a counting
//! allocator machine-checks it — the hot loop either allocates or it
//! does not, and this module observes which).
//!
//! The counter is per-thread (`thread_local!`), so a test runs its
//! measured loop on a dedicated thread, resets the counter after setup,
//! and asserts it is still zero when the loop ends. Other test threads
//! cannot pollute the measurement.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

/// Forwards every allocation to the system allocator and records it on
/// the allocating thread.
pub struct CountingAlloc;

// SAFETY: every operation is forwarded verbatim to `System`, a sound
// `GlobalAlloc`; the counter increment happens before the forward and
// neither reads nor writes the returned pointer, so it cannot affect
// the allocator contract (validity, layout, alignment).
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCS.with(|c| c.set(c.get() + 1));
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.with(|c| c.set(c.get() + 1));
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// Zero the calling thread's allocation counter. Call after setup and
/// before the measured loop.
pub fn reset() {
    ALLOCS.with(|c| c.set(0));
}

/// Allocations observed on the calling thread since the last reset.
pub fn count() -> usize {
    ALLOCS.with(|c| c.get())
}
