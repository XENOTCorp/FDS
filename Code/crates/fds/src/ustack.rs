//! Userspace TCP: 3-way handshake, software TSO, RACK loss detection
//! (RFC 8985), and retransmission. IPv4 and IPv6. Packet-in / packet-out
//! over Ethernet frames so the same code drives a simulated wire in tests
//! and the AF_XDP datapath in the engine.
//!
//! The stack is one connection per instance (a listener accepts the first
//! SYN; a client has one active open). That matches a per-queue AF_XDP
//! worker and keeps the hot path allocation-free after construction.

use crate::checksum::{ip_checksum, tcp_checksum, tcp_checksum_v6};
use crate::parse::{parse_ipv4, parse_ipv6, parse_tcp};
use std::collections::VecDeque;

/// Ethernet header length.
pub const ETH_LEN: usize = 14;
/// IPv4 header length (no options).
pub const IPV4_LEN: usize = 20;
/// IPv6 header length.
pub const IPV6_LEN: usize = 40;
/// TCP header length without options.
pub const TCP_LEN: usize = 20;
/// Default IPv4 MSS (1500 - 20 - 20).
pub const MSS_V4: usize = 1460;
/// Default IPv6 MSS (1500 - 40 - 20).
pub const MSS_V6: usize = 1440;

pub const TH_FIN: u8 = 0x01;
pub const TH_SYN: u8 = 0x02;
pub const TH_RST: u8 = 0x04;
pub const TH_PSH: u8 = 0x08;
pub const TH_ACK: u8 = 0x10;

const OPT_MSS: u8 = 2;
const OPT_SACK_PERM: u8 = 4;
const OPT_SACK: u8 = 5;
const REO_WND_MIN_US: u64 = 1_000;
const RTO_MIN_US: u64 = 200_000;
const SND_CAP: usize = 65535;
const RCV_CAP: usize = 65535;
const MAX_SACK_BLOCKS: usize = 3;

/// IPv4 or IPv6 host address (no port).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HostAddr {
    V4([u8; 4]),
    V6([u8; 16]),
}

impl HostAddr {
    fn is_v4(self) -> bool {
        matches!(self, HostAddr::V4(_))
    }
}

/// TCP state (RFC 793 subset).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TcpState {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
}

/// One in-flight segment (the RACK scoreboard).
#[derive(Clone, Debug)]
struct TxSeg {
    seq: u32,
    data: Vec<u8>,
    xmit_ts_us: u64,
    sacked: bool,
    lost: bool,
}

impl TxSeg {
    fn end(&self) -> u32 {
        self.seq.wrapping_add(self.data.len() as u32)
    }
}

/// RACK state (RFC 8985 §7).
#[derive(Clone, Copy, Debug, Default)]
struct Rack {
    xmit_ts_us: u64,
    end_seq: u32,
    rtt_us: u64,
    min_rtt_us: u64,
}

/// One connection.
struct Pcb {
    state: TcpState,
    remote_mac: [u8; 6],
    remote: HostAddr,
    remote_port: u16,
    snd_una: u32,
    snd_nxt: u32,
    rcv_nxt: u32,
    mss: usize,
    iss: u32,
    irs: u32,
    inflight: VecDeque<TxSeg>,
    tx_app: Vec<u8>,
    rx_app: Vec<u8>,
    rack: Rack,
}

/// Userspace TCP endpoint.
pub struct TcpStack {
    mac: [u8; 6],
    local: HostAddr,
    local_port: u16,
    now_us: u64,
    ip_id: u16,
    listen: bool,
    pcb: Option<Pcb>,
    txq: VecDeque<Vec<u8>>,
}

impl TcpStack {
    /// IPv4 endpoint bound to `port`.
    pub fn new_v4(mac: [u8; 6], ip: [u8; 4], port: u16) -> Self {
        TcpStack {
            mac,
            local: HostAddr::V4(ip),
            local_port: port,
            now_us: 0,
            ip_id: 1,
            listen: false,
            pcb: None,
            txq: VecDeque::new(),
        }
    }

    /// IPv6 endpoint bound to `port`.
    pub fn new_v6(mac: [u8; 6], ip: [u8; 16], port: u16) -> Self {
        TcpStack {
            mac,
            local: HostAddr::V6(ip),
            local_port: port,
            now_us: 0,
            ip_id: 1,
            listen: false,
            pcb: None,
            txq: VecDeque::new(),
        }
    }

    /// Advance the stack clock (microseconds). Tests and the engine
    /// drive time; RACK and RTO read this.
    pub fn set_now(&mut self, us: u64) {
        self.now_us = us;
    }

    /// Current clock.
    pub fn now_us(&self) -> u64 {
        self.now_us
    }

    /// Passive open.
    pub fn listen(&mut self) {
        self.listen = true;
        self.pcb = None;
    }

    /// Active open: queue a SYN to `remote`.
    pub fn connect(&mut self, remote_mac: [u8; 6], remote: HostAddr, remote_port: u16) {
        let iss = 1_000;
        let mss = if remote.is_v4() { MSS_V4 } else { MSS_V6 };
        let pcb = Pcb {
            state: TcpState::SynSent,
            remote_mac,
            remote,
            remote_port,
            snd_una: iss,
            snd_nxt: iss.wrapping_add(1),
            rcv_nxt: 0,
            mss,
            iss,
            irs: 0,
            inflight: VecDeque::new(),
            tx_app: Vec::new(),
            rx_app: Vec::new(),
            rack: Rack::default(),
        };
        self.pcb = Some(pcb);
        self.emit_control(TH_SYN, true);
    }

    /// True when the connection is ESTABLISHED.
    pub fn established(&self) -> bool {
        self.pcb
            .as_ref()
            .is_some_and(|p| p.state == TcpState::Established)
    }

    /// Current state, if any.
    pub fn state(&self) -> Option<TcpState> {
        self.pcb.as_ref().map(|p| p.state)
    }

    /// Ingest one Ethernet frame. Invalid frames are dropped.
    pub fn ingest(&mut self, frame: &[u8]) {
        if frame.len() < ETH_LEN + TCP_LEN + IPV4_LEN {
            return;
        }
        let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
        match ethertype {
            0x0800 => self.ingest_v4(frame),
            0x86dd => self.ingest_v6(frame),
            _ => {}
        }
    }

    /// Pop the next outbound Ethernet frame, if any.
    pub fn pop_tx(&mut self) -> Option<Vec<u8>> {
        self.txq.pop_front()
    }

    /// Queue application bytes for the established connection. Returns
    /// the number accepted (bounded by the send buffer). TSO chops at
    /// MSS on the way out.
    pub fn write(&mut self, data: &[u8]) -> usize {
        let Some(pcb) = self.pcb.as_mut() else {
            return 0;
        };
        if pcb.state != TcpState::Established {
            return 0;
        }
        let room = SND_CAP.saturating_sub(pcb.tx_app.len());
        let n = data.len().min(room);
        pcb.tx_app.extend_from_slice(&data[..n]);
        self.flush_tx();
        n
    }

    /// Copy received application bytes into `buf`. Returns the count.
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let Some(pcb) = self.pcb.as_mut() else {
            return 0;
        };
        let n = buf.len().min(pcb.rx_app.len());
        buf[..n].copy_from_slice(&pcb.rx_app[..n]);
        pcb.rx_app.drain(..n);
        n
    }

    /// Run RACK loss detection and retransmit lost or timed-out segments.
    pub fn on_timer(&mut self, now_us: u64) {
        self.now_us = now_us;
        self.rack_detect();
        self.retransmit_lost();
    }

    fn ingest_v4(&mut self, frame: &[u8]) {
        let ip = match parse_ipv4(&frame[ETH_LEN..]) {
            Ok(h) => h,
            Err(_) => return,
        };
        if ip.protocol != 6 {
            return;
        }
        if self.listen && self.pcb.is_none() {
            self.mac.copy_from_slice(&frame[0..6]);
            self.local = HostAddr::V4(ip.dst);
        }
        let ihl = 20;
        let tcp_off = ETH_LEN + ihl;
        if frame.len() < tcp_off + TCP_LEN {
            return;
        }
        let ip_end = ETH_LEN + ip.total_len as usize;
        if ip_end > frame.len() || ip_end < tcp_off {
            return;
        }
        self.ingest_tcp(
            HostAddr::V4(ip.src),
            &frame[6..12],
            &frame[tcp_off..ip_end],
        );
    }

    fn ingest_v6(&mut self, frame: &[u8]) {
        let ip = match parse_ipv6(&frame[ETH_LEN..]) {
            Ok(h) => h,
            Err(_) => return,
        };
        if ip.next_header != 6 {
            return;
        }
        if self.listen && self.pcb.is_none() {
            self.mac.copy_from_slice(&frame[0..6]);
            self.local = HostAddr::V6(ip.dst);
        }
        let tcp_off = ETH_LEN + IPV6_LEN;
        let ip_end = tcp_off + ip.payload_len as usize;
        if ip_end > frame.len() || ip_end < tcp_off + TCP_LEN {
            return;
        }
        self.ingest_tcp(
            HostAddr::V6(ip.src),
            &frame[6..12],
            &frame[tcp_off..ip_end],
        );
    }

    fn ingest_tcp(&mut self, src: HostAddr, src_mac: &[u8], seg: &[u8]) {
        let hdr = match parse_tcp(seg) {
            Ok(h) => h,
            Err(_) => return,
        };
        if hdr.dst_port != self.local_port {
            return;
        }
        let doff = hdr.data_offset as usize * 4;
        if doff > seg.len() {
            return;
        }
        let payload = &seg[doff..];
        let sacks = parse_sack(&seg[TCP_LEN..doff]);
        let syn = hdr.flags & TH_SYN != 0;
        let ack = hdr.flags & TH_ACK != 0;

        if self.pcb.is_none() {
            if self.listen && syn && !ack {
                let mss = if src.is_v4() { MSS_V4 } else { MSS_V6 };
                let iss = 2_000;
                let mut remote_mac = [0u8; 6];
                remote_mac.copy_from_slice(src_mac);
                self.pcb = Some(Pcb {
                    state: TcpState::SynReceived,
                    remote_mac,
                    remote: src,
                    remote_port: hdr.src_port,
                    snd_una: iss,
                    snd_nxt: iss.wrapping_add(1),
                    rcv_nxt: hdr.seq.wrapping_add(1),
                    mss,
                    iss,
                    irs: hdr.seq,
                    inflight: VecDeque::new(),
                    tx_app: Vec::new(),
                    rx_app: Vec::new(),
                    rack: Rack::default(),
                });
                self.emit_control(TH_SYN | TH_ACK, true);
            }
            return;
        }

        let syn_ack = syn && ack;
        {
            let pcb = self.pcb.as_mut().unwrap();
            if pcb.remote != src || pcb.remote_port != hdr.src_port {
                return;
            }
            match pcb.state {
                TcpState::SynSent if syn_ack => {
                    pcb.irs = hdr.seq;
                    pcb.rcv_nxt = hdr.seq.wrapping_add(1);
                    if seq_geq(hdr.ack, pcb.iss.wrapping_add(1)) {
                        pcb.snd_una = hdr.ack;
                        pcb.state = TcpState::Established;
                    }
                }
                TcpState::SynReceived if ack => {
                    if seq_geq(hdr.ack, pcb.iss.wrapping_add(1)) {
                        pcb.snd_una = hdr.ack;
                        pcb.state = TcpState::Established;
                    }
                }
                TcpState::Established => {}
                _ => return,
            }
        }
        if syn_ack && self.pcb.as_ref().unwrap().state == TcpState::Established {
            self.emit_control(TH_ACK, false);
        }
        if self.pcb.as_ref().unwrap().state != TcpState::Established {
            return;
        }
        self.on_ack(hdr.ack, &sacks);
        let in_order = !payload.is_empty()
            && self
                .pcb
                .as_ref()
                .is_some_and(|p| hdr.seq == p.rcv_nxt);
        if in_order {
            {
                let pcb = self.pcb.as_mut().unwrap();
                let room = RCV_CAP.saturating_sub(pcb.rx_app.len());
                let n = payload.len().min(room);
                pcb.rx_app.extend_from_slice(&payload[..n]);
                pcb.rcv_nxt = pcb.rcv_nxt.wrapping_add(n as u32);
            }
            self.emit_control(TH_ACK, false);
        }
        self.flush_tx();
    }

    fn on_ack(&mut self, ack: u32, sacks: &[(u32, u32)]) {
        let now = self.now_us;
        {
            let pcb = self.pcb.as_mut().unwrap();
            if seq_gt(ack, pcb.snd_nxt) {
                return; // invalid ACK
            }
            if seq_gt(ack, pcb.snd_una) {
                pcb.snd_una = ack;
            }
            while pcb.inflight.front().is_some_and(|s| seq_leq(s.end(), ack)) {
                let seg = pcb.inflight.pop_front().unwrap();
                rack_update(&mut pcb.rack, now, &seg);
            }
            for &(left, right) in sacks {
                for seg in pcb.inflight.iter_mut() {
                    if seq_geq(seg.seq, left) && seq_leq(seg.end(), right) && !seg.sacked {
                        seg.sacked = true;
                        rack_update(&mut pcb.rack, now, seg);
                    }
                }
            }
        }
        self.rack_detect();
        self.retransmit_lost();
    }

    fn rack_detect(&mut self) {
        let now = self.now_us;
        let Some(pcb) = self.pcb.as_mut() else {
            return;
        };
        let reo = rack_reo_wnd(pcb.rack.min_rtt_us);
        if pcb.rack.rtt_us > 0 {
            for seg in pcb.inflight.iter_mut() {
                if seg.sacked || seq_geq(seg.end(), pcb.rack.end_seq) {
                    continue;
                }
                if now.saturating_sub(seg.xmit_ts_us) >= pcb.rack.rtt_us.saturating_add(reo) {
                    seg.lost = true;
                }
            }
        }
        // RTO: a segment that has waited RTO_MIN without an ACK is lost.
        let rto = pcb.rack.rtt_us.saturating_mul(2).max(RTO_MIN_US);
        for seg in pcb.inflight.iter_mut() {
            if now.saturating_sub(seg.xmit_ts_us) >= rto {
                seg.lost = true;
            }
        }
    }

    fn retransmit_lost(&mut self) {
        let now = self.now_us;
        let (remote_mac, remote, remote_port, rcv_nxt, lost) = {
            let Some(pcb) = self.pcb.as_mut() else {
                return;
            };
            let mut lost: Vec<(u32, Vec<u8>)> = Vec::new();
            for seg in pcb.inflight.iter_mut() {
                if seg.lost {
                    seg.lost = false;
                    seg.sacked = false;
                    seg.xmit_ts_us = now;
                    lost.push((seg.seq, seg.data.clone()));
                }
            }
            (
                pcb.remote_mac,
                pcb.remote,
                pcb.remote_port,
                pcb.rcv_nxt,
                lost,
            )
        };
        for (seq, data) in lost {
            let frame = self.build_data(remote_mac, remote, remote_port, seq, rcv_nxt, &data);
            self.txq.push_back(frame);
        }
    }

    fn flush_tx(&mut self) {
        let now = self.now_us;
        let (remote_mac, remote, remote_port, rcv_nxt, chunks) = {
            let Some(pcb) = self.pcb.as_mut() else {
                return;
            };
            if pcb.state != TcpState::Established || pcb.tx_app.is_empty() {
                return;
            }
            let mss = pcb.mss;
            let chunks: Vec<(u32, Vec<u8>)> = tso_chop(mss, pcb.snd_nxt, &pcb.tx_app)
                .into_iter()
                .map(|(seq, sl)| (seq, sl.to_vec()))
                .collect();
            pcb.tx_app.clear();
            for (seq, data) in &chunks {
                pcb.snd_nxt = seq.wrapping_add(data.len() as u32);
                pcb.inflight.push_back(TxSeg {
                    seq: *seq,
                    data: data.clone(),
                    xmit_ts_us: now,
                    sacked: false,
                    lost: false,
                });
            }
            (
                pcb.remote_mac,
                pcb.remote,
                pcb.remote_port,
                pcb.rcv_nxt,
                chunks,
            )
        };
        for (seq, data) in chunks {
            let frame = self.build_data(remote_mac, remote, remote_port, seq, rcv_nxt, &data);
            self.txq.push_back(frame);
        }
    }

    fn emit_control(&mut self, flags: u8, with_mss: bool) {
        let (remote_mac, remote, remote_port, seq, ack) = {
            let Some(pcb) = self.pcb.as_ref() else {
                return;
            };
            let seq = if flags & TH_SYN != 0 {
                pcb.iss
            } else {
                pcb.snd_nxt
            };
            (
                pcb.remote_mac,
                pcb.remote,
                pcb.remote_port,
                seq,
                pcb.rcv_nxt,
            )
        };
        let frame = self.build_ctrl(remote_mac, remote, remote_port, seq, ack, flags, with_mss);
        self.txq.push_back(frame);
    }

    #[allow(clippy::too_many_arguments)]
    fn build_ctrl(
        &mut self,
        dst_mac: [u8; 6],
        dst: HostAddr,
        dst_port: u16,
        seq: u32,
        ack: u32,
        flags: u8,
        with_mss: bool,
    ) -> Vec<u8> {
        let mut opts = Vec::new();
        if with_mss {
            let mss = if dst.is_v4() { MSS_V4 } else { MSS_V6 };
            opts.extend_from_slice(&[OPT_MSS, 4, (mss >> 8) as u8, mss as u8]);
            opts.extend_from_slice(&[OPT_SACK_PERM, 2]);
            while opts.len() % 4 != 0 {
                opts.push(1); // NOP
            }
        }
        self.build_segment(dst_mac, dst, dst_port, seq, ack, flags, &opts, &[])
    }

    fn build_data(
        &mut self,
        dst_mac: [u8; 6],
        dst: HostAddr,
        dst_port: u16,
        seq: u32,
        ack: u32,
        payload: &[u8],
    ) -> Vec<u8> {
        self.build_segment(dst_mac, dst, dst_port, seq, ack, TH_ACK | TH_PSH, &[], payload)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_segment(
        &mut self,
        dst_mac: [u8; 6],
        dst: HostAddr,
        dst_port: u16,
        seq: u32,
        ack: u32,
        flags: u8,
        opts: &[u8],
        payload: &[u8],
    ) -> Vec<u8> {
        let tcp_hdr_len = TCP_LEN + opts.len();
        let tcp_len = tcp_hdr_len + payload.len();
        let mut tcp = vec![0u8; tcp_len];
        tcp[0..2].copy_from_slice(&self.local_port.to_be_bytes());
        tcp[2..4].copy_from_slice(&dst_port.to_be_bytes());
        tcp[4..8].copy_from_slice(&seq.to_be_bytes());
        tcp[8..12].copy_from_slice(&ack.to_be_bytes());
        tcp[12] = ((tcp_hdr_len / 4) as u8) << 4;
        tcp[13] = flags;
        tcp[14..16].copy_from_slice(&(RCV_CAP as u16).to_be_bytes());
        if !opts.is_empty() {
            tcp[TCP_LEN..TCP_LEN + opts.len()].copy_from_slice(opts);
        }
        if !payload.is_empty() {
            tcp[tcp_hdr_len..].copy_from_slice(payload);
        }
        match (self.local, dst) {
            (HostAddr::V4(src), HostAddr::V4(dst_ip)) => {
                let c = tcp_checksum(src, dst_ip, tcp_len as u16, &tcp);
                tcp[16..18].copy_from_slice(&c.to_be_bytes());
                self.wrap_v4(dst_mac, dst_ip, &tcp)
            }
            (HostAddr::V6(src), HostAddr::V6(dst_ip)) => {
                let c = tcp_checksum_v6(src, dst_ip, tcp_len as u32, &tcp);
                tcp[16..18].copy_from_slice(&c.to_be_bytes());
                self.wrap_v6(dst_mac, dst_ip, &tcp)
            }
            _ => Vec::new(),
        }
    }

    fn wrap_v4(&mut self, dst_mac: [u8; 6], dst: [u8; 4], tcp: &[u8]) -> Vec<u8> {
        let total = IPV4_LEN + tcp.len();
        let mut f = vec![0u8; ETH_LEN + total];
        f[0..6].copy_from_slice(&dst_mac);
        f[6..12].copy_from_slice(&self.mac);
        f[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        let ip = &mut f[ETH_LEN..ETH_LEN + IPV4_LEN];
        ip[0] = 0x45;
        ip[2..4].copy_from_slice(&(total as u16).to_be_bytes());
        ip[4..6].copy_from_slice(&self.ip_id.to_be_bytes());
        self.ip_id = self.ip_id.wrapping_add(1);
        ip[8] = 64;
        ip[9] = 6;
        if let HostAddr::V4(src) = self.local {
            ip[12..16].copy_from_slice(&src);
        }
        ip[16..20].copy_from_slice(&dst);
        let c = ip_checksum(ip);
        ip[10..12].copy_from_slice(&c.to_be_bytes());
        f[ETH_LEN + IPV4_LEN..].copy_from_slice(tcp);
        f
    }

    fn wrap_v6(&mut self, dst_mac: [u8; 6], dst: [u8; 16], tcp: &[u8]) -> Vec<u8> {
        let mut f = vec![0u8; ETH_LEN + IPV6_LEN + tcp.len()];
        f[0..6].copy_from_slice(&dst_mac);
        f[6..12].copy_from_slice(&self.mac);
        f[12..14].copy_from_slice(&0x86ddu16.to_be_bytes());
        let ip = &mut f[ETH_LEN..ETH_LEN + IPV6_LEN];
        ip[0] = 0x60;
        ip[4..6].copy_from_slice(&(tcp.len() as u16).to_be_bytes());
        ip[6] = 6;
        ip[7] = 64;
        if let HostAddr::V6(src) = self.local {
            ip[8..24].copy_from_slice(&src);
        }
        ip[24..40].copy_from_slice(&dst);
        f[ETH_LEN + IPV6_LEN..].copy_from_slice(tcp);
        f
    }
}

/// Software TSO: chop `data` into MSS-sized pieces starting at `seq`.
pub fn tso_chop(mss: usize, seq: u32, data: &[u8]) -> Vec<(u32, &[u8])> {
    let mss = mss.max(1);
    let mut out = Vec::new();
    let mut off = 0;
    let mut s = seq;
    while off < data.len() {
        let n = (data.len() - off).min(mss);
        out.push((s, &data[off..off + n]));
        s = s.wrapping_add(n as u32);
        off += n;
    }
    out
}

/// Sequence comparison (RFC 1982 serial numbers).
pub fn seq_lt(a: u32, b: u32) -> bool {
    a.wrapping_sub(b) > 0x7fff_ffff && a != b
}

pub fn seq_leq(a: u32, b: u32) -> bool {
    a == b || seq_lt(a, b)
}

pub fn seq_gt(a: u32, b: u32) -> bool {
    seq_lt(b, a)
}

pub fn seq_geq(a: u32, b: u32) -> bool {
    a == b || seq_gt(a, b)
}

fn rack_reo_wnd(min_rtt_us: u64) -> u64 {
    (min_rtt_us / 4).max(REO_WND_MIN_US)
}

fn rack_update(rack: &mut Rack, now: u64, seg: &TxSeg) {
    let rtt = now.saturating_sub(seg.xmit_ts_us);
    if rtt > 0 {
        rack.rtt_us = rtt;
        if rack.min_rtt_us == 0 || rtt < rack.min_rtt_us {
            rack.min_rtt_us = rtt;
        }
    }
    if seq_gt(seg.end(), rack.end_seq) || rack.xmit_ts_us == 0 {
        rack.xmit_ts_us = seg.xmit_ts_us;
        rack.end_seq = seg.end();
    }
}

fn parse_sack(opts: &[u8]) -> Vec<(u32, u32)> {
    let mut i = 0;
    let mut out = Vec::new();
    while i < opts.len() {
        let kind = opts[i];
        if kind == 0 {
            break;
        }
        if kind == 1 {
            i += 1;
            continue;
        }
        if i + 1 >= opts.len() {
            break;
        }
        let len = opts[i + 1] as usize;
        if len < 2 || i + len > opts.len() {
            break;
        }
        if kind == OPT_SACK && len >= 10 {
            let mut o = i + 2;
            while o + 8 <= i + len && out.len() < MAX_SACK_BLOCKS {
                let left = u32::from_be_bytes(opts[o..o + 4].try_into().unwrap());
                let right = u32::from_be_bytes(opts[o + 4..o + 8].try_into().unwrap());
                out.push((left, right));
                o += 8;
            }
        }
        i += len;
    }
    out
}

/// Deliver every queued frame from `src` to `dst`, optionally dropping
/// data segments whose first payload byte matches `drop`. Control
/// (SYN/ACK-only) packets are never dropped so the handshake stays
/// reliable in tests; data loss is the RACK case.
pub fn pump(src: &mut TcpStack, dst: &mut TcpStack, mut drop: impl FnMut(&[u8]) -> bool) {
    let mut frames = Vec::new();
    while let Some(f) = src.pop_tx() {
        frames.push(f);
    }
    for f in frames {
        if drop(&f) {
            continue;
        }
        dst.ingest(&f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac(n: u8) -> [u8; 6] {
        [n, n, n, n, n, n]
    }

    fn handshake_v4() -> (TcpStack, TcpStack) {
        let mut srv = TcpStack::new_v4(mac(1), [10, 0, 0, 1], 80);
        let mut cli = TcpStack::new_v4(mac(2), [10, 0, 0, 2], 12345);
        srv.listen();
        cli.connect(mac(1), HostAddr::V4([10, 0, 0, 1]), 80);
        pump(&mut cli, &mut srv, |_| false);
        pump(&mut srv, &mut cli, |_| false);
        pump(&mut cli, &mut srv, |_| false);
        assert!(cli.established(), "client not established");
        assert!(srv.established(), "server not established");
        (srv, cli)
    }

    #[test]
    fn tso_chops_at_mss() {
        let data = vec![0xABu8; 4000];
        let segs = tso_chop(1460, 100, &data);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].0, 100);
        assert_eq!(segs[0].1.len(), 1460);
        assert_eq!(segs[1].0, 100 + 1460);
        assert_eq!(segs[1].1.len(), 1460);
        assert_eq!(segs[2].0, 100 + 2920);
        assert_eq!(segs[2].1.len(), 4000 - 2920);
        assert_eq!(tso_chop(1460, 0, &[]).len(), 0);
    }

    #[test]
    fn seq_wraps() {
        assert!(seq_lt(0xffff_fff0, 16));
        assert!(seq_gt(16, 0xffff_fff0));
        assert!(seq_leq(5, 5));
    }

    #[test]
    fn handshake_and_echo_v4() {
        let (mut srv, mut cli) = handshake_v4();
        let payload = b"hello rack";
        assert_eq!(cli.write(payload), payload.len());
        pump(&mut cli, &mut srv, |_| false);
        pump(&mut srv, &mut cli, |_| false);
        let mut buf = [0u8; 32];
        let n = srv.read(&mut buf);
        assert_eq!(&buf[..n], payload);
        assert_eq!(srv.write(&buf[..n]), n);
        pump(&mut srv, &mut cli, |_| false);
        pump(&mut cli, &mut srv, |_| false);
        let n = cli.read(&mut buf);
        assert_eq!(&buf[..n], payload);
    }

    #[test]
    fn handshake_and_echo_v6() {
        let mut src = [0u8; 16];
        let mut dst = [0u8; 16];
        src[15] = 1;
        dst[15] = 2;
        let mut srv = TcpStack::new_v6(mac(1), src, 80);
        let mut cli = TcpStack::new_v6(mac(2), dst, 12345);
        srv.listen();
        cli.connect(mac(1), HostAddr::V6(src), 80);
        pump(&mut cli, &mut srv, |_| false);
        pump(&mut srv, &mut cli, |_| false);
        pump(&mut cli, &mut srv, |_| false);
        assert!(cli.established() && srv.established());
        assert_eq!(cli.write(b"v6"), 2);
        pump(&mut cli, &mut srv, |_| false);
        pump(&mut srv, &mut cli, |_| false);
        let mut buf = [0u8; 8];
        let n = srv.read(&mut buf);
        assert_eq!(&buf[..n], b"v6");
    }

    #[test]
    fn tso_write_is_mss_segments() {
        let (mut srv, mut cli) = handshake_v4();
        let big = vec![0x5Au8; 4000];
        assert_eq!(cli.write(&big), 4000);
        let mut nframes = 0;
        while let Some(f) = cli.pop_tx() {
            nframes += 1;
            srv.ingest(&f);
        }
        assert!(nframes >= 3, "TSO must emit several MSS segments, got {nframes}");
        pump(&mut srv, &mut cli, |_| false);
        let mut buf = vec![0u8; 4096];
        let n = srv.read(&mut buf);
        assert_eq!(n, 4000);
        assert!(buf[..n].iter().all(|&b| b == 0x5A));
    }

    /// Drop the first data segment. RACK + RTO retransmit it. The
    /// payload is still delivered.
    #[test]
    fn rack_recovers_from_loss() {
        let (mut srv, mut cli) = handshake_v4();
        let payload = vec![0x11u8; 2000]; // two MSS-ish chunks (1460+540)
        assert_eq!(cli.write(&payload), payload.len());
        let mut dropped = 0usize;
        let mut data_seen = 0usize;
        // Drop the first data-bearing frame only.
        let mut frames = Vec::new();
        while let Some(f) = cli.pop_tx() {
            frames.push(f);
        }
        for f in &frames {
            let is_data = f.len() > ETH_LEN + IPV4_LEN + TCP_LEN + 20;
            if is_data {
                data_seen += 1;
                if data_seen == 1 {
                    dropped += 1;
                    continue;
                }
            }
            srv.ingest(f);
        }
        assert_eq!(dropped, 1);
        pump(&mut srv, &mut cli, |_| false);
        // Advance time past RTO so the lost segment retransmits.
        let t = cli.now_us() + RTO_MIN_US + 1;
        cli.on_timer(t);
        pump(&mut cli, &mut srv, |_| false);
        pump(&mut srv, &mut cli, |_| false);
        let mut buf = vec![0u8; 4096];
        let n = srv.read(&mut buf);
        assert_eq!(n, payload.len(), "RACK/RTO must repair the hole");
        assert_eq!(&buf[..n], payload.as_slice());
    }
}
