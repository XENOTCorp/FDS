//! Fixed-capacity buffers and a lock-free object pool (standard \[R\],
//! `ALLOC`).
//!
//! `Buffer` is a fixed-size byte buffer with a length; `Pool` is an
//! arena of `N` values with a lock-free free list (an MPMC ring of free
//! indices), so allocate/return are non-blocking and allocation-free in
//! the hot path.

use crate::ring::MpmcRing;
use core::cell::UnsafeCell;
use core::marker::PhantomData;

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
        Buffer {
            data: [0; N],
            len: 0,
        }
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
    /// The arena, heap-allocated. An inline `[T; N]` would
    /// make `Pool` a `N * size_of::<T>()`-byte by-value type; a 1024-slot
    /// connection table (~200 KiB) would blow a 1 MiB thread stack in
    /// debug builds. The box keeps the struct small; the allocation is
    /// startup-only (the free-list protocol still owns every slot).
    slots: Box<[UnsafeCell<T>]>,
    free: MpmcRing<usize, N>,
}

// SAFETY: slot access is mediated by the free-list ring; a slot is handed
// to exactly one guard at a time (MPMC ring protocol).
unsafe impl<T: Send, const N: usize> Sync for Pool<T, N> {}

impl<T, const N: usize> Pool<T, N> {
    /// Construct a fully initialized pool. Initialization allocates only
    /// at startup; a panicking initializer drops values already created.
    pub fn new_with(mut initialize: impl FnMut(usize) -> T) -> Self {
        let free = MpmcRing::new();
        let slots = (0..N)
            .map(|i| UnsafeCell::new(initialize(i)))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let pool = Pool { slots, free };
        for i in 0..N {
            assert!(pool.free.try_push(i).is_ok());
        }
        pool
    }

    /// Construct a pool of default values.
    pub fn new() -> Self
    where
        T: Default,
    {
        Self::new_with(|_| T::default())
    }

    /// Replace a slot while the pool is exclusively borrowed.
    pub fn initialize(&mut self, i: usize, value: T) {
        *self.slots[i].get_mut() = value;
    }

    /// Allocate a slot, or `None` if the pool is exhausted.
    pub fn try_alloc(&self) -> Option<PoolGuard<'_, T, N>> {
        let idx = self.free.try_pop()?;
        Some(PoolGuard {
            pool: self,
            idx,
            marker: PhantomData,
        })
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
    ///
    /// # Safety
    /// `idx` must have been acquired from this pool and not yet released.
    /// No references or guards to the slot may remain.
    pub unsafe fn release_index(&self, idx: usize) {
        assert!(idx < N, "Pool::release_index: index out of range");
        self.release(idx);
    }

    /// Mutable access to a raw-index-owned slot; used by completion-driven
    /// connection tables. Guard-owned slots should use `DerefMut` instead.
    ///
    /// `&self -> &mut T` is sound here because the pool is interior-mutable
    /// (`UnsafeCell` slots) and exclusive ownership is enforced by the
    /// free-list protocol, not by the borrow checker; the same contract
    /// as `PoolGuard`'s deref.
    ///
    /// # Safety
    /// The caller must exclusively own an index acquired from this pool.
    /// No other references to the slot may exist for the returned lifetime.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get_mut(&self, idx: usize) -> &mut T {
        assert!(idx < N, "Pool::get_mut: index out of range");
        // SAFETY: the caller owns the slot, so it is initialized and not
        // aliased by any guard.
        unsafe { &mut *self.slots[idx].get() }
    }

    fn release(&self, idx: usize) {
        // At least one free-list position exists: this index is owned
        // by the caller and has not yet been returned.
        let mut i = idx;
        loop {
            match self.free.try_push(i) {
                Ok(()) => return,
                Err(b) => i = b,
            }
        }
    }

    /// Number of slots currently in use (approximate under concurrency).
    pub fn in_use(&self) -> usize {
        N - self.free.len()
    }
}

impl<T: Default, const N: usize> Default for Pool<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

/// A borrowed pool slot; returns the slot on drop.
pub struct PoolGuard<'a, T, const N: usize> {
    pool: &'a Pool<T, N>,
    idx: usize,
    // A guard may be shared only when T: Sync, even though Pool is Sync
    // for T: Send. This also models exclusive ownership of the slot.
    marker: PhantomData<&'a mut T>,
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
        unsafe { &*self.pool.slots[self.idx].get() }
    }
}

impl<'a, T, const N: usize> core::ops::DerefMut for PoolGuard<'a, T, N> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the slot is exclusively owned by this guard.
        unsafe { &mut *self.pool.slots[self.idx].get() }
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
        let pool: Pool<u64, 4> = Pool::new_with(|i| i as u64);
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
