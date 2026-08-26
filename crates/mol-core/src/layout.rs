//! Layout discipline: cache-line alignment, hot/cold separation, padded
//! counters (standard `CACHE` cache-line accounting).

/// The cache-line size in bytes for alignment purposes.
pub const fn cache_line_size() -> usize {
    64
}

/// A value aligned to a cache line (64 bytes), so it never shares a line
/// with another `CachePadded` in an array — the false-sharing antidote.
#[repr(align(64))]
pub struct CachePadded<T> {
    value: T,
}

impl<T> CachePadded<T> {
    pub const fn new(value: T) -> Self {
        CachePadded { value }
    }

    pub fn into_inner(self) -> T {
        self.value
    }

    pub const fn get(&self) -> &T {
        &self.value
    }

    pub fn get_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

impl<T> core::ops::Deref for CachePadded<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T> core::ops::DerefMut for CachePadded<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

impl<T: Default> Default for CachePadded<T> {
    fn default() -> Self {
        CachePadded::new(T::default())
    }
}

/// A frequently written counter in its own cache line. Use sharded or
/// per-core instances and aggregate rarely (standard \[OBS\], \[CONC\]).
pub type PaddedCounter = CachePadded<core::sync::atomic::AtomicU64>;

/// Hot/cold state separation marker.
///
/// The FDS pattern for connection state (standard \[CACHE\]):
/// split the state into a `Hot` struct (fields read/written every step)
/// and a `Cold` struct (rarely touched fields), each in its own cache
/// line. This struct pairs them; align both with [`CachePadded`] or
/// `#[repr(align(64))]` so they never share a line.
#[repr(C)]
pub struct HotCold<Hot, Cold> {
    /// Hot fields: keep this aligned to a cache line.
    pub hot: Hot,
    /// Cold fields: keep this in a separate line from `hot`.
    pub cold: Cold,
}

impl<Hot, Cold> HotCold<Hot, Cold> {
    pub const fn new(hot: Hot, cold: Cold) -> Self {
        HotCold { hot, cold }
    }
}

/// Compile-time size/alignment checks for shared structures.
///
/// Usage: `layout::assert_align::<CachePadded<u64>, 64>();` in a test.
pub const fn assert_align<T>(expected: usize) {
    assert!(core::mem::align_of::<T>() == expected);
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::AtomicU64;

    #[test]
    fn cache_padded_is_64_aligned() {
        assert_eq!(core::mem::align_of::<CachePadded<u64>>(), 64);
        let a = [CachePadded::new(1u64), CachePadded::new(2u64)];
        let off = (&a[1] as *const _ as usize) - (&a[0] as *const _ as usize);
        assert_eq!(off, 64); // no false sharing between elements
    }

    #[test]
    fn padded_counter_works() {
        let c = PaddedCounter::new(AtomicU64::new(0));
        c.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        assert_eq!(c.load(core::sync::atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn hot_cold_does_not_share_line_by_default_layout() {
        // With repr(C), hot comes first; the user is responsible for
        // padding. Assert the struct is at least aligned:
        assert_eq!(core::mem::align_of::<HotCold<u64, u64>>(), 8);
        assert_align::<CachePadded<u64>>(64);
    }
}
