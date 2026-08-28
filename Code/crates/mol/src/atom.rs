//! Atoms: the irreducible morphisms of Mol.
//!
//! A pure atom is a total function `A → B` with no state; an effectful
//! atom is a morphism of EffMol(Ctx), acting on a preallocated runtime
//! context. Atoms carry no hidden state: an atom is exactly one of these
//! two shapes (standard \[A\]). Hybrid is a residual class of
//! *molecules* (stateful composites), not a third atom kind.

/// An atom's interface: an input type and an output type.
pub trait Atom {
    type Input;
    type Output;
}

/// A pure atom: `apply` is a total function with no state (PureMol).
///
/// Implement this directly on your (typically zero-sized) atom type.
/// For a closure carrier that is also a molecule, use
/// [`crate::molecule::PureFn`].
pub trait PureAtom: Atom {
    fn apply(&self, input: Self::Input) -> Self::Output;
}

/// An effectful atom: `apply` reads and writes a preallocated context
/// (EffMol(Ctx)). The context is generic over `Ctx`, so applications may
/// define their own runtime context type.
pub trait EffectfulAtom<Ctx>: Atom {
    fn apply(&self, ctx: &mut Ctx, input: Self::Input) -> Self::Output;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A concrete pure atom (the pattern to copy): zero-sized, no state.
    #[derive(Clone, Copy)]
    struct Inc(u32);

    impl Atom for Inc {
        type Input = u32;
        type Output = u32;
    }

    impl PureAtom for Inc {
        fn apply(&self, input: u32) -> u32 {
            input + self.0
        }
    }

    #[test]
    fn concrete_atom_works() {
        let a = Inc(1);
        assert_eq!(a.apply(41), 42);
    }
}
