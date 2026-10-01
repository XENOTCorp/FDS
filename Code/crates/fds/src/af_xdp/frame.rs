//! Socket-scoped checkout tokens prevent stale or cross-socket UMEM access.
use std::sync::atomic::{AtomicU64, Ordering};

/// A checked-out UMEM frame. Copies identify the same checkout, not new
/// ownership: after transmit/drop, every copy becomes invalid. The socket
/// validates its identity and generation before touching frame memory.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub(super) addr: u64,
    pub(super) len: u32,
    owner: u64,
    lease: u64,
}

impl Frame {
    pub fn len(&self) -> usize {
        self.len as usize
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn addr(&self) -> u64 {
        self.addr
    }
}

pub(super) struct FrameRegistry {
    owner: u64,
    frame_size: u32,
    leases: Box<[u64]>,
    next_lease: u64,
}

impl FrameRegistry {
    pub(super) fn new(count: u32, frame_size: u32) -> Self {
        static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);
        let owner = NEXT_OWNER
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .expect("AF_XDP socket identities exhausted");
        Self {
            owner,
            frame_size,
            leases: vec![0; count as usize].into_boxed_slice(),
            next_lease: 1,
        }
    }

    fn index(&self, addr: u64, len: u32) -> usize {
        let index = addr / u64::from(self.frame_size);
        assert!(
            index < self.leases.len() as u64,
            "af_xdp: frame outside umem"
        );
        let within = addr % u64::from(self.frame_size);
        assert!(
            within + u64::from(len) <= u64::from(self.frame_size),
            "af_xdp: frame crosses chunk boundary"
        );
        index as usize
    }

    pub(super) fn checkout(&mut self, addr: u64, len: u32) -> Frame {
        let index = self.index(addr, len);
        assert_eq!(self.leases[index], 0, "af_xdp: frame already checked out");
        let lease = self.next_lease;
        self.next_lease = lease.checked_add(1).expect("AF_XDP frame leases exhausted");
        self.leases[index] = lease;
        Frame {
            addr,
            len,
            owner: self.owner,
            lease,
        }
    }

    pub(super) fn validate(&self, frame: &Frame) {
        assert_eq!(
            frame.owner, self.owner,
            "af_xdp: frame belongs to another socket"
        );
        let index = self.index(frame.addr, frame.len);
        assert_eq!(
            self.leases[index], frame.lease,
            "af_xdp: stale frame checkout"
        );
    }

    pub(super) fn release(&mut self, frame: &Frame) {
        self.validate(frame);
        let index = self.index(frame.addr, frame.len);
        self.leases[index] = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headroom_and_zero_length_are_valid() {
        let mut frames = FrameRegistry::new(2, 4096);
        let frame = frames.checkout(256, 1500);
        frames.validate(&frame);
        frames.release(&frame);
        let frame = frames.checkout(4096, 0);
        assert!(frame.is_empty());
        frames.release(&frame);
    }

    #[test]
    #[should_panic(expected = "another socket")]
    fn cross_socket_handles_are_rejected() {
        let mut a = FrameRegistry::new(2, 4096);
        let b = FrameRegistry::new(2, 4096);
        b.validate(&a.checkout(0, 64));
    }

    #[test]
    #[should_panic(expected = "stale frame")]
    fn stale_copies_cannot_access_reissued_frames() {
        let mut frames = FrameRegistry::new(2, 4096);
        let old = frames.checkout(0, 64);
        frames.release(&old);
        let _new = frames.checkout(0, 64);
        frames.validate(&old);
    }

    #[test]
    #[should_panic(expected = "stale frame")]
    fn duplicate_returns_are_rejected() {
        let mut frames = FrameRegistry::new(2, 4096);
        let frame = frames.checkout(0, 64);
        frames.release(&frame);
        frames.release(&frame);
    }

    #[test]
    #[should_panic(expected = "chunk boundary")]
    fn crossing_a_chunk_is_rejected() {
        FrameRegistry::new(2, 4096).checkout(4090, 100);
    }
}
