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
/// The mapping is private, anonymous, and zero-initialized by the kernel.
/// Zero length, arithmetic overflow, and lengths above `isize::MAX` fail.
pub fn huge_page(len: usize) -> Option<HugePageGuard> {
    use rustix::mm::{madvise, mmap_anonymous, Advice, MapFlags, ProtFlags};

    // This Linux/x86-64 mapping uses 4 KiB pages. Checked rounding also
    // preserves the maximum byte-slice length required by Rust.
    let len = len.checked_add(4095)? & !4095;
    if len == 0 || len > isize::MAX as usize {
        return None;
    }

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
    /// The mapped bytes, initially zeroed by the anonymous mapping.
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

/// Initialize a slot with an all-zero value.
///
/// # Safety
/// The all-zero bit pattern must be valid for `T`. `Copy` alone does
/// not imply this: references and nonzero integers are counterexamples.
#[inline]
pub unsafe fn zeroed<T: Copy>(out: &mut core::mem::MaybeUninit<T>) {
    // SAFETY: caller guarantees the zeroed pattern is valid for T.
    unsafe {
        out.write(core::mem::zeroed());
    }
}

/// Allocate and deliberately leak a value, returning a `'static` mutable
/// reference. Each call leaks another allocation; this is not lazy or
/// once-only initialization. Prefer `OnceLock` for shared globals.
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
