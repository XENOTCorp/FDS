//! Checksum atoms (standard \[SIMD\] batching): IP/TCP/UDP use
//! the one's-complement sum from the framework's bounds-safe SIMD helpers;
//! SCTP uses CRC32c (Castagnoli), table-driven with a byte-at-a-time
//! reference (vectorizable later). All parsers validate lengths before
//! reading (standard \[SEC\]).

use mol::{checksum_finalize, sum_u16};

/// IPv4 header checksum (RFC 791): one's-complement sum of the header
/// with the checksum field zeroed. Bounds-safe by construction (operates
/// on the slice the caller validated).
pub(crate) fn ip_checksum(header: &[u8]) -> u16 {
    checksum_finalize(sum_u16(header))
}

/// TCP checksum including the IPv4 pseudo-header (RFC 793). `src`/`dst`
/// are the 4-byte addresses, `tcp_len` the TCP segment length in bytes,
/// `data` the full TCP segment (header + payload).
pub(crate) fn tcp_checksum(src: [u8; 4], dst: [u8; 4], tcp_len: u16, data: &[u8]) -> u16 {
    let mut sum = sum_u16(&src);
    sum = sum.wrapping_add(sum_u16(&dst));
    // Pseudo-header: zeros (1 byte) + protocol 6 + TCP length.
    sum = sum.wrapping_add(6);
    sum = sum.wrapping_add(tcp_len as u32);
    checksum_finalize(sum.wrapping_add(sum_u16(data)))
}

/// UDP checksum including the IPv4 pseudo-header (RFC 768). A zero
/// checksum means "no checksum" on IPv4; this function always computes.
pub(crate) fn udp_checksum(src: [u8; 4], dst: [u8; 4], udp_len: u16, data: &[u8]) -> u16 {
    let mut sum = sum_u16(&src);
    sum = sum.wrapping_add(sum_u16(&dst));
    sum = sum.wrapping_add(17);
    sum = sum.wrapping_add(udp_len as u32);
    checksum_finalize(sum.wrapping_add(sum_u16(data)))
}

/// RFC 3309 / RFC 4960 CRC32c (Castagnoli, poly 0x1EDC6F41 reflected).
/// Table-driven, byte-at-a-time. Init = all ones, final = complement.
pub(crate) fn sctp_checksum(data: &[u8]) -> u32 {
    crc32c_impl(data)
}

const fn crc32c_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x82F63B78
            } else {
                crc >> 1
            };
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

fn crc32c_impl(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = crc32c_table();
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        let idx = ((crc ^ b as u32) & 0xFF) as usize;
        crc = TABLE[idx] ^ (crc >> 8);
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ip_checksum_known_vector() {
        // A minimal IPv4 header (20 bytes) with checksum zeroed:
        // 45 00 00 3c 00 00 00 00 40 11 00 00 c0 a8 00 01 c0 a8 00 02
        let hdr = [
            0x45, 0x00, 0x00, 0x3c, 0x00, 0x00, 0x00, 0x00, 0x40, 0x11, //
            0x00, 0x00, 0xc0, 0xa8, 0x00, 0x01, 0xc0, 0xa8, 0x00, 0x02,
        ];
        let c = ip_checksum(&hdr);
        // One's-complement sum including the checksum field folds to 0.
        let sum = sum_u16(&hdr).wrapping_add(c as u32);
        assert_eq!(checksum_finalize(sum), 0, "checksum of header+csum folds to 0");
        assert_eq!(c, 0xf95d); // hand-computed RFC-style value
    }

    #[test]
    fn udp_checksum_folds_to_zero_with_csum_included() {
        // UDP datagram: pseudo-header + UDP header (csum zeroed) + payload
        // "hello". The computed checksum, placed in the header, must fold
        // the whole thing to zero.
        let src = [10, 0, 0, 1];
        let dst = [10, 0, 0, 2];
        let mut udp = [0u8; 8 + 5];
        udp[0..2].copy_from_slice(&1234u16.to_be_bytes()); // sport
        udp[2..4].copy_from_slice(&5678u16.to_be_bytes()); // dport
        udp[4..6].copy_from_slice(&13u16.to_be_bytes()); // length
        // udp[6..8] stays zeroed: checksum field.
        udp[8..].copy_from_slice(b"hello");
        let c = udp_checksum(src, dst, 13, &udp);
        // Fold: pseudo-header parts + whole datagram incl. checksum == 0.
        let mut sum = sum_u16(&src).wrapping_add(sum_u16(&dst));
        sum = sum.wrapping_add(17).wrapping_add(13);
        sum = sum.wrapping_add(sum_u16(&udp)).wrapping_add(c as u32);
        assert_eq!(checksum_finalize(sum), 0);
    }

    #[test]
    fn sctp_checksum_known_vector() {
        // RFC 4960 example / common test vector for CRC32c of "123456789"
        // is 0xE3069283 (Castagnoli over that ASCII string).
        assert_eq!(sctp_checksum(b"123456789"), 0xE306_9283);
        assert_eq!(sctp_checksum(b""), 0);
    }

    #[test]
    fn crc32c_table_is_stable() {
        // Table is const-computed; spot-check two entries.
        const TABLE: [u32; 256] = crc32c_table();
        assert_eq!(TABLE[0], 0);
        assert_eq!(TABLE[1], 0xF26B_8303);
    }
}
