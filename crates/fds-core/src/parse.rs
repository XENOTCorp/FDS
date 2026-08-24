//! Parser atoms (standard [A], [SIMD], [SEC]; thesis NT10 left factoring,
//! NT28 fast-path specialization): the protocol header parsers are PURE
//! molecules — total functions from validated `&[u8]` inputs to parsed
//! headers — with every length checked before any indexing. The protocol
//! parsers are the paper's worked example (thesis Ch. 13/14).
//!
//! CONTRACT (implementer): each parser is a bounds-safe pure atom. Inputs
//! are raw `&[u8]` (never assume NUL termination); every parser returns
//! `Result<Header, ParseError>` where `ParseError` is a unit-ish enum
//! (no allocation). Parse exactly the fixed-size fields; reject truncated
//! inputs. Add unit tests for: valid header, truncated header (every
//! boundary), oversized length fields, and a deterministic property sweep
//! (seeded LCG, no external RNG).

use mol::{Atom, PureAtom};

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

/// Pure atom: `&[u8]` → [`Ipv4Header`].
pub(crate) struct ParseIpv4;

impl Atom for ParseIpv4 {
    type Input = &'static [u8];
    type Output = Result<Ipv4Header, ParseError>;
}

impl PureAtom for ParseIpv4 {
    fn apply(&self, input: &'static [u8]) -> Result<Ipv4Header, ParseError> {
        parse_ipv4(input)
    }
}

/// Pure atom: `&[u8]` → [`UdpHeader`].
pub(crate) struct ParseUdp;

impl Atom for ParseUdp {
    type Input = &'static [u8];
    type Output = Result<UdpHeader, ParseError>;
}

impl PureAtom for ParseUdp {
    fn apply(&self, input: &'static [u8]) -> Result<UdpHeader, ParseError> {
        parse_udp(input)
    }
}

/// Pure atom: `&[u8]` → [`TcpHeader`].
pub(crate) struct ParseTcp;

impl Atom for ParseTcp {
    type Input = &'static [u8];
    type Output = Result<TcpHeader, ParseError>;
}

impl PureAtom for ParseTcp {
    fn apply(&self, input: &'static [u8]) -> Result<TcpHeader, ParseError> {
        parse_tcp(input)
    }
}

/// Parse an IPv4 header. Requires at least 20 bytes; validates IHL and
/// version; never reads past `input.len()`.
pub(crate) fn parse_ipv4(input: &[u8]) -> Result<Ipv4Header, ParseError> {
    // CONTRACT: implement here (todo!() replaced by the implementer).
    let _ = input;
    todo!("parse_ipv4: implemented by fds-core milestone task")
}

/// Parse a UDP header. Requires at least 8 bytes.
pub(crate) fn parse_udp(input: &[u8]) -> Result<UdpHeader, ParseError> {
    let _ = input;
    todo!("parse_udp: implemented by fds-core milestone task")
}

/// Parse a TCP header. Requires at least 20 bytes; honors the data
/// offset field for the options region (never past `input.len()`).
pub(crate) fn parse_tcp(input: &[u8]) -> Result<TcpHeader, ParseError> {
    let _ = input;
    todo!("parse_tcp: implemented by fds-core milestone task")
}

#[cfg(test)]
mod tests {
    // CONTRACT: the implementer adds the required tests here.
    use super::*;

    #[test]
    fn placeholder() {
        let _ = core::mem::size_of::<ParseError>();
    }
}
