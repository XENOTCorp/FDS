//! Lock-free rings (standard [R], thesis NT48).
//!
//! Both rings are power-of-two capacity with bitmask indexing. The SPSC
//! ring keeps in-flight ≤ CAP − 1 so the masked full/empty checks are
//! unambiguous; the MPMC ring (Vyukov) holds up to CAP items, with
//! sequence-number epochs disambiguating full/empty. `SpscRing` is single
//! producer, single consumer; `MpmcRing` allows arbitrary producers and
//! consumers, lock-free.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicUsize, Ordering};

const _: () = assert!(core::mem::size_of::<usize>() >= 4);

/// Single-producer, single-consumer lock-free ring (power-of-two CAP).
///
/// Usage invariant: exactly one thread may call [`SpscRing::try_push`] and
/// exactly one (possibly different) thread may call [`SpscRing::try_pop`]
/// at any time. In-flight items never exceed `CAP − 1`.
///
/// SAFETY: `Sync` is sound because the buffer slot for index `i` is
/// written only by the producer (after a `Release` head store) and read
/// only by the consumer (after an `Acquire` head load); Release/Acquire
/// ordering pairs each write with the read that observes it.
pub struct SpscRing<T, const CAP: usize> {
    buffer: UnsafeCell<[MaybeUninit<T>; CAP]>,
    head: AtomicUsize, // next slot to write (producer-owned)
    tail: AtomicUsize, // next slot to read (consumer-owned)
}

// SAFETY: see struct docs — the slot discipline makes cross-thread access
// sound for `T: Send`.
unsafe impl<T: Send, const CAP: usize> Sync for SpscRing<T, CAP> {}

impl<T, const CAP: usize> SpscRing<T, CAP> {
    /// Mask for power-of-two capacity (a method, since associated consts
    /// cannot reference generic parameters).
    #[inline(always)]
    const fn mask() -> usize {
        CAP - 1
    }

    /// A new empty ring. `CAP` must be a power of two.
    pub const fn new() -> Self {
        assert!(CAP.is_power_of_two(), "SpscRing: CAP must be a power of two");
        SpscRing {
            // SAFETY: MaybeUninit array, no reads before writes.
            buffer: UnsafeCell::new(unsafe { MaybeUninit::uninit().assume_init() }),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }
}

impl<T, const CAP: usize> Default for SpscRing<T, CAP> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const CAP: usize> SpscRing<T, CAP> {
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
            (*self.buffer.get())[head & Self::mask()].write(value);
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
        let v = unsafe { (*self.buffer.get())[tail & Self::mask()].assume_init_read() };
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(v)
    }

    /// Number of in-flight items.
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
pub struct MpmcRing<T, const CAP: usize> {
    cells: UnsafeCell<[Cell<T>; CAP]>,
    head: AtomicUsize, // next enqueue slot
    tail: AtomicUsize, // next dequeue slot
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
        assert!(CAP.is_power_of_two(), "MpmcRing: CAP must be a power of two");
        // Build the cell array through MaybeUninit (the cells hold
        // uninitialized payloads until a push writes them; sequence
        // numbers are set here before any data read can observe them).
        let mut cells: MaybeUninit<[Cell<T>; CAP]> = MaybeUninit::uninit();
        // SAFETY: the pointer targets the array's first element; writing
        // `CAP` consecutive elements stays in bounds.
        let arr_ptr = cells.as_mut_ptr() as *mut Cell<T>;
        for i in 0..CAP {
            // SAFETY: `arr_ptr.add(i)` is in-bounds for i < CAP.
            unsafe {
                arr_ptr.add(i).write(Cell {
                    seq: AtomicUsize::new(i),
                    data: UnsafeCell::new(MaybeUninit::uninit()),
                });
            }
        }
        // SAFETY: every element of the array was written above.
        let cells = unsafe { cells.assume_init() };
        MpmcRing {
            cells: UnsafeCell::new(cells),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
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
            // SAFETY: cells is a valid array of CAP cells.
            let cell = unsafe { &(*self.cells.get())[head & Self::mask()] };
            let seq = cell.seq.load(Ordering::Acquire);
            let diff = seq.wrapping_sub(head) as isize;
            if diff == 0 {
                // Slot available: claim it.
                match self.head.compare_exchange_weak(head, head.wrapping_add(1), Ordering::Relaxed, Ordering::Relaxed)
                {
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
            // SAFETY: cells is a valid array of CAP cells.
            let cell = unsafe { &(*self.cells.get())[tail & Self::mask()] };
            let seq = cell.seq.load(Ordering::Acquire);
            let diff = seq.wrapping_sub(tail.wrapping_add(1)) as isize;
            if diff == 0 {
                match self
                    .tail
                    .compare_exchange_weak(tail, tail.wrapping_add(1), Ordering::Relaxed, Ordering::Relaxed)
                {
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

    /// Number of in-flight items (approximate under concurrency).
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head.wrapping_sub(tail)
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

    /// Threaded stress tests spin busy-wait loops; under parallel debug
    /// execution on few cores they are slow. Run explicitly:
    /// `cargo test --release -p mol-core -- --ignored --test-threads=1`.
    #[test]
    #[ignore]
    fn spsc_threaded_stress() {
        use std::sync::Arc;
        use std::thread;
        const N: usize = 20_000;
        let ring = Arc::new(SpscRing::<u32, 256>::new());
        let r2 = ring.clone();
        let producer = thread::spawn(move || {
            for i in 0..N {
                let mut v = i as u32;
                loop {
                    match r2.try_push(v) {
                        Ok(()) => break,
                        Err(b) => v = b,
                    }
                    // Yield: under parallel test execution the spinning
                    // threads can starve each other on few cores.
                    std::thread::yield_now();
                }
            }
        });
        let consumer = thread::spawn(move || {
            let mut seen = 0usize;
            let mut sum: u64 = 0;
            while seen < N {
                if let Some(v) = ring.try_pop() {
                    sum += v as u64;
                    seen += 1;
                } else {
                    std::thread::yield_now();
                }
            }
            sum
        });
        producer.join().unwrap();
        let sum = consumer.join().unwrap();
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
        use std::sync::Arc;
        use std::thread;
        const P: usize = 2; // producers
        const C: usize = 2; // consumers
        const PER: usize = 5_000; // items per producer
        let ring = Arc::new(MpmcRing::<u32, 1024>::new());
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
                        std::thread::yield_now();
                    }
                }
            }));
        }
        let mut consumers = Vec::new();
        for _ in 0..C {
            let r = ring.clone();
            consumers.push(thread::spawn(move || {
                let mut sum: u64 = 0;
                let mut got = 0usize;
                while got < P * PER {
                    if let Some(v) = r.try_pop() {
                        sum += v as u64;
                        got += 1;
                    } else {
                        std::thread::yield_now();
                    }
                }
                sum
            }));
        }
        for p in producers {
            p.join().unwrap();
        }
        let total: u64 = consumers.into_iter().map(|c| c.join().unwrap()).sum();
        let expect = (0..(P * PER) as u64).sum::<u64>();
        assert_eq!(total, expect);
    }
}
