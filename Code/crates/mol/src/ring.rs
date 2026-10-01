//! Lock-free FIFO rings (standard \[R\]).
//!
//! Both rings are power-of-two capacity with bitmask indexing. They
//! realize FIFO content: `push` writes at `w`, `pop` reads at `r`.
//! On a nonempty buffer, `push; pop` returns the oldest stored element,
//! not the element just pushed, so they are not models of the stack
//! theory (`push; pop = id`).
//!
//! The SPSC ring keeps in-flight `≤ CAP − 1` so masked full/empty
//! checks are unambiguous (ring-capacity invariant). The MPMC ring
//! (Vyukov) holds up to `CAP` items: sequence-number epochs
//! disambiguate full/empty, which is a different protocol from the
//! bitmask-only occupancy bound. `SpscRing` is single producer, single
//! consumer; `MpmcRing` allows arbitrary producers and consumers,
//! lock-free.

use crate::layout::CachePadded;
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicUsize, Ordering};

const _: () = assert!(core::mem::size_of::<usize>() >= 4);

/// Single-producer, single-consumer lock-free ring (power-of-two CAP).
///
/// The ring is deliberately not `Sync`: shared push/pop methods are safe
/// only on one thread. Use [`SpscRing::split`] for cross-thread access;
/// its exclusive endpoints enforce one producer and one consumer.
/// In-flight items never exceed `CAP − 1`.
#[repr(C)]
pub struct SpscRing<T, const CAP: usize> {
    buffer: [UnsafeCell<MaybeUninit<T>>; CAP],
    /// Producer-owned write index. Own cache line so the consumer's
    /// tail load does not bounce this line.
    head: CachePadded<AtomicUsize>,
    /// Consumer-owned read index. Own cache line so the producer's
    /// head load does not bounce this line.
    tail: CachePadded<AtomicUsize>,
}

/// Exclusive producer endpoint, created by [`SpscRing::split`].
pub struct SpscProducer<'a, T, const CAP: usize> {
    ring: &'a SpscRing<T, CAP>,
    not_sync: PhantomData<core::cell::Cell<()>>,
}

/// Exclusive consumer endpoint, created by [`SpscRing::split`].
pub struct SpscConsumer<'a, T, const CAP: usize> {
    ring: &'a SpscRing<T, CAP>,
    not_sync: PhantomData<core::cell::Cell<()>>,
}

// SAFETY: split exclusively borrows the ring and creates exactly one of
// each endpoint. Endpoints cannot be cloned or shared across threads;
// Release/Acquire publication transfers ownership of T between them.
unsafe impl<T: Send, const CAP: usize> Send for SpscProducer<'_, T, CAP> {}
// SAFETY: the same exclusive endpoint and publication protocol as above.
unsafe impl<T: Send, const CAP: usize> Send for SpscConsumer<'_, T, CAP> {}

impl<T, const CAP: usize> SpscProducer<'_, T, CAP> {
    /// Push an item, returning it if the ring is full.
    pub fn try_push(&mut self, value: T) -> Result<(), T> {
        self.ring.try_push(value)
    }
}

impl<T, const CAP: usize> SpscConsumer<'_, T, CAP> {
    /// Pop the oldest item, or `None` if empty.
    pub fn try_pop(&mut self) -> Option<T> {
        self.ring.try_pop()
    }
}

impl<T, const CAP: usize> SpscRing<T, CAP> {
    /// Mask for power-of-two capacity (a method, since associated consts
    /// cannot reference generic parameters).
    #[inline(always)]
    const fn mask() -> usize {
        CAP - 1
    }

    /// A new empty ring. `CAP` must be a power of two.
    pub const fn new() -> Self {
        assert!(
            CAP >= 2 && CAP.is_power_of_two(),
            "SpscRing: CAP must be a power of two >= 2"
        );
        SpscRing {
            buffer: [const { UnsafeCell::new(MaybeUninit::uninit()) }; CAP],
            head: CachePadded::new(AtomicUsize::new(0)),
            tail: CachePadded::new(AtomicUsize::new(0)),
        }
    }
}

impl<T, const CAP: usize> Default for SpscRing<T, CAP> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const CAP: usize> SpscRing<T, CAP> {
    /// Borrow exclusive endpoints for use on separate threads. The ring
    /// cannot otherwise be accessed until both endpoints are dropped.
    ///
    /// ```
    /// let mut ring = mol::SpscRing::<u32, 8>::new();
    /// let (mut producer, mut consumer) = ring.split();
    /// std::thread::scope(|scope| {
    ///     scope.spawn(move || producer.try_push(42).unwrap());
    ///     scope.spawn(move || loop {
    ///         if let Some(value) = consumer.try_pop() {
    ///             assert_eq!(value, 42);
    ///             break;
    ///         }
    ///         std::thread::yield_now();
    ///     });
    /// });
    /// ```
    pub fn split(&mut self) -> (SpscProducer<'_, T, CAP>, SpscConsumer<'_, T, CAP>) {
        (
            SpscProducer {
                ring: self,
                not_sync: PhantomData,
            },
            SpscConsumer {
                ring: self,
                not_sync: PhantomData,
            },
        )
    }

    /// Push one item; returns it back if the ring is full.
    pub fn try_push(&self, value: T) -> Result<(), T> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if (head.wrapping_add(1) & Self::mask()) == (tail & Self::mask()) {
            return Err(value); // full (in-flight == CAP - 1)
        }
        // SAFETY: the slot `head & mask` is not in use: in-flight < CAP − 1
        // and the producer owns every slot strictly below `head`.
        unsafe {
            (*self.buffer[head & Self::mask()].get()).write(value);
        }
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Pop one item, or `None` if empty.
    pub fn try_pop(&self) -> Option<T> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail == head {
            return None; // empty
        }
        // SAFETY: the slot `tail & mask` was written by the producer and
        // observed via the Acquire load above; the consumer owns it.
        let v = unsafe { (*self.buffer[tail & Self::mask()].get()).assume_init_read() };
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(v)
    }

    /// Number of in-flight items (exact on this single-threaded handle).
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head.wrapping_sub(tail)
    }

    /// Capacity (always `CAP`).
    pub const fn capacity(&self) -> usize {
        CAP
    }

    /// Empty check.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T, const CAP: usize> Drop for SpscRing<T, CAP> {
    /// Drain-and-drop remaining items. Only valid when the ring is
    /// quiesced (teardown); the consumer-side discipline must hold.
    fn drop(&mut self) {
        while let Some(v) = self.try_pop() {
            drop(v);
        }
    }
}

/// Vyukov bounded MPMC queue: arbitrary producers/consumers, lock-free,
/// power-of-two `CAP`. Holds up to `CAP` items; sequence-number epochs
/// disambiguate full/empty.
///
/// Each cell carries a sequence number: enqueue waits for `seq == head`,
/// dequeue waits for `seq == tail + 1`; the difference with `head`/`tail`
/// distinguishes empty/full unambiguously.
#[repr(C)]
pub struct MpmcRing<T, const CAP: usize> {
    cells: [Cell<T>; CAP],
    /// Enqueue index. Own cache line: producers and consumers do not
    /// share this line with `tail`.
    head: CachePadded<AtomicUsize>,
    /// Dequeue index. Own cache line.
    tail: CachePadded<AtomicUsize>,
}

struct Cell<T> {
    seq: AtomicUsize,
    data: UnsafeCell<MaybeUninit<T>>,
}

// SAFETY: the Vyukov protocol synchronizes every slot write (Release seq
// store after data write) with the matching read (Acquire seq load before
// data read), so cross-thread access is sound for `T: Send`.
unsafe impl<T: Send, const CAP: usize> Sync for MpmcRing<T, CAP> {}

impl<T, const CAP: usize> MpmcRing<T, CAP> {
    #[inline(always)]
    const fn mask() -> usize {
        CAP - 1
    }

    /// A new empty ring. `CAP` must be a power of two.
    pub fn new() -> Self {
        assert!(
            CAP >= 2 && CAP.is_power_of_two(),
            "MpmcRing: CAP must be a power of two >= 2"
        );
        let cells = std::array::from_fn(|i| Cell {
            seq: AtomicUsize::new(i),
            data: UnsafeCell::new(MaybeUninit::uninit()),
        });
        MpmcRing {
            cells,
            head: CachePadded::new(AtomicUsize::new(0)),
            tail: CachePadded::new(AtomicUsize::new(0)),
        }
    }
}

impl<T, const CAP: usize> Default for MpmcRing<T, CAP> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const CAP: usize> MpmcRing<T, CAP> {
    /// Push one item; returns it back if the ring is full.
    pub fn try_push(&self, value: T) -> Result<(), T> {
        let mut head = self.head.load(Ordering::Relaxed);
        loop {
            let cell = &self.cells[head & Self::mask()];
            let seq = cell.seq.load(Ordering::Acquire);
            let diff = seq.wrapping_sub(head) as isize;
            if diff == 0 {
                // Slot available: claim it.
                match self.head.compare_exchange_weak(
                    head,
                    head.wrapping_add(1),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        // SAFETY: we own this slot now; no reader can
                        // observe it until the Release seq store.
                        unsafe {
                            (*cell.data.get()).write(value);
                        }
                        cell.seq.store(head.wrapping_add(1), Ordering::Release);
                        return Ok(());
                    }
                    Err(h) => {
                        head = h;
                        continue;
                    }
                }
            } else if diff < 0 {
                return Err(value); // full
            } else {
                head = self.head.load(Ordering::Relaxed);
            }
        }
    }

    /// Pop one item, or `None` if empty.
    pub fn try_pop(&self) -> Option<T> {
        let mut tail = self.tail.load(Ordering::Relaxed);
        loop {
            let cell = &self.cells[tail & Self::mask()];
            let seq = cell.seq.load(Ordering::Acquire);
            let diff = seq.wrapping_sub(tail.wrapping_add(1)) as isize;
            if diff == 0 {
                match self.tail.compare_exchange_weak(
                    tail,
                    tail.wrapping_add(1),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        // SAFETY: the slot was written and published by an
                        // enqueuer observed via the Acquire seq load.
                        let v = unsafe { (*cell.data.get()).assume_init_read() };
                        cell.seq.store(tail.wrapping_add(CAP), Ordering::Release);
                        return Some(v);
                    }
                    Err(t) => {
                        tail = t;
                        continue;
                    }
                }
            } else if diff < 0 {
                return None; // empty
            } else {
                tail = self.tail.load(Ordering::Relaxed);
            }
        }
    }

    /// Number of in-flight items (approximate under concurrency, bounded
    /// by capacity even when the two loads observe different epochs).
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head.wrapping_sub(tail).min(CAP)
    }

    /// Whether the ring is empty (approximate under concurrency).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Capacity (always `CAP`).
    pub const fn capacity(&self) -> usize {
        CAP
    }
}

impl<T, const CAP: usize> Drop for MpmcRing<T, CAP> {
    fn drop(&mut self) {
        // Quiesce: drain remaining items before the cells are dropped.
        while let Some(v) = self.try_pop() {
            drop(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spsc_head_and_tail_occupy_distinct_cache_lines() {
        let h = core::mem::offset_of!(SpscRing<u64, 8>, head);
        let t = core::mem::offset_of!(SpscRing<u64, 8>, tail);
        assert!(
            h.abs_diff(t) >= 64,
            "SPSC head and tail share a cache line: head={h} tail={t}"
        );
    }

    #[test]
    fn mpmc_head_and_tail_occupy_distinct_cache_lines() {
        let h = core::mem::offset_of!(MpmcRing<u64, 8>, head);
        let t = core::mem::offset_of!(MpmcRing<u64, 8>, tail);
        assert!(
            h.abs_diff(t) >= 64,
            "MPMC head and tail share a cache line: head={h} tail={t}"
        );
    }

    #[test]
    fn spsc_roundtrip_and_invariant() {
        let ring = SpscRing::<u32, 8>::new();
        for i in 0..7 {
            assert!(ring.try_push(i).is_ok()); // CAP-1 = 7 in flight
        }
        assert!(ring.try_push(99).is_err()); // full
        assert_eq!(ring.len(), 7);
        for i in 0..7 {
            assert_eq!(ring.try_pop(), Some(i));
        }
        assert_eq!(ring.try_pop(), None);
        assert_eq!(ring.len(), 0);
    }

    #[test]
    fn spsc_wraparound() {
        let ring = SpscRing::<u32, 4>::new();
        for round in 0..3 {
            for i in 0..3 {
                assert!(ring.try_push(round * 10 + i).is_ok());
            }
            for i in 0..3 {
                assert_eq!(ring.try_pop(), Some(round * 10 + i));
            }
        }
    }

    /// Threaded stress tests busy-wait by nature, so the default suite
    /// skips them. Run explicitly (~2 s in debug):
    /// `cargo test -p mol -- --ignored --test-threads=1`.
    ///
    /// The spin loops use a pause-instruction backoff with a periodic
    /// scheduler yield: cheaper than a `sched_yield` syscall per iteration
    /// (which dominates in unoptimized debug builds) while still letting
    /// spinning threads share few cores.
    fn spin_backoff(rounds: u32) {
        for _ in 0..rounds {
            core::hint::spin_loop();
        }
        std::thread::yield_now();
    }

    #[test]
    #[ignore]
    fn spsc_threaded_stress() {
        const N: usize = 20_000;
        let mut ring = SpscRing::<u32, 256>::new();
        let (mut producer, mut consumer) = ring.split();
        let sum = std::thread::scope(|scope| {
            scope.spawn(move || {
                for i in 0..N {
                    let mut v = i as u32;
                    loop {
                        match producer.try_push(v) {
                            Ok(()) => break,
                            Err(b) => v = b,
                        }
                        spin_backoff(64);
                    }
                }
            });
            scope
                .spawn(move || {
                    let mut seen = 0usize;
                    let mut sum = 0u64;
                    while seen < N {
                        if let Some(v) = consumer.try_pop() {
                            sum += v as u64;
                            seen += 1;
                        } else {
                            spin_backoff(64);
                        }
                    }
                    sum
                })
                .join()
                .unwrap()
        });
        let expect = (0..N as u64).sum::<u64>();
        assert_eq!(sum, expect);
    }

    #[test]
    fn mpmc_roundtrip_and_invariant() {
        let ring = MpmcRing::<u32, 8>::new();
        // Vyukov MPMC holds up to CAP items (unlike the SPSC ring).
        for i in 0..8 {
            assert!(ring.try_push(i).is_ok());
        }
        assert!(ring.try_push(99).is_err()); // 9th fails: full
        assert_eq!(ring.len(), 8);
        for i in 0..8 {
            assert_eq!(ring.try_pop(), Some(i));
        }
        assert_eq!(ring.try_pop(), None);
        assert_eq!(ring.len(), 0);
    }

    #[test]
    #[ignore]
    fn mpmc_threaded_stress() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::Arc;
        use std::thread;
        const P: usize = 2; // producers
        const C: usize = 2; // consumers

        // Miri retains contention and slot reuse with a smaller queue
        // and workload; the native stress case still transfers 10k items.
        const PER: usize = if cfg!(miri) { 100 } else { 5_000 };
        const CAP: usize = if cfg!(miri) { 8 } else { 1024 };
        const TOTAL: usize = P * PER;
        let ring = Arc::new(MpmcRing::<u32, CAP>::new());
        // Shared count of items still to be consumed: consumers exit when
        // it reaches zero (a per-consumer `got == TOTAL` target can never
        // be met once the items are split between consumers; livelock).
        let remaining = Arc::new(AtomicUsize::new(TOTAL));
        let mut producers = Vec::new();
        for p in 0..P {
            let r = ring.clone();
            producers.push(thread::spawn(move || {
                for i in 0..PER {
                    let v = (p * PER + i) as u32;
                    let mut v = v;
                    loop {
                        match r.try_push(v) {
                            Ok(()) => break,
                            Err(b) => v = b,
                        }
                        spin_backoff(64);
                    }
                }
            }));
        }
        let mut consumers = Vec::new();
        for _ in 0..C {
            let r = ring.clone();
            let rem = remaining.clone();
            consumers.push(thread::spawn(move || {
                let mut sum: u64 = 0;
                loop {
                    if rem.load(Ordering::Relaxed) == 0 {
                        break;
                    }
                    if let Some(v) = r.try_pop() {
                        rem.fetch_sub(1, Ordering::Relaxed);
                        sum += v as u64;
                    } else {
                        spin_backoff(64);
                    }
                }
                sum
            }));
        }
        for p in producers {
            p.join().unwrap();
        }
        let total: u64 = consumers.into_iter().map(|c| c.join().unwrap()).sum();
        let expect = (0..TOTAL as u64).sum::<u64>();
        assert_eq!(total, expect);
    }
}
