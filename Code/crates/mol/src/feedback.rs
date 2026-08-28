//! Delayed feedback: the reactor loop of a molecule, not a JSV trace.
//!
//! For a body `M : A ⊗ U → B ⊗ U` the reactor loop `Tr(M) : A → B`
//! has state `S × U` and feeds the `U`-output of one step into the
//! `U`-input of the next (thesis ch. 8, ch. 10). Vanishing, superposing,
//! and sliding of pure maps hold. Yanking fails: `Tr(σ_{U,U})` is the
//! one-step delay `Δ_U`, not `id_U`, whenever `|U| > 1`.

use crate::molecule::Molecule;
use core::marker::PhantomData;

/// One-step delay `Δ_U`: state space `U`, `step(u, x) = (u, x)`.
///
/// The output is the previous register value; the input becomes the
/// next register value.
pub struct Delay<U>(PhantomData<U>);

impl<U> Delay<U> {
    /// The delay molecule on register type `U`.
    pub const fn new() -> Self {
        Delay(PhantomData)
    }
}

impl<U> Default for Delay<U> {
    fn default() -> Self {
        Self::new()
    }
}

/// Constructor for [`Delay`].
pub const fn delay<U>() -> Delay<U> {
    Delay::new()
}

impl<U: Copy + 'static> Molecule for Delay<U> {
    type State = U;
    type Input = U;
    type Output = U;

    #[inline(always)]
    fn step(&self, state: &mut U, input: U) -> U {
        let prev = *state;
        *state = input;
        prev
    }
}

/// Delayed trace `Tr(M)`: body `M : (A, U) → (B, U)` becomes `A → B`
/// with loop-carried register `U`.
///
/// The register type is `Copy`, matching dataplane registers.
pub struct Tr<M> {
    body: M,
}

/// Constructor for [`Tr`].
pub const fn tr<M>(body: M) -> Tr<M> {
    Tr { body }
}

impl<M, A, B, U> Molecule for Tr<M>
where
    M: Molecule<Input = (A, U), Output = (B, U)>,
    U: Copy + 'static,
{
    type State = (M::State, U);
    type Input = A;
    type Output = B;

    #[inline(always)]
    fn step(&self, state: &mut (M::State, U), input: A) -> B {
        let u = state.1;
        let (b, u_next) = self.body.step(&mut state.0, (input, u));
        state.1 = u_next;
        b
    }
}

/// Fixed-iteration loop `L_K(M)`: apply the register update `K` times
/// per input (`K ≥ 1`) and emit the last output. `K = 1` is [`Tr`].
pub struct FixedIter<const K: usize, M> {
    body: M,
}

/// Constructor for [`FixedIter`]. `K` must be at least 1.
pub const fn fixed_iter<const K: usize, M>(body: M) -> FixedIter<K, M> {
    const { assert!(K >= 1, "fixed-iteration loop requires K >= 1") };
    FixedIter { body }
}

impl<const K: usize, M, A, B, U> Molecule for FixedIter<K, M>
where
    M: Molecule<Input = (A, U), Output = (B, U)>,
    A: Copy,
    U: Copy + 'static,
{
    type State = (M::State, U);
    type Input = A;
    type Output = B;

    #[inline]
    fn step(&self, state: &mut (M::State, U), input: A) -> B {
        const { assert!(K >= 1, "fixed-iteration loop requires K >= 1") };
        let mut b = {
            let u = state.1;
            let (out, u_next) = self.body.step(&mut state.0, (input, u));
            state.1 = u_next;
            out
        };
        for _ in 1..K {
            let u = state.1;
            let (out, u_next) = self.body.step(&mut state.0, (input, u));
            state.1 = u_next;
            b = out;
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::molecule::Molecule;

    /// Braiding `σ_{U,U}`: swap, pure (state `()`).
    struct Swap;

    impl Molecule for Swap {
        type State = ();
        type Input = (u32, u32);
        type Output = (u32, u32);

        fn step(&self, _state: &mut (), input: (u32, u32)) -> (u32, u32) {
            (input.1, input.0)
        }
    }

    #[test]
    fn tr_of_swap_is_delay_not_identity() {
        // Yanking fails: Tr(σ) = Δ_U, and Δ_U ≠ id when |U| > 1.
        let looped = tr(Swap);
        let mut s = ((), 7u32);
        assert_eq!(looped.step(&mut s, 3), 7);
        assert_eq!(s.1, 3);
        assert_eq!(looped.step(&mut s, 9), 3);
        assert_eq!(s.1, 9);

        let d = delay::<u32>();
        let mut u = 7u32;
        assert_eq!(d.step(&mut u, 3), 7);
        assert_eq!(u, 3);
        assert_eq!(d.step(&mut u, 9), 3);
        assert_eq!(u, 9);

        // Identity on U would have returned the input; delay returned the
        // previous register.
        let mut id_state = ();
        // Swap as a pure map on a pair is not the identity on U.
        assert_ne!(Swap.step(&mut id_state, (3, 7)), (3, 7));
    }

    #[test]
    fn vanishing_unit_register() {
        struct Body;
        impl Molecule for Body {
            type State = u32;
            type Input = (u32, ());
            type Output = (u32, ());
            fn step(&self, s: &mut u32, (a, ()): (u32, ())) -> (u32, ()) {
                *s = s.wrapping_add(1);
                (a.wrapping_add(*s), ())
            }
        }
        let m = tr(Body);
        let mut st = (0u32, ());
        assert_eq!(m.step(&mut st, 10), 11);
        assert_eq!(st.0, 1);
        assert_eq!(m.step(&mut st, 10), 12);
    }

    #[test]
    fn fixed_iter_k1_matches_tr() {
        let mut t = ((), 1u32);
        let mut f = ((), 1u32);
        let a = tr(Swap).step(&mut t, 5);
        let b = fixed_iter::<1, _>(Swap).step(&mut f, 5);
        assert_eq!(a, b);
        assert_eq!(t.1, f.1);
    }

    #[test]
    fn fixed_iter_applies_k_times() {
        // K = 2 on Swap: first step emits 1 stores 5; second emits 5 stores 5.
        let mut s = ((), 1u32);
        let out = fixed_iter::<2, _>(Swap).step(&mut s, 5);
        assert_eq!(out, 5);
        assert_eq!(s.1, 5);
    }
}
