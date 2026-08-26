//! Atoms: the irreducible morphisms of Mol.
//!
//! A pure atom is a total function `A → B` with no state; an effectful
//! atom is a morphism of EffMol(Ctx), acting on a preallocated runtime
//! context. Atoms carry no hidden state: an atom is exactly one of these
//! two shapes (standard \[A\]).

/// An atom's interface: an input type and an output type.
pub trait Atom {
    type Input;
    type Output;
}

/// A pure atom: `apply` is a total function with no state (PureMol).
///
/// Implement this directly on your (typically zero-sized) atom type, or
/// wrap a closure with [`PureFn`] and call it — `PureFn` is a pure
/// function carrier with no hidden state.
pub trait PureAtom: Atom {
    fn apply(&self, input: Self::Input) -> Self::Output;
}

/// An effectful atom: `apply` reads and writes a preallocated context
/// (EffMol(Ctx)). The context is generic over `Ctx`, so applications may
/// define their own runtime context type.
pub trait EffectfulAtom<Ctx>: Atom {
    fn apply(&self, ctx: &mut Ctx, input: Self::Input) -> Self::Output;
}

/// A pure-function carrier: wrap any closure, call it with
/// [`PureFn::call`]. Zero-sized in the common case (when the closure
/// captures nothing), so it composes with no runtime cost. Note that
/// `PureFn` itself does not implement [`Atom`] — implement [`Atom`] +
/// [`PureAtom`] on your own atom types, or use [`crate::molecule::PureFn`]
/// as a [`crate::molecule::Molecule`].
pub struct PureFn<F>(pub F);

impl<F> PureFn<F> {
    /// Invoke the wrapped function.
    #[inline(always)]
    pub fn call<A, B>(&self, input: A) -> B
    where
        F: Fn(A) -> B,
    {
        (self.0)(input)
    }
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

    #[test]
    fn pure_fn_carrier_invokes() {
        let f = PureFn(|x: u32| x + 1);
        assert_eq!(f.call(41), 42);
    }
}
