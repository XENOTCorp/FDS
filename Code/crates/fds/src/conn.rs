//! Connection/association state (standard \[CACHE\]): hot
//! fields (touched every step) and cold fields (rarely touched) live in
//! separate cache lines; per-core tables are preallocated at startup
//! (standard \[ALLOC\]) and index by a packed [`ConnectionId`].

use mol::{CachePadded, Pool, PoolGuard};
use std::net::SocketAddr;

/// A packed connection id: core in the high 32 bits, slot index in the
/// low 32 bits. Cheap to encode/decode, safe to use as an epoll token.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ConnectionId(u64);

impl ConnectionId {
    #[inline]
    pub const fn new(core: u32, slot: u32) -> Self {
        ConnectionId(((core as u64) << 32) | slot as u64)
    }

    #[inline]
    pub const fn core(&self) -> u32 {
        (self.0 >> 32) as u32
    }

    #[inline]
    pub const fn slot(&self) -> u32 {
        self.0 as u32
    }

    #[inline]
    pub const fn as_u64(&self) -> u64 {
        self.0
    }

    /// Rebuild from an epoll token (the u64 the reactor stores).
    #[inline]
    pub const fn from_u64(token: u64) -> Self {
        ConnectionId(token)
    }
}

/// HOT connection fields: read/written on every step. Own cache line.
#[repr(align(64))]
#[derive(Clone, Copy, Debug, Default)]
pub struct HotState {
    /// Protocol sequence number / stream offset (TCP seq, SCTP TSN, ...).
    pub seq: u32,
    /// Last activity in coarse monotonic ticks (seconds since start).
    pub last_activity: u64,
    /// Bytes in flight (send window / receive pressure).
    pub in_flight: u32,
    /// Transport fd. Touched on every I/O, so it lives on the hot line.
    pub fd: i32,
}

/// COLD connection fields: touched rarely (setup, teardown, peer info).
/// Own cache line.
#[repr(align(64))]
#[derive(Clone, Copy, Debug)]
pub struct ColdState {
    pub peer: SocketAddr,
    pub established_at: u64,
    /// Protocol-specific cold flags (e.g. TCP_MD5 enabled).
    pub flags: u32,
}

/// A connection: hot and cold halves, each alone on a cache line.
pub struct Connection {
    pub hot: CachePadded<HotState>,
    pub cold: CachePadded<ColdState>,
}

/// Connection table capacity per worker (preallocated slots). Shared by
/// the epoll and io_uring datapaths.
pub const CONN_CAP: usize = 1024;

impl Connection {
    pub fn new(peer: SocketAddr, established_at: u64) -> Self {
        Connection {
            hot: CachePadded::new(HotState {
                seq: 0,
                last_activity: established_at,
                in_flight: 0,
                fd: -1,
            }),
            cold: CachePadded::new(ColdState {
                peer,
                established_at,
                flags: 0,
            }),
        }
    }
}

/// A per-core, preallocated connection table: `CAP` slots, free indices
/// in a lock-free MPMC ring, so acquire/release are non-blocking and
/// allocation-free in the hot path. `Sync` when `T: Send` (slot access is
/// mediated by the free ring; each slot is owned by exactly one guard).
pub struct ConnTable<const CAP: usize> {
    pool: Pool<Connection, CAP>,
    /// Per-slot flags used by transports (e.g. closed/ready bits).
    /// Rarely touched, so plain relaxed atomics are fine.
    flags: [std::sync::atomic::AtomicU8; CAP],
}

unsafe impl<const CAP: usize> Sync for ConnTable<CAP> {}

impl<const CAP: usize> ConnTable<CAP> {
    /// A new table; the caller initializes every slot (see
    /// [`ConnTable::initialize`]) before sharing it.
    pub fn new() -> Self {
        ConnTable {
            pool: Pool::new(),
            flags: std::array::from_fn(|_| std::sync::atomic::AtomicU8::new(0)),
        }
    }

    /// Initialize slot `i` (call once per slot before sharing).
    pub fn initialize(&self, i: usize, conn: Connection) {
        self.pool.initialize(i, conn);
    }

    /// Acquire a free slot, or `None` when the table is full.
    pub fn try_acquire(&self) -> Option<ConnectionSlot<'_, CAP>> {
        let guard = self.pool.try_alloc()?;
        Some(ConnectionSlot { guard })
    }

    /// Acquire a slot index without a guard: the caller owns the slot
    /// and MUST call [`ConnTable::release_slot`] exactly once (used by
    /// completion-driven datapaths where the slot outlives the accept
    /// frame and a guard would release it early).
    pub fn acquire_index(&self) -> Option<usize> {
        self.pool.try_alloc_index()
    }

    /// Number of slots in use.
    pub fn in_use(&self) -> usize {
        self.pool.in_use()
    }

    /// Release a slot back to the free list (the caller must own it,
    /// e.g. after closing a connection). The slot's data stays in place
    /// for the next owner.
    pub fn release_slot(&self, slot: usize) {
        self.pool.release_index(slot);
    }

    /// Mutable access to an owned slot's connection (the caller must own
    /// the slot; e.g. the reactor holds it for a live connection).
    pub fn conn_mut(&self, slot: usize) -> &mut Connection {
        self.pool.get_mut(slot)
    }

    /// Capacity.
    pub const fn capacity() -> usize {
        CAP
    }

    /// Set a transport flag on a slot (relaxed; call from the owning core).
    pub fn set_flag(&self, slot: usize, bit: u8, on: bool) {
        use std::sync::atomic::Ordering;
        let f = &self.flags[slot];
        if on {
            f.fetch_or(bit, Ordering::Relaxed);
        } else {
            f.fetch_and(!bit, Ordering::Relaxed);
        }
    }

    /// Read a transport flag.
    pub fn flag(&self, slot: usize, bit: u8) -> bool {
        use std::sync::atomic::Ordering;
        self.flags[slot].load(Ordering::Relaxed) & bit != 0
    }
}

impl<const CAP: usize> Default for ConnTable<CAP> {
    fn default() -> Self {
        Self::new()
    }
}

/// An acquired table slot; returns the slot to the free list on drop.
pub struct ConnectionSlot<'a, const CAP: usize> {
    guard: PoolGuard<'a, Connection, CAP>,
}

impl<'a, const CAP: usize> ConnectionSlot<'a, CAP> {
    /// The slot index (low 32 bits of the [`ConnectionId`]).
    pub fn index(&self) -> usize {
        self.guard.index()
    }

    pub fn conn(&self) -> &Connection {
        &self.guard
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.guard
    }
}

/// Assert the layout discipline: hot and cold never share a cache line,
/// and a table of `CAP` slots does not surprise with hidden padding.
#[cfg(test)]
mod tests {
    use super::*;
    use static_assertions::{const_assert, const_assert_eq};

    const_assert_eq!(core::mem::align_of::<HotState>(), 64);
    const_assert_eq!(core::mem::size_of::<HotState>(), 64);
    const_assert!(core::mem::offset_of!(HotState, fd) < 64);
    const_assert_eq!(core::mem::align_of::<ColdState>(), 64);
    const_assert_eq!(core::mem::size_of::<ColdState>(), 64);
    const_assert_eq!(core::mem::align_of::<CachePadded<HotState>>(), 64);
    const_assert!(core::mem::size_of::<ConnectionId>() == 8);

    #[test]
    fn id_packs_core_and_slot() {
        let id = ConnectionId::new(3, 42);
        assert_eq!(id.core(), 3);
        assert_eq!(id.slot(), 42);
        assert_eq!(ConnectionId::from_u64(id.as_u64()), id);
    }

    #[test]
    fn table_acquire_release_cycle() {
        let table: ConnTable<4> = ConnTable::new();
        for i in 0..4 {
            table.initialize(i, Connection::new("127.0.0.1:0".parse().unwrap(), 1));
        }
        let a = table.try_acquire().expect("free slot");
        let ai = a.index();
        assert_eq!(a.conn().hot.seq, 0);
        drop(a);
        let b = table.try_acquire().expect("free again");
        assert_eq!(b.conn().hot.seq, 0);
        drop(b);
        // All four slots are allocatable.
        let s0 = table.try_acquire().unwrap();
        let s1 = table.try_acquire().unwrap();
        let s2 = table.try_acquire().unwrap();
        let s3 = table.try_acquire().unwrap();
        assert!(table.try_acquire().is_none());
        assert_eq!(table.in_use(), 4);
        drop((s0, s1, s2, s3));
        assert_eq!(table.in_use(), 0);
        let _ = ai;
    }

    #[test]
    fn hot_and_cold_are_a_line_apart() {
        let conn = Connection::new("127.0.0.1:9".parse().unwrap(), 7);
        let hot = &conn.hot as *const _ as usize;
        let cold = &conn.cold as *const _ as usize;
        // Each is 64 bytes; nothing shares a line (assert they are at
        // least 64 bytes apart in either order).
        let gap = cold.abs_diff(hot);
        assert!(gap >= 64);
    }
}
