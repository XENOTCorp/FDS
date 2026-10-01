//! Regression checks for ownership and initialization boundaries.
use mol::{MpmcRing, Pool, PoolGuard, SpscConsumer, SpscProducer, SpscRing};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use std::cell::Cell;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

assert_not_impl_any!(SpscRing<u32, 8>: Sync);
assert_impl_all!(SpscProducer<'static, u32, 8>: Send);
assert_impl_all!(SpscConsumer<'static, u32, 8>: Send);
assert_not_impl_any!(SpscProducer<'static, u32, 8>: Sync, Clone);
assert_not_impl_any!(SpscConsumer<'static, u32, 8>: Sync, Clone);
assert_impl_all!(Pool<Cell<u32>, 4>: Send, Sync);
assert_impl_all!(PoolGuard<'static, Cell<u32>, 4>: Send);
assert_not_impl_any!(PoolGuard<'static, Cell<u32>, 4>: Sync);

struct Tracked(Arc<AtomicUsize>);
impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn array_composition_and_delay_move_noncopy_payloads() {
    use mol::Molecule;
    #[derive(Clone, Copy)]
    struct Owned;
    impl Molecule for Owned {
        type State = usize;
        type Input = String;
        type Output = String;
        fn step(&self, state: &mut usize, input: String) -> String {
            *state += input.len();
            input
        }
    }
    let mut state = [0, 0];
    let output = [Owned; 2].step(&mut state, [String::from("a"), String::from("bb")]);
    assert_eq!(output, ["a", "bb"]);
    assert_eq!(state, [1, 2]);
    let delay = mol::delay::<String>();
    let mut state = String::from("old");
    assert_eq!(delay.step(&mut state, String::from("new")), "old");
    assert_eq!(state, "new");
}

#[test]
fn large_checksums_preserve_end_around_carries() {
    let input = vec![0xff; 300_001];
    let mut reference = 0u64;
    for bytes in input.chunks(2) {
        reference += u64::from(bytes[0]) << 8;
        if bytes.len() == 2 {
            reference += u64::from(bytes[1]);
        }
    }
    while reference >> 16 != 0 {
        reference = (reference & 0xffff) + (reference >> 16);
    }
    assert_eq!(mol::u16_checksum(&input), !(reference as u16));
}

#[test]
fn mappings_reject_zero_and_overflowing_lengths() {
    assert!(mol::huge_page(0).is_none());
    assert!(mol::huge_page(usize::MAX).is_none());
    assert!(mol::huge_page(isize::MAX as usize).is_none());
    let mut value = std::mem::MaybeUninit::<u64>::uninit();
    // SAFETY: every all-zero bit pattern is valid for u64.
    unsafe { mol::zeroed(&mut value) };
    assert_eq!(unsafe { value.assume_init() }, 0);
}

#[test]
fn default_pool_is_initialized_before_allocation() {
    let pool = Pool::<String, 4>::new();
    let mut guard = pool.try_alloc().unwrap();
    assert!(guard.is_empty());
    guard.push_str("retained");
    drop(guard);
    assert_eq!(pool.in_use(), 0);
}

#[test]
fn pool_replacement_and_teardown_drop_every_value() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut pool = Pool::<Tracked, 4>::new_with(|_| Tracked(drops.clone()));
    pool.initialize(0, Tracked(drops.clone()));
    assert_eq!(drops.load(Ordering::Relaxed), 1);
    drop(pool.try_alloc().unwrap());
    assert_eq!(drops.load(Ordering::Relaxed), 1, "return retains the value");
    drop(pool);
    assert_eq!(drops.load(Ordering::Relaxed), 5);
}

#[test]
fn panicking_initializer_drops_partial_pool() {
    let drops = Arc::new(AtomicUsize::new(0));
    let result = std::panic::catch_unwind(|| {
        Pool::<Tracked, 4>::new_with(|i| {
            assert!(i != 2, "initializer failed");
            Tracked(drops.clone())
        })
    });
    assert!(result.is_err());
    assert_eq!(drops.load(Ordering::Relaxed), 2);
}

#[test]
fn raw_index_cycle_retains_initialized_value() {
    let pool = Pool::<u32, 4>::new_with(|i| i as u32);
    let index = pool.try_alloc_index().unwrap();
    // SAFETY: index is exclusively acquired from this pool; the mutable
    // reference expires before the index is returned exactly once.
    unsafe {
        *pool.get_mut(index) = 42;
        pool.release_index(index);
    }
    assert_eq!(pool.in_use(), 0);
}

#[test]
#[should_panic(expected = "power of two >= 2")]
fn mpmc_rejects_single_slot_which_cannot_disambiguate_epochs() {
    let _ = MpmcRing::<u32, 1>::new();
}

#[test]
#[should_panic(expected = "power of two >= 2")]
fn spsc_rejects_zero_usable_capacity() {
    let _ = SpscRing::<u32, 1>::new();
}
