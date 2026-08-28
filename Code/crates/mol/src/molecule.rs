//! Molecules: stateful transformations, the morphisms of Mol.
//!
//! A molecule is a Mealy machine: `step` maps `S × A → B × S`, realized
//! as in-place mutation of the state (`&mut State`) plus a returned output.
//! Under currying, `step` is a Kleisli morphism of the state monad on `S`;
//! the response on input words is that morphism's iterate. Mol composition
//! (`then`) products the states; Kleisli composition of two EffMol
//! machines (`kleisli_then`) threads one context. Tensor (`par`) runs two
//! molecules side by side on product states. All molecules are `Sized`
//! and their state is `'static`; hot-path molecules should keep `State`
//! and `Input`/`Output` small and `Copy` (linearity: exactly-once
//! consumption, no hidden allocation).

/// A stateful transformation `A → B` with state space `S`.
///
/// `step` is the Mealy transition: it reads `input`, mutates `state` in
/// place (the new state), and returns the output. `S: 'static` so states
/// can live in preallocated per-core structures. Equality of morphisms
/// in Mol is state-space bijection of representatives, not bisimulation.
pub trait Molecule: Sized {
    type State: 'static;
    type Input;
    type Output;

    /// One step: `S × A → B × S` with the state transition in place.
    fn step(&self, state: &mut Self::State, input: Self::Input) -> Self::Output;
}

/// PureMol: molecules whose state space is the unit type.
/// These are exactly total functions `A → B`.
pub trait PureMolecule: Molecule<State = ()> {}

impl<T: Molecule<State = ()>> PureMolecule for T {}

/// EffMol(Ctx): molecules whose state space is exactly a runtime context
/// `Ctx`.
///
/// `Ctx` is generic: each application supplies its preallocated context.
/// Composition in this subcategory is [`crate::kleisli_then`] (one
/// context threaded). Mol composition [`crate::then`] of two such
/// machines has state `(Ctx, Ctx)` and is hybrid.
pub trait EffectfulMolecule<Ctx>: Molecule<State = Ctx> {}

impl<Ctx, T: Molecule<State = Ctx>> EffectfulMolecule<Ctx> for T {}

/// Product-state representative of a hybrid molecule: `State = (Spure, Ctx)`.
///
/// HybridMol is the residual class: neither `S ≅ ()` (pure) nor
/// `S ≅ Ctx` (effectful). This marker names the common product-state
/// form, including `then` of two effectful machines (`E ∘ E ⊆ H`).
/// It does not claim a unique factorization `q ∘ e ∘ p`. A dummy-wire
/// factorization of a representative exists and does not identify `S`
/// among behavioral equivalents. The inequalities that make
/// `(Spure, Ctx)` genuinely hybrid (`Spure ≇ ()`, product `≇ Ctx`) are
/// semantic; `((), Ctx)` is state-space-isomorphic to `Ctx`.
pub trait HybridMolecule<Spure, Ctx>: Molecule<State = (Spure, Ctx)> {}

impl<Spure, Ctx, T: Molecule<State = (Spure, Ctx)>> HybridMolecule<Spure, Ctx> for T {}

/// A pure-function carrier that IS a molecule (state = `()`): wrap any
/// closure. Zero-sized when the closure captures nothing.
pub struct PureFn<F>(pub F);

impl<F> PureFn<F> {
    #[inline(always)]
    pub fn call<A, B>(&self, input: A) -> B
    where
        F: Fn(A) -> B,
    {
        (self.0)(input)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Concrete pure molecule for tests: `x + n`, no state.
    #[derive(Clone, Copy)]
    pub(crate) struct Add(pub(crate) u32);

    impl Molecule for Add {
        type State = ();
        type Input = u32;
        type Output = u32;

        #[inline(always)]
        fn step(&self, _state: &mut (), input: u32) -> u32 {
            input + self.0
        }
    }

    /// Concrete pure molecule for tests: `x * n`.
    #[derive(Clone, Copy)]
    pub(crate) struct Mul(pub(crate) u32);

    impl Molecule for Mul {
        type State = ();
        type Input = u32;
        type Output = u32;

        #[inline(always)]
        fn step(&self, _state: &mut (), input: u32) -> u32 {
            input * self.0
        }
    }

    /// Concrete pure molecule for tests: `x - n` (wrapping).
    #[derive(Clone, Copy)]
    pub(crate) struct Sub(pub(crate) u32);

    impl Molecule for Sub {
        type State = ();
        type Input = u32;
        type Output = u32;

        #[inline(always)]
        fn step(&self, _state: &mut (), input: u32) -> u32 {
            input.wrapping_sub(self.0)
        }
    }

    /// Concrete pure molecule for tests: boolean negation.
    #[derive(Clone, Copy)]
    pub(crate) struct Not;

    impl Molecule for Not {
        type State = ();
        type Input = bool;
        type Output = bool;

        #[inline(always)]
        fn step(&self, _state: &mut (), input: bool) -> bool {
            !input
        }
    }

    #[test]
    fn concrete_molecule_steps() {
        let m = Add(1);
        let mut s = ();
        assert_eq!(m.step(&mut s, 41), 42);
    }

    #[test]
    fn marker_traits_apply() {
        fn assert_pure<M: PureMolecule>(_: &M) {}
        fn assert_effectful<Ctx, M: EffectfulMolecule<Ctx>>(_: &M) {}

        struct CountCtx(u32);
        struct Count;
        impl Molecule for Count {
            type State = CountCtx;
            type Input = ();
            type Output = u32;
            fn step(&self, s: &mut CountCtx, _: ()) -> u32 {
                s.0 += 1;
                s.0
            }
        }

        let m = Add(0);
        assert_pure(&m);
        assert_effectful::<CountCtx, _>(&Count);
    }

    #[test]
    fn mol_then_of_effectful_is_hybrid_marker() {
        use crate::compose::then;
        fn assert_hybrid<Spure, Ctx, M: HybridMolecule<Spure, Ctx>>(_: &M) {}
        struct Tick;
        impl Molecule for Tick {
            type State = u32;
            type Input = u32;
            type Output = u32;
            fn step(&self, s: &mut u32, x: u32) -> u32 {
                *s += 1;
                x
            }
        }
        let m = then(Tick, Tick);
        assert_hybrid::<u32, u32, _>(&m);
    }
}
