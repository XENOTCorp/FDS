//! Zero-allocation hot-path probe (standard [ALLOC]).
//!
//! A counting global allocator records every heap allocation. The declared
//! hot path — ring ingress/egress, molecule steps, checksums, pool
//! alloc/return — must perform zero heap allocations; all structures live
//! inline on the stack or in preallocated arenas. The whole pipeline runs
//! in one test so no concurrent test-harness thread can allocate mid-probe.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use mol::{Buffer, Molecule, MpmcRing, Pool, PureFn, SpscRing, par, then, u16_checksum};

struct CountingAllocator;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

#[inline(always)]
fn allocations() -> usize {
    ALLOCATIONS.load(Ordering::Relaxed)
}

/// Pure molecule for the probe: `x + n`.
#[derive(Clone, Copy)]
struct Add(u32);

impl Molecule for Add {
    type State = ();
    type Input = u32;
    type Output = u32;

    #[inline(always)]
    fn step(&self, _state: &mut (), input: u32) -> u32 {
        input + self.0
    }
}

/// Pure molecule for the probe: `x * n`.
#[derive(Clone, Copy)]
struct Mul(u32);

impl Molecule for Mul {
    type State = ();
    type Input = u32;
    type Output = u32;

    #[inline(always)]
    fn step(&self, _state: &mut (), input: u32) -> u32 {
        input * self.0
    }
}

/// The per-core runtime context, fully preallocated at startup.
struct Ctx {
    ingress: SpscRing<u32, 64>,
    egress: SpscRing<u32, 64>,
    scratch: Buffer<1500>,
}

/// Effectful molecule: one processing step — checksum the input in the
/// preallocated scratch buffer and hand the result to the egress ring.
struct Probe;

impl Molecule for Probe {
    type State = Ctx;
    type Input = u32;
    type Output = u32;

    #[inline(always)]
    fn step(&self, ctx: &mut Ctx, input: u32) -> u32 {
        let buf = &mut ctx.scratch;
        assert!(buf.set_len(4).is_ok());
        buf.as_mut_slice().copy_from_slice(&input.to_be_bytes());
        let csum = u16_checksum(buf.as_slice());
        let output = input.wrapping_add(csum as u32);
        assert!(ctx.egress.try_push(output).is_ok());
        output
    }
}

#[test]
fn reactor_pipeline_allocates_nothing() {
    // Construction (setup, before the watermark): a pool arena is
    // heap-backed (see `Pool` docs), so build it here — the hot path
    // below only does allocate/return cycles.
    let pool: Pool<u64, 8> = Pool::new();
    for i in 0..8 {
        pool.initialize(i, i as u64);
    }

    // Snapshot the allocation count, then run the whole hot path. Any heap
    // allocation inside it fails the test.
    let before = allocations();

    let mut ctx = Ctx {
        ingress: SpscRing::new(),
        egress: SpscRing::new(),
        scratch: Buffer::new(),
    };

    // Ingress: 60 items (in-flight ≤ 63 for CAP 64).
    for i in 0..60u32 {
        assert!(ctx.ingress.try_push(i).is_ok());
    }

    // Composed pure pipeline: (x + 1) * 2.
    let pipeline = then(Add(1), Mul(2));
    let probe = Probe;
    let mut pipeline_state = ((), ());
    let mut processed = 0u64;
    while let Some(input) = ctx.ingress.try_pop() {
        let mid = pipeline.step(&mut pipeline_state, input);
        let out = probe.step(&mut ctx, mid);
        processed += out as u64;
    }
    assert!(processed > 0);

    // Tensor and array molecules in the same hot path.
    let pp = par(Add(1), Add(2));
    let mut ps = ((), ());
    assert_eq!(pp.step(&mut ps, (3, 4)), (4, 6));
    let batch = [Add(1); 4];
    let mut bstates = [(), (), (), ()];
    assert_eq!(batch.step(&mut bstates, [1, 2, 3, 4]), [2, 3, 4, 5]);

    // Closure carrier: no captured state, no allocation.
    let f = PureFn(|x: u32| x.wrapping_mul(3));
    assert_eq!(f.call(7), 21);

    // MPMC ring in the hot path (single-threaded here).
    let mpmc: MpmcRing<u32, 16> = MpmcRing::new();
    for i in 0..16u32 {
        assert!(mpmc.try_push(i).is_ok());
    }
    let mut acc = 0u64;
    while let Some(v) = mpmc.try_pop() {
        acc += v as u64;
    }
    assert_eq!(acc, (0..16u32).map(u64::from).sum());

    // Lock-free pool: allocate/return cycles, zero allocation.
    for _ in 0..100 {
        let guard = pool.try_alloc().expect("slot free after drop");
        assert_eq!(*guard, guard.index() as u64);
        drop(guard);
    }
    assert_eq!(pool.in_use(), 0);

    // Egress ring received every output.
    let mut egress_sum = 0u64;
    while let Some(v) = ctx.egress.try_pop() {
        egress_sum += v as u64;
    }
    assert_eq!(egress_sum, processed);

    assert_eq!(
        allocations(),
        before,
        "hot path must not allocate (standard [ALLOC])"
    );
}
