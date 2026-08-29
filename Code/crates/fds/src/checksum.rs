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

/// IPv6 pseudo-header one's-complement sum (RFC 2460): src, dst, 32-bit
/// upper-layer length, next-header. Used by TCP and UDP over IPv6.
fn ipv6_pseudo_sum(src: [u8; 16], dst: [u8; 16], next: u8, len: u32) -> u32 {
    let mut sum = sum_u16(&src);
    sum = sum.wrapping_add(sum_u16(&dst));
    sum = sum.wrapping_add(len >> 16);
    sum = sum.wrapping_add(len & 0xffff);
    sum.wrapping_add(next as u32)
}

/// TCP checksum including the IPv6 pseudo-header (RFC 2460).
pub(crate) fn tcp_checksum_v6(src: [u8; 16], dst: [u8; 16], tcp_len: u32, data: &[u8]) -> u16 {
    let sum = ipv6_pseudo_sum(src, dst, 6, tcp_len);
    checksum_finalize(sum.wrapping_add(sum_u16(data)))
}

/// UDP checksum including the IPv6 pseudo-header. A computed 0 is
/// stored as 0xFFFF (RFC 2460: IPv6 UDP checksums are mandatory).
pub(crate) fn udp_checksum_v6(src: [u8; 16], dst: [u8; 16], udp_len: u32, data: &[u8]) -> u16 {
    let sum = ipv6_pseudo_sum(src, dst, 17, udp_len);
    let c = checksum_finalize(sum.wrapping_add(sum_u16(data)));
    if c == 0 {
        0xffff
    } else {
        c
    }
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
    fn ipv6_udp_checksum_folds_to_zero() {
        let mut src = [0u8; 16];
        let mut dst = [0u8; 16];
        src[15] = 1;
        dst[15] = 2;
        let mut udp = [0u8; 8 + 5];
        udp[0..2].copy_from_slice(&1234u16.to_be_bytes());
        udp[2..4].copy_from_slice(&5678u16.to_be_bytes());
        udp[4..6].copy_from_slice(&13u16.to_be_bytes());
        udp[8..].copy_from_slice(b"hello");
        let c = udp_checksum_v6(src, dst, 13, &udp);
        assert_ne!(c, 0, "IPv6 UDP checksum must not be zero");
        udp[6..8].copy_from_slice(&c.to_be_bytes());
        let mut sum = ipv6_pseudo_sum(src, dst, 17, 13);
        sum = sum.wrapping_add(sum_u16(&udp));
        assert_eq!(checksum_finalize(sum), 0);
    }

    #[test]
    fn ipv6_tcp_checksum_folds_to_zero() {
        let mut src = [0u8; 16];
        let mut dst = [0u8; 16];
        src[15] = 1;
        dst[15] = 2;
        let mut tcp = [0u8; 20];
        tcp[0..2].copy_from_slice(&8080u16.to_be_bytes());
        tcp[2..4].copy_from_slice(&80u16.to_be_bytes());
        tcp[12] = 0x50; // data offset 5
        tcp[13] = 0x02; // SYN
        let c = tcp_checksum_v6(src, dst, 20, &tcp);
        tcp[16..18].copy_from_slice(&c.to_be_bytes());
        let mut sum = ipv6_pseudo_sum(src, dst, 6, 20);
        sum = sum.wrapping_add(sum_u16(&tcp));
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
