//! Equational law tests: the algebraic theory of
//! Mol must hold in the concrete model. Property-style tests over
//! deterministic sweeps — no external RNG, so the suite stays fast and
//! reproducible (standard [TEST]).

use mol::{Molecule, MpmcRing, SpscRing, par, then};

/// Pure molecule: `x + n`.
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

/// Pure molecule: `x * n`.
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

/// Stateful molecule: output = input + accumulator, accumulator += n each
/// step (a per-connection sequence counter minimal state).
#[derive(Clone, Copy)]
struct Acc(u32);

impl Molecule for Acc {
    type State = u32;
    type Input = u32;
    type Output = u32;

    #[inline(always)]
    fn step(&self, state: &mut u32, input: u32) -> u32 {
        *state = state.wrapping_add(self.0);
        input.wrapping_add(*state)
    }
}

/// Stateful molecule with a branch (exercises determinism on both paths).
#[derive(Clone, Copy)]
struct Branchy;

impl Molecule for Branchy {
    type State = u32;
    type Input = u32;
    type Output = u32;

    #[inline(always)]
    fn step(&self, state: &mut u32, input: u32) -> u32 {
        *state = state.wrapping_add(1);
        if input.is_multiple_of(2) {
            input.wrapping_add(*state)
        } else {
            input.wrapping_mul(*state)
        }
    }
}

#[test]
fn then_is_associative() {
    // (f;g);h and f;(g;h) agree on every input. The
    // state shapes differ (nested tuples) but the observable behavior —
    // the output stream — is identical.
    let f = Acc(1);
    let g = Add(2);
    let h = Mul(3);
    let lhs = then(then(f, g), h);
    let rhs = then(f, then(g, h));
    for x in 0..1000u32 {
        let mut s1 = ((0u32, ()), ());
        let mut s2 = (0u32, ((), ()));
        assert_eq!(lhs.step(&mut s1, x), rhs.step(&mut s2, x), "input {x}");
    }
}

#[test]
fn interchange_law_holds_with_state() {
    // (f⊗g);(h⊗k) ≅ (f;h)⊗(g;k) — with stateful molecules on both
    // sides. The outputs agree for every input; the states agree up to the
    // canonical reassociation isomorphism ((f,g),(h,k)) ≅ ((f,h),(g,k)).
    let f = Acc(1);
    let g = Acc(2);
    let h = Acc(3);
    let k = Acc(4);
    let lhs = then(par(f, g), par(h, k));
    let rhs = par(then(f, h), then(g, k));
    let mut s1 = ((0u32, 0u32), (0u32, 0u32));
    let mut s2 = ((0u32, 0u32), (0u32, 0u32));
    for (x, y) in [(10, 20), (0, 0), (7, 99), (1 << 16, 1 << 8)] {
        let o1 = lhs.step(&mut s1, (x, y));
        let o2 = rhs.step(&mut s2, (x, y));
        assert_eq!(o1, o2, "inputs ({x},{y})");
    }
    // Component correspondence under the reassociation isomorphism:
    // lhs ((f,g),(h,k)) maps to rhs ((f,h),(g,k)).
    assert_eq!(s1.0 .0, s2.0 .0, "f state");
    assert_eq!(s1.0 .1, s2.1 .0, "g state");
    assert_eq!(s1.1 .0, s2.0 .1, "h state");
    assert_eq!(s1.1 .1, s2.1 .1, "k state");
}

#[test]
fn par_is_symmetric_up_to_swap() {
    // f⊗g on (x, y) equals g⊗f on (y, x) with outputs swapped (tensor
    // symmetry).
    let f = Acc(1);
    let g = Add(7);
    let a = par(f, g);
    let b = par(g, f);
    let mut sa = (0u32, ());
    let mut sb = ((), 0u32);
    for x in 0..100u32 {
        let y = x.wrapping_mul(3).wrapping_add(1);
        let (p, q) = a.step(&mut sa, (x, y));
        let (r, s) = b.step(&mut sb, (y, x));
        assert_eq!((p, q), (s, r), "inputs ({x},{y})");
    }
}

#[test]
fn tensor_array_is_elementwise_independent() {
    // [M; N] steps each element independently: element
    // i's output depends only on input i and state i.
    let arr = [Acc(1), Acc(2), Acc(3), Acc(4)];
    let mut states = [0u32; 4];
    let outs = arr.step(&mut states, [10, 20, 30, 40]);
    assert_eq!(outs, [11, 22, 33, 44]);
    assert_eq!(states, [1, 2, 3, 4]);

    // A single-element step leaves other elements' state untouched.
    let single = [Acc(1)];
    let mut s1 = [7u32];
    single.step(&mut s1, [100]);
    assert_eq!(s1, [8]);
}

#[test]
fn sequential_pipeline_satisfies_its_equation() {
    // Concrete equations hold over a sweep (equations in the model):
    // (x + 5) * 2 and x * 2 + 5 are both realized — and they differ, so
    // order matters (composition is not commutative).
    let a = then(Add(5), Mul(2));
    let b = then(Mul(2), Add(5));
    for x in 0..1000u32 {
        let mut sa = ((), ());
        let mut sb = ((), ());
        assert_eq!(a.step(&mut sa, x), (x + 5) * 2, "input {x}");
        assert_eq!(b.step(&mut sb, x), x * 2 + 5, "input {x}");
    }
}

#[test]
fn step_is_deterministic() {
    // Determinism: the same molecule, state, and input give the same output and
    // successor state — on both branches of Branchy.
    let m = Branchy;
    for x in 0..200u32 {
        let mut s1 = 0u32;
        let mut s2 = 0u32;
        let o1 = m.step(&mut s1, x);
        let o2 = m.step(&mut s2, x);
        assert_eq!(o1, o2, "input {x}");
        assert_eq!(s1, s2, "state after input {x}");
    }
    // The transition is a function of (state, input): identical starting
    // states give identical results (no hidden state).
    let mut s = 5u32;
    let a = m.step(&mut s, 3);
    let mut t = 5u32;
    let b = m.step(&mut t, 3);
    assert_eq!((a, s), (b, t));
}

#[test]
fn spsc_ring_is_fifo_across_wraparound() {
    // The ring realizes FIFO streams; push;pop = id elementwise,
    // including across slot-index wraparound (CAP 16, in-flight ≤ 15).
    let ring = SpscRing::<u64, 16>::new();
    let mut next_push = 0u64;
    let mut next_pop = 0u64;
    for _ in 0..5 {
        for _ in 0..5 {
            assert!(ring.try_push(next_push).is_ok());
            next_push += 1;
        }
        for _ in 0..5 {
            let got = ring.try_pop().expect("item in flight");
            assert_eq!(got, next_pop);
            next_pop += 1;
        }
    }
    // Interleaved push/pop also preserves FIFO.
    for _ in 0..3 {
        assert!(ring.try_push(next_push).is_ok());
        next_push += 1;
    }
    assert_eq!(ring.try_pop(), Some(next_pop));
    next_pop += 1;
    for _ in 0..2 {
        assert!(ring.try_push(next_push).is_ok());
        next_push += 1;
    }
    while let Some(v) = ring.try_pop() {
        assert_eq!(v, next_pop);
        next_pop += 1;
    }
    assert_eq!(next_push, next_pop);
}

#[test]
fn mpmc_ring_is_fifo_single_threaded() {
    // Vyukov MPMC holds up to CAP items; single-threaded access is FIFO.
    let ring = MpmcRing::<u64, 8>::new();
    for i in 0..8u64 {
        assert!(ring.try_push(i).is_ok());
    }
    assert!(ring.try_push(99).is_err(), "full at CAP items");
    for i in 0..8u64 {
        assert_eq!(ring.try_pop(), Some(i));
    }
    // Wraparound across a full cycle of sequence-number epochs.
    for i in 8..16u64 {
        assert!(ring.try_push(i).is_ok());
    }
    for i in 8..16u64 {
        assert_eq!(ring.try_pop(), Some(i));
    }
    assert!(ring.is_empty());
}
