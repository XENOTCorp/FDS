//! Template: a PURE ATOM (PureMol) — no state, a total function `A → B`.
//!
//! Copy this file, rename the struct, and fill in `apply`. Keep the atom
//! zero-sized (no fields) whenever possible so it composes with no runtime
//! cost (thesis ch. 3; standard [A]: atoms are pure or effectful, no hidden
//! state, total on their declared domain).

use mol::{Atom, PureAtom};

/// A pure atom transforming `Input` into `Output`.
///
/// Fill in the `apply` body. The signature must be a total function on
/// the declared input domain; validate lengths and bounds inside before
/// indexing untrusted input (standard [SEC]).
#[derive(Clone, Copy)]
pub struct MyPureAtom;

impl Atom for MyPureAtom {
    type Input = u32;
    type Output = u32;
}

impl PureAtom for MyPureAtom {
    #[inline(always)]
    fn apply(&self, input: u32) -> u32 {
        // TODO: replace with the transformation. Keep it branch-light and
        // allocation-free; prefer dense integer matches over if-else chains.
        input
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_atom_applies() {
        let atom = MyPureAtom;
        assert_eq!(atom.apply(1), 1);
    }
}
