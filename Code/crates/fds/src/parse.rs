//! Parser atoms (standard \[A\], \[SIMD\], \[SEC\] left factoring,
//! Fast-path specialization): the protocol header parsers are PURE
//! molecules; total functions from validated `&[u8]` inputs to parsed
//! headers; with every length checked before any indexing. The protocol
//! parsers are the paper's worked example (thesis Ch. 13/14).
//!
//! The bare parser functions are the atoms (wrapped by the Mol framework
//! in `templates/`); the concrete `Atom`/`PureAtom` wrapper structs were
//! pruned as dead code (nothing constructs them in the lib; `fuzz` and
//! `af_xdp::process_frame` call the functions directly).

/// Parser failure: an enum of the ways a header can be malformed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ParseError {
    /// Fewer bytes than the header requires.
    Truncated,
    /// A length field (IHL, payload length, ...) is inconsistent.
    BadLength,
    /// Version/next-header field is not what this parser handles.
    BadVersion,
}

/// Parsed IPv4 header (fixed 20-byte form; options are not a fast path).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ipv4Header {
    pub total_len: u16,
    pub identification: u16,
    pub flags_fragment: u16,
    pub ttl: u8,
    pub protocol: u8,
    pub src: [u8; 4],
    pub dst: [u8; 4],
}

/// Parsed IPv6 header (fixed 40-byte form; extension headers are not a
/// fast path: `next_header` is the upper-layer protocol or the first
/// extension).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ipv6Header {
    pub payload_len: u16,
    pub next_header: u8,
    pub hop_limit: u8,
    pub src: [u8; 16],
    pub dst: [u8; 16],
}

/// Parsed UDP header (8 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UdpHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub len: u16,
}

/// Parsed TCP header (fixed 20-byte form; options skipped after the
/// data-offset field).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TcpHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq: u32,
    pub ack: u32,
    pub data_offset: u8,
    pub flags: u8,
    pub window: u16,
    pub checksum: u16,
    pub urgent_ptr: u16,
}

/// Parse an IPv4 header. Requires at least 20 bytes; validates IHL and
/// version; never reads past `input.len()`.
pub(crate) fn parse_ipv4(input: &[u8]) -> Result<Ipv4Header, ParseError> {
    if input.len() < 20 {
        return Err(ParseError::Truncated);
    }
    if input[0] >> 4 != 4 {
        return Err(ParseError::BadVersion);
    }
    let ihl = usize::from(input[0] & 0x0F);
    if ihl < 5 || ihl * 4 > input.len() {
        return Err(ParseError::BadLength);
    }
    Ok(Ipv4Header {
        total_len: u16::from_be_bytes([input[2], input[3]]),
        identification: u16::from_be_bytes([input[4], input[5]]),
        flags_fragment: u16::from_be_bytes([input[6], input[7]]),
        ttl: input[8],
        protocol: input[9],
        src: [input[12], input[13], input[14], input[15]],
        dst: [input[16], input[17], input[18], input[19]],
    })
}

/// Parse an IPv6 header. Requires at least 40 bytes; validates version;
/// never reads past `input.len()`. Extension headers are not walked:
/// `next_header` is returned as on the wire.
pub(crate) fn parse_ipv6(input: &[u8]) -> Result<Ipv6Header, ParseError> {
    if input.len() < 40 {
        return Err(ParseError::Truncated);
    }
    if input[0] >> 4 != 6 {
        return Err(ParseError::BadVersion);
    }
    let mut src = [0u8; 16];
    let mut dst = [0u8; 16];
    src.copy_from_slice(&input[8..24]);
    dst.copy_from_slice(&input[24..40]);
    Ok(Ipv6Header {
        payload_len: u16::from_be_bytes([input[4], input[5]]),
        next_header: input[6],
        hop_limit: input[7],
        src,
        dst,
    })
}

/// Parse a UDP header. Requires at least 8 bytes.
pub(crate) fn parse_udp(input: &[u8]) -> Result<UdpHeader, ParseError> {
    if input.len() < 8 {
        return Err(ParseError::Truncated);
    }
    let len = u16::from_be_bytes([input[4], input[5]]);
    if len < 8 {
        return Err(ParseError::BadLength);
    }
    Ok(UdpHeader {
        src_port: u16::from_be_bytes([input[0], input[1]]),
        dst_port: u16::from_be_bytes([input[2], input[3]]),
        len,
    })
}

/// Parse a TCP header. Requires at least 20 bytes; honors the data
/// offset field for the options region (never past `input.len()`).
pub(crate) fn parse_tcp(input: &[u8]) -> Result<TcpHeader, ParseError> {
    if input.len() < 20 {
        return Err(ParseError::Truncated);
    }
    let data_offset = usize::from(input[12] >> 4);
    if data_offset < 5 || data_offset * 4 > input.len() {
        return Err(ParseError::BadLength);
    }
    // flags are the low 9 bits of the 16-bit field (NS is bit 8); the
    // u8 field holds the eight classic flags, NS is dropped.
    let flags = u16::from_be_bytes([input[12], input[13]]) & 0x01FF;
    Ok(TcpHeader {
        src_port: u16::from_be_bytes([input[0], input[1]]),
        dst_port: u16::from_be_bytes([input[2], input[3]]),
        seq: u32::from_be_bytes([input[4], input[5], input[6], input[7]]),
        ack: u32::from_be_bytes([input[8], input[9], input[10], input[11]]),
        data_offset: data_offset as u8,
        flags: flags as u8,
        window: u16::from_be_bytes([input[14], input[15]]),
        checksum: u16::from_be_bytes([input[16], input[17]]),
        urgent_ptr: u16::from_be_bytes([input[18], input[19]]),
    })
}

#[cfg(test)]
mod tests {
    // CONTRACT: the implementer adds the required tests here.
    use super::*;

    /// A canonical 20-byte IPv4 header (version 4, IHL 5, protocol TCP).
    fn ipv4_header() -> [u8; 20] {
        [
            0x45, 0x00, 0x00, 0x14, // version/IHL, DSCP/ECN, total_len
            0x12, 0x34, // identification
            0x40, 0x00, // flags (DF) + fragment offset
            0x40, 0x06, // ttl, protocol
            0x00, 0x00, // header checksum (ignored by parser)
            192, 168, 1, 1, // src
            10, 0, 0, 1, // dst
        ]
    }

    #[test]
    fn valid_ipv4() {
        let h = parse_ipv4(&ipv4_header()).unwrap();
        assert_eq!(h.total_len, 20);
        assert_eq!(h.identification, 0x1234);
        assert_eq!(h.flags_fragment, 0x4000);
        assert_eq!(h.ttl, 64);
        assert_eq!(h.protocol, 6);
        assert_eq!(h.src, [192, 168, 1, 1]);
        assert_eq!(h.dst, [10, 0, 0, 1]);
    }

    #[test]
    fn ipv4_truncated() {
        let full = ipv4_header();
        for len in 0..20 {
            assert_eq!(parse_ipv4(&full[..len]), Err(ParseError::Truncated));
        }
    }

    #[test]
    fn ipv4_ihl_beyond_input() {
        // IHL = 15 claims a 60-byte header; only 40 bytes are present.
        let mut input = [0u8; 40];
        input[0] = 0x4F;
        assert_eq!(parse_ipv4(&input), Err(ParseError::BadLength));
    }

    #[test]
    fn ipv4_wrong_version() {
        let mut input = ipv4_header();
        input[0] = 0x65; // version 6, IHL 5
        assert_eq!(parse_ipv4(&input), Err(ParseError::BadVersion));
    }

    /// A canonical 40-byte IPv6 header (next-header TCP, hop 64).
    fn ipv6_header() -> [u8; 40] {
        let mut h = [0u8; 40];
        h[0] = 0x60; // version 6
        h[4..6].copy_from_slice(&20u16.to_be_bytes()); // payload_len
        h[6] = 6; // TCP
        h[7] = 64;
        h[8] = 0x20; // src 2001:db8::1
        h[9] = 0x01;
        h[10] = 0x0d;
        h[11] = 0xb8;
        h[23] = 1;
        h[24] = 0x20; // dst 2001:db8::2
        h[25] = 0x01;
        h[26] = 0x0d;
        h[27] = 0xb8;
        h[39] = 2;
        h
    }

    #[test]
    fn valid_ipv6() {
        let h = parse_ipv6(&ipv6_header()).unwrap();
        assert_eq!(h.payload_len, 20);
        assert_eq!(h.next_header, 6);
        assert_eq!(h.hop_limit, 64);
        assert_eq!(h.src[0], 0x20);
        assert_eq!(h.src[15], 1);
        assert_eq!(h.dst[15], 2);
    }

    #[test]
    fn ipv6_truncated() {
        let full = ipv6_header();
        for len in 0..40 {
            assert_eq!(parse_ipv6(&full[..len]), Err(ParseError::Truncated));
        }
    }

    #[test]
    fn ipv6_wrong_version() {
        let mut input = ipv6_header();
        input[0] = 0x40;
        assert_eq!(parse_ipv6(&input), Err(ParseError::BadVersion));
    }

    #[test]
    fn valid_udp() {
        let mut input = [0u8; 8];
        input[0..2].copy_from_slice(&0x1234u16.to_be_bytes());
        input[2..4].copy_from_slice(&0x0035u16.to_be_bytes());
        input[4..6].copy_from_slice(&0x0010u16.to_be_bytes());
        let h = parse_udp(&input).unwrap();
        assert_eq!(h.src_port, 0x1234);
        assert_eq!(h.dst_port, 0x0035);
        assert_eq!(h.len, 16);
    }

    #[test]
    fn udp_truncated() {
        let mut input = [0u8; 8];
        input[4..6].copy_from_slice(&8u16.to_be_bytes());
        for len in 0..8 {
            assert_eq!(parse_udp(&input[..len]), Err(ParseError::Truncated));
        }
    }

    #[test]
    fn udp_bad_length() {
        let mut input = [0u8; 8];
        input[4..6].copy_from_slice(&7u16.to_be_bytes()); // len < 8
        assert_eq!(parse_udp(&input), Err(ParseError::BadLength));
    }

    /// A canonical 20-byte TCP header (data offset 5, flags SYN|ACK).
    fn tcp_header() -> [u8; 20] {
        [
            0x1F, 0x90, // src_port = 8080
            0x00, 0x50, // dst_port = 80
            0x01, 0x02, 0x03, 0x04, // seq
            0x05, 0x06, 0x07, 0x08, // ack
            0x50, 0x12, // data offset 5, flags SYN|ACK
            0x20, 0x00, // window
            0x00, 0x00, // checksum
            0x00, 0x00, // urgent ptr
        ]
    }

    #[test]
    fn tcp_truncated() {
        let full = tcp_header();
        for len in 0..20 {
            assert_eq!(parse_tcp(&full[..len]), Err(ParseError::Truncated));
        }
    }

    #[test]
    fn tcp_data_offset_beyond_input() {
        // Data offset 15 claims a 60-byte header; only 20 are present.
        let mut input = tcp_header();
        input[12] = 0xF0;
        assert_eq!(parse_tcp(&input), Err(ParseError::BadLength));
    }

    #[test]
    fn property_sweep() {
        // Deterministic xorshift64 LCG, no external RNG.
        let mut state: u64 = 0x9E3779B97F4A7C15;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut buf = [0u8; 64];
        for _ in 0..100_000 {
            let len = (next() as usize) % 65; // 0..=64
            for b in &mut buf {
                *b = next() as u8;
            }
            let input = &buf[..len];
            // Parsers must never panic; short inputs always Err(Truncated).
            let ip = parse_ipv4(input);
            if len < 20 {
                assert_eq!(ip, Err(ParseError::Truncated));
            }
            assert_eq!(ip, parse_ipv4(input)); // deterministic
            let ip6 = parse_ipv6(input);
            if len < 40 {
                assert_eq!(ip6, Err(ParseError::Truncated));
            }
            assert_eq!(ip6, parse_ipv6(input));
            let ud = parse_udp(input);
            if len < 8 {
                assert_eq!(ud, Err(ParseError::Truncated));
            }
            assert_eq!(ud, parse_udp(input));
            let tc = parse_tcp(input);
            if len < 20 {
                assert_eq!(tc, Err(ParseError::Truncated));
            }
            assert_eq!(tc, parse_tcp(input));
        }
    }
}
