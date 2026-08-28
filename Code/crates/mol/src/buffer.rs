//! Fixed-capacity buffers and a lock-free object pool (standard \[R\],
//! `ALLOC`).
//!
//! `Buffer` is a fixed-size byte buffer with a length; `Pool` is an
//! arena of `N` values with a lock-free free list (an MPMC ring of free
//! indices), so allocate/return are non-blocking and allocation-free in
//! the hot path.

use crate::ring::MpmcRing;
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;

/// A fixed-capacity byte buffer (e.g. one packet slot). Zero allocation.
/// Aligned to a cache line so AVX loads on `as_slice` can use aligned
/// paths when the length covers a full line.
#[repr(align(64))]
#[derive(Clone)]
pub struct Buffer<const N: usize> {
    data: [u8; N],
    len: usize,
}

/// Error returned by [`Buffer::set_len`] when the requested length exceeds
/// the buffer capacity (`N`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetLenError;

impl<const N: usize> Buffer<N> {
    /// A zeroed buffer with length 0.
    pub const fn new() -> Self {
        Buffer { data: [0; N], len: 0 }
    }

    /// The buffer contents.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.data[..self.len]
    }

    /// The buffer contents, mutable.
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.data[..self.len]
    }

    /// The full backing array (beyond `len` is zeroed but unused).
    #[inline]
    pub fn as_full_slice(&self) -> &[u8; N] {
        &self.data
    }

    /// The full backing array, mutable (for receive paths where the
    /// length is only known after the syscall; call [`Buffer::set_len`]
    /// afterwards to publish the received length).
    #[inline]
    pub fn as_mut_full_slice(&mut self) -> &mut [u8; N] {
        &mut self.data
    }

    /// Current length.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the buffer is empty (length 0).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Capacity.
    #[inline]
    pub const fn capacity(&self) -> usize {
        N
    }

    /// Set the length after filling the prefix. Returns
    /// [`SetLenError`] if `len > N`.
    #[inline]
    pub fn set_len(&mut self, len: usize) -> Result<(), SetLenError> {
        if len > N {
            return Err(SetLenError);
        }
        self.len = len;
        Ok(())
    }

    /// Clear (length to 0).
    #[inline]
    pub fn clear(&mut self) {
        self.len = 0;
    }
}

impl<const N: usize> Default for Buffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// A lock-free pool of `N` preallocated values.
///
/// The free list is an MPMC ring of indices, so any thread may allocate
/// and any thread may return. Each allocation hands out a [`PoolGuard`]
/// that returns the slot on drop. The pool itself may be shared (`Sync`
/// when `T: Send`); it is not safe to hand out the same slot twice, which
/// the free-list protocol prevents.
pub struct Pool<T, const N: usize> {
    /// The arena, heap-allocated. An inline `[MaybeUninit<T>; N]` would
    /// make `Pool` a `N * size_of::<T>()`-byte by-value type; a 1024-slot
    /// connection table (~200 KiB) would blow a 1 MiB thread stack in
    /// debug builds. The box keeps the struct small; the allocation is
    /// startup-only (the free-list protocol still owns every slot).
    slots: UnsafeCell<Box<[MaybeUninit<T>; N]>>,
    free: MpmcRing<usize, N>,
}

// SAFETY: slot access is mediated by the free-list ring; a slot is handed
// to exactly one guard at a time (MPMC ring protocol).
unsafe impl<T: Send, const N: usize> Sync for Pool<T, N> {}

impl<T, const N: usize> Pool<T, N> {
    /// A new pool; the caller fills each slot with
    /// [`Pool::initialize`] (or `push_initial`).
    pub fn new() -> Self {
        // All indices 0..N are free.
        let free = MpmcRing::new();
        let pool = Pool {
            // SAFETY: `Box::new_uninit` allocates without a stack
            // temporary and without initializing; every slot is written
            // via `initialize` before any guard exists.
            slots: UnsafeCell::new(unsafe { Box::new_uninit().assume_init() }),
            free,
        };
        for i in 0..N {
            // The ring is fresh and single-threaded here; unwrap is sound.
            let _ = pool.free.try_push(i).map_err(|_| ());
        }
        pool
    }

    /// Initialize slot `i` with a value (call once per slot before the
    /// pool is shared).
    pub fn initialize(&self, i: usize, value: T) {
        assert!(i < N, "Pool::initialize: index out of range");
        // SAFETY: slot `i` is uninitialized and not yet handed out.
        unsafe {
            (*self.slots.get())[i].write(value);
        }
    }

    /// Allocate a slot, or `None` if the pool is exhausted.
    pub fn try_alloc(&self) -> Option<PoolGuard<'_, T, N>> {
        let idx = self.free.try_pop()?;
        Some(PoolGuard { pool: self, idx })
    }

    /// Allocate a slot INDEX without a guard. The caller owns the slot
    /// and MUST call [`Pool::release_index`] exactly once; used by
    /// completion-driven datapaths that track slot ownership themselves
    /// (a guard would release the slot on drop, which for a long-lived
    /// connection would double-release at close).
    pub fn try_alloc_index(&self) -> Option<usize> {
        self.free.try_pop()
    }

    /// Return slot `idx` to the free list. The caller must own the slot
    /// (i.e. hold no live guard for it); used by tables that release
    /// slots out-of-order (e.g. connection close).
    pub fn release_index(&self, idx: usize) {
        assert!(idx < N, "Pool::release_index: index out of range");
        self.release(idx);
    }

    /// Mutable access to slot `idx`. The caller must own the slot (hold
    /// its guard, or the table must have handed it out); used by
    /// connection tables to update hot/cold state per packet.
    ///
    /// `&self -> &mut T` is sound here because the pool is interior-mutable
    /// (`UnsafeCell` slots) and exclusive ownership is enforced by the
    /// free-list protocol, not by the borrow checker; the same contract
    /// as `PoolGuard`'s deref.
    #[allow(clippy::mut_from_ref)]
    pub fn get_mut(&self, idx: usize) -> &mut T {
        assert!(idx < N, "Pool::get_mut: index out of range");
        // SAFETY: the caller owns the slot, so it is initialized and not
        // aliased by any guard.
        unsafe { (*self.slots.get())[idx].assume_init_mut() }
    }

    fn release(&self, idx: usize) {
        // Push the index back; the ring never fills (N slots, at most
        // N − 1 in flight by the ring invariant).
        let mut i = idx;
        loop {
            match self.free.try_push(i) {
                Ok(()) => return,
                Err(b) => i = b,
            }
        }
    }

    /// Number of slots currently in use.
    pub fn in_use(&self) -> usize {
        N - self.free.len()
    }
}

impl<T, const N: usize> Default for Pool<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

/// A borrowed pool slot; returns the slot on drop.
pub struct PoolGuard<'a, T, const N: usize> {
    pool: &'a Pool<T, N>,
    idx: usize,
}

impl<'a, T, const N: usize> PoolGuard<'a, T, N> {
    #[inline]
    pub fn index(&self) -> usize {
        self.idx
    }
}

impl<'a, T, const N: usize> core::ops::Deref for PoolGuard<'a, T, N> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the slot is exclusively owned by this guard.
        unsafe { (*self.pool.slots.get())[self.idx].assume_init_ref() }
    }
}

impl<'a, T, const N: usize> core::ops::DerefMut for PoolGuard<'a, T, N> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the slot is exclusively owned by this guard.
        unsafe { (*self.pool.slots.get())[self.idx].assume_init_mut() }
    }
}

impl<'a, T, const N: usize> Drop for PoolGuard<'a, T, N> {
    fn drop(&mut self) {
        self.pool.release(self.idx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_bounds() {
        let mut b = Buffer::<16>::new();
        assert!(b.set_len(16).is_ok());
        assert!(b.set_len(17).is_err());
        b.as_mut_slice()[0] = 7;
        assert_eq!(b.as_slice()[0], 7);
        b.clear();
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn pool_alloc_return_cycle() {
        let pool: Pool<u64, 4> = Pool::new();
        for i in 0..4 {
            pool.initialize(i, i as u64);
        }
        // First allocation returns some slot whose value matches its index.
        let a = pool.try_alloc().expect("a free slot");
        assert_eq!(*a, a.index() as u64);
        drop(a);
        // After returning, allocation succeeds again (the free list is a
        // FIFO ring, so the exact slot is not deterministic; any slot
        // whose value matches its index is correct).
        let a = pool.try_alloc().expect("a free slot again");
        assert_eq!(*a, a.index() as u64);
        drop(a);
        // All four slots are allocatable:
        let g0 = pool.try_alloc().unwrap();
        let g1 = pool.try_alloc().unwrap();
        let g2 = pool.try_alloc().unwrap();
        let g3 = pool.try_alloc().unwrap();
        assert!(pool.try_alloc().is_none()); // exhausted
        drop(g0);
        drop(g1);
        drop(g2);
        drop(g3);
        assert!(pool.try_alloc().is_some()); // free again
    }
}
