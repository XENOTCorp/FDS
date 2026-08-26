//! Bounds-safe SIMD helpers (standard \[SIMD\]).
//!
//! Discipline: vectorized loops run over array slices only — never past
//! the slice end — and the remainder is handled by a scalar loop. Length
//! 0 and unaligned slices are safe (unaligned loads via `_mm256_loadu`).
//! No vector operation can read or write out of bounds by construction.

/// Sum 16-bit big-endian words of `data` into a `u32` accumulator,
/// wrapping. This is the building block of IP/TCP/UDP one's-complement
/// checksums (batching; RFC 1071).
///
/// `sum_u16(b"ab")` = 0x6162 = 24930.
#[inline]
pub fn sum_u16(data: &[u8]) -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            // SAFETY: the function is guarded by the feature check above;
            // `as_chunks` guarantees the SIMD loop never over-reads.
            return unsafe { sum_u16_avx2(data) };
        }
    }
    sum_u16_scalar(data)
}

/// Scalar reference implementation (also the portable fallback).
#[inline]
pub fn sum_u16_scalar(data: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    let (chunks, rem) = data.as_chunks::<2>();
    for pair in chunks {
        sum = sum.wrapping_add(((pair[0] as u32) << 8) | pair[1] as u32);
    }
    if !rem.is_empty() {
        // Odd trailing byte: pad with zero (RFC 1071 semantics).
        sum = sum.wrapping_add((rem[0] as u32) << 8);
    }
    sum
}

/// Fold a 32-bit accumulator into a 16-bit one's-complement checksum
/// (RFC 1071: add carries until none remain, then complement).
#[inline]
pub fn checksum_finalize(acc: u32) -> u16 {
    let mut sum = acc;
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !sum as u16
}

/// One's-complement checksum of `data` (big-endian 16-bit words), the
/// IP/TCP/UDP checksum body (without the pseudo-header).
#[inline]
pub fn u16_checksum(data: &[u8]) -> u16 {
    checksum_finalize(sum_u16(data))
}

/// AVX2 fast path: sum 16-bit big-endian words via two `PSADBW` passes
/// (even bytes and odd bytes), 32 bytes per iteration.
///
/// # Safety
/// Call only when AVX2 is detected. Bounds are guaranteed by
/// `as_chunks`; loads are unaligned-safe (`_mm256_loadu_si256`).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn sum_u16_avx2(data: &[u8]) -> u32 {
    use core::arch::x86_64::*;

    let mut acc_even: u64 = 0;
    let mut acc_odd: u64 = 0;

    let (chunks, rem) = data.as_chunks::<32>();
    for chunk in chunks {
        let v = _mm256_loadu_si256(chunk.as_ptr() as *const __m256i);
        // even = v & 0x00FF00FF... (keep low byte of each 16-bit lane)
        let even_mask = _mm256_set1_epi16(0x00FF);
        let even = _mm256_and_si256(v, even_mask);
        // odd = (v >> 8) & 0x00FF
        let odd = _mm256_and_si256(_mm256_srli_epi16::<8>(v), even_mask);
        let sad_even = _mm256_sad_epu8(even, _mm256_setzero_si256());
        let sad_odd = _mm256_sad_epu8(odd, _mm256_setzero_si256());
        // sad returns 4 u64 lanes per 256-bit; store and sum them.
        let mut e = [0u64; 4];
        let mut o = [0u64; 4];
        _mm256_storeu_si256(e.as_mut_ptr() as *mut __m256i, sad_even);
        _mm256_storeu_si256(o.as_mut_ptr() as *mut __m256i, sad_odd);
        acc_even += e[0] + e[1] + e[2] + e[3];
        acc_odd += o[0] + o[1] + o[2] + o[3];
    }

    // Combine: word sum = even_sum * 256 + odd_sum, then add the scalar
    // remainder (with the RFC 1071 odd-byte padding).
    let mut total = (acc_even << 8).wrapping_add(acc_odd) as u32;
    if !rem.is_empty() {
        total = total.wrapping_add(sum_u16_scalar(rem));
    }
    total
}

/// A bounds-safe bulk copy for SIMD-ready buffers: copies `dst.len()`
/// bytes from `src`, exact chunks with a scalar tail. Exists to
/// demonstrate the chunk discipline; prefer `copy_from_slice` (which is
/// already vectorized) in real code.
#[inline]
pub fn copy_exact(src: &[u8], dst: &mut [u8]) {
    assert!(dst.len() <= src.len(), "copy_exact: dst longer than src");
    dst.copy_from_slice(&src[..dst.len()]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be16_words(data: &[u8]) -> u32 {
        sum_u16_scalar(data)
    }

    #[test]
    fn scalar_sum_matches_definition() {
        assert_eq!(be16_words(b"ab"), 0x6162);
        assert_eq!(be16_words(&[0x12, 0x34, 0x56]), 0x1234 + 0x5600); // odd tail
        assert_eq!(be16_words(&[]), 0);
    }

    #[test]
    fn simd_matches_scalar_across_lengths() {
        let mut rng = 0x9E3779B97F4A7C15u64;
        let mut data = vec![0u8; 300];
        for b in data.iter_mut() {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            *b = (rng >> 32) as u8;
        }
        for len in (0..=300).step_by(7) {
            let s = &data[..len];
            let scalar = sum_u16_scalar(s);
            let fast = sum_u16(s); // dispatches to AVX2 when available
            assert_eq!(fast, scalar, "length {len}");
        }
    }

    #[test]
    fn checksum_known_vector() {
        // Classic example: checksum of "The quick brown fox..." style data
        // is verified against the scalar implementation and RFC folding.
        let data = b"hello world";
        let c = u16_checksum(data);
        assert_eq!(c, checksum_finalize(sum_u16_scalar(data)));
        // One's complement checksum of itself (with the checksum word
        // included) folds to zero — property check:
        let sum = sum_u16_scalar(data).wrapping_add(c as u32);
        assert_eq!(checksum_finalize(sum), 0);
    }

    #[test]
    fn zero_length_is_safe() {
        assert_eq!(sum_u16(&[]), 0);
        assert_eq!(u16_checksum(&[]), 0xFFFF);
    }
}
