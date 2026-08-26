//! Composition combinators: `then` (sequential `∘`) and `par` (tensor `⊗`).
//!
//! Sequential composition threads `M`'s output into `N`'s input over a
//! product state; tensor runs two molecules in parallel over a product
//! state and pairs their outputs (sequential associativity, tensor
//! symmetry). Both are zero-allocation by construction.

use crate::molecule::Molecule;

/// Sequential composition `g ∘ f`: run `m` on the input, feed its output
/// to `n`. State is `(M::State, N::State)`.
pub fn then<M, N>(m: M, n: N) -> Compose<M, N>
where
    M: Molecule,
    N: Molecule<Input = M::Output>,
{
    Compose { m, n }
}

/// Tensor `f ⊗ g`: run `m` and `n` in parallel on paired inputs, producing
/// paired outputs. State is `(M::State, N::State)`.
pub fn par<M, N>(m: M, n: N) -> Par<M, N>
where
    M: Molecule,
    N: Molecule,
{
    Par { m, n }
}

/// The sequential composite `M; N` (pipeline order: `m` first, then `n`).
pub struct Compose<M, N> {
    m: M,
    n: N,
}

impl<M, N> Molecule for Compose<M, N>
where
    M: Molecule,
    N: Molecule<Input = M::Output>,
{
    type State = (M::State, N::State);
    type Input = M::Input;
    type Output = N::Output;

    #[inline(always)]
    fn step(&self, state: &mut (M::State, N::State), input: M::Input) -> N::Output {
        let b = self.m.step(&mut state.0, input);
        self.n.step(&mut state.1, b)
    }
}

/// The tensor `M ⊗ N`.
pub struct Par<M, N> {
    m: M,
    n: N,
}

impl<M, N> Molecule for Par<M, N>
where
    M: Molecule,
    N: Molecule,
{
    type State = (M::State, N::State);
    type Input = (M::Input, N::Input);
    type Output = (M::Output, N::Output);

    #[inline(always)]
    fn step(&self, state: &mut (M::State, N::State), input: (M::Input, N::Input)) -> (M::Output, N::Output) {
        // Sequential evaluation in pipeline order: m's state transition
        // happens-before n's, matching the tensor's product-state
        // semantics (the compiler may interleave at the instruction level).
        let b1 = self.m.step(&mut state.0, input.0);
        let b2 = self.n.step(&mut state.1, input.1);
        (b1, b2)
    }
}

/// An array of molecules is a molecule: `[M; N]` realizes the n-fold
/// tensor `M ⊗ ⋯ ⊗ M`. Requires
/// `Input`/`Output: Copy` so elements can be read out of the arrays
/// without allocation.
impl<M, const N: usize> Molecule for [M; N]
where
    M: Molecule,
    M::Input: Copy,
    M::Output: Copy,
{
    type State = [M::State; N];
    type Input = [M::Input; N];
    type Output = [M::Output; N];

    #[inline]
    fn step(&self, state: &mut [M::State; N], input: [M::Input; N]) -> [M::Output; N] {
        std::array::from_fn(|i| self[i].step(&mut state[i], input[i]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::molecule::{Molecule, tests::{Add, Mul, Not, Sub}};

    #[test]
    fn sequential_composition_threads_output() {
        let m = Add(1);
        let n = Mul(10);
        let c = then(m, n);
        let mut s = ((), ());
        assert_eq!(c.step(&mut s, 4), 50); // (4+1)*10
    }

    #[test]
    fn tensor_runs_in_parallel() {
        let m = Add(1);
        let n = Not;
        let p = par(m, n);
        let mut s = ((), ());
        assert_eq!(p.step(&mut s, (4, true)), (5, false));
    }

    #[test]
    fn interchange_law_holds_on_examples() {
        // (f⊗g);(h⊗k) == (f;h)⊗(g;k).
        let f = Add(1);
        let g = Mul(2);
        let h = Add(3);
        let k = Sub(1);

        let lhs = then(par(f, g), par(h, k));
        let rhs = par(then(f, h), then(g, k));

        let mut s1 = (((), ()), ((), ()));
        let mut s2 = (((), ()), ((), ()));
        let out1 = lhs.step(&mut s1, (10, 10));
        let out2 = rhs.step(&mut s2, (10, 10));
        assert_eq!(out1, out2);
        assert_eq!(out1, (14, 19)); // (10+1+3, 10*2-1)
    }

    #[test]
    fn array_tensor_steps_all_elements() {
        let arr = [Add(1), Add(2), Add(3)];
        let mut s = [(), (), ()];
        assert_eq!(arr.step(&mut s, [1, 1, 1]), [2, 3, 4]);
    }
}
