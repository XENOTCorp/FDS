//! Memory layer: hugepage mappings, zeroed init, global state (standard
//! \[ALLOC\], \[CACHE\]).
//!
//! `huge_page` maps a private anonymous region and advises it with
//! `MADV_HUGEPAGE`, so transparent-huge-page 2 MiB pages are used when the
//! kernel provides them and never required.

use core::ffi::c_void;

/// A private anonymous memory mapping, released on drop.
///
/// Alignment: the mapping is page-aligned (at least), so any 64-byte
/// aligned structure placed at its start is cache-line aligned.
pub struct HugePageGuard {
    ptr: *mut c_void,
    len: usize,
}

/// Map `len` bytes with huge pages when available; otherwise a normal
/// mapping advised with `MADV_HUGEPAGE`. Returns `None` on failure.
///
/// The mapping is private and anonymous; nothing is written until the
/// caller initializes it. Use [`HugePageGuard::as_mut_slice`] with
/// `MaybeUninit` + explicit zeroing where the content must start zeroed.
pub fn huge_page(len: usize) -> Option<HugePageGuard> {
    use rustix::mm::{madvise, mmap_anonymous, Advice, MapFlags, ProtFlags};

    // Round to a 4 KiB page so MAP_HUGETLB (2 MiB) can succeed when the
    // kernel provides huge pages; the fallback accepts any length.
    let len = len.next_multiple_of(4096);

    // Attempt a private anonymous mapping, advised with MADV_HUGEPAGE so
    // THP can back it with 2 MiB pages when available (never required).
    let flags = MapFlags::PRIVATE;
    let prot = ProtFlags::READ | ProtFlags::WRITE;
    // SAFETY: anonymous private mapping; the guard owns the pointer and
    // unmaps it in Drop.
    match unsafe { mmap_anonymous(core::ptr::null_mut(), len, prot, flags) } {
        Ok(ptr) => {
            // SAFETY: ptr/len valid for the mapped region; the hint is
            // best-effort and failure is ignored.
            let _ = unsafe { madvise(ptr, len, Advice::LinuxHugepage) };
            Some(HugePageGuard { ptr, len })
        }
        Err(_) => None,
    }
}

impl HugePageGuard {
    /// The mapped region as a byte slice (uninitialized contents — zero
    /// explicitly before reading, per the no-uninitialized-reads policy).
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: the region is valid for `len` bytes for the guard's
        // lifetime; the caller is the sole owner.
        unsafe { core::slice::from_raw_parts_mut(self.ptr.cast::<u8>(), self.len) }
    }

    /// Zero the whole region.
    pub fn zero(&mut self) {
        self.as_mut_slice().fill(0);
    }

    /// The start pointer, cache-line aligned (page-aligned, hence 64-aligned).
    pub fn as_ptr(&self) -> *mut c_void {
        self.ptr
    }

    /// Length in bytes (page-multiple).
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the mapping is empty (always false: mappings are at least
    /// one page).
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Drop for HugePageGuard {
    fn drop(&mut self) {
        // SAFETY: the guard owns this exact mapping and unmaps it once.
        unsafe {
            let _ = rustix::mm::munmap(self.ptr, self.len);
        }
    }
}

// SAFETY: the guard is the sole owner of the mapping; moving it between
// threads is safe as long as no other thread holds a reference.
unsafe impl Send for HugePageGuard {}

/// Initialize a `MaybeUninit` slot with zeroed bytes and assume init.
/// Use only after every byte of the value is initialized (zeroed values
/// are valid only for types where the all-zero bit pattern is valid —
/// e.g. integers, fixed arrays of integers).
#[inline]
pub fn zeroed<T: Copy>(out: &mut core::mem::MaybeUninit<T>) {
    // SAFETY: caller guarantees the zeroed pattern is valid for T.
    unsafe {
        out.write(core::mem::zeroed());
    }
}

/// A lazily initialized global, `Box::leak`-style: returns a `'static`
/// reference, initializing exactly once. Prefer `std::sync::OnceLock` for
/// shared globals; this helper exists for preallocated runtime contexts.
pub fn leak_box<T>(value: T) -> &'static mut T {
    Box::leak(Box::new(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huge_page_maps_aligned_region() {
        let mut g = huge_page(4096).expect("mapping should succeed");
        g.zero();
        assert_eq!(g.len() % 4096, 0);
        assert_eq!(g.as_ptr() as usize % 64, 0);
        // Write/read back to prove the mapping is usable.
        let s = g.as_mut_slice();
        s[0] = 42;
        assert_eq!(s[0], 42);
    }
}
