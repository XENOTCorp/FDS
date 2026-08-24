//! Template: an EFFECTFUL ATOM (EffMol(Ctx)) — reads and writes a
//! preallocated runtime context.
//!
//! Copy this file, rename the struct and the context, and fill in `apply`.
//! The context is generic over the application's `Ctx` type: define one
//! context per application holding rings, buffers, and counters, all
//! preallocated at startup (standard [R], [ALLOC]).

use mol::{Atom, EffectfulAtom};

/// The application's preallocated runtime context: rings, buffers,
/// event arrays, counters. Allocate everything at startup; the hot path
/// must not allocate (standard [ALLOC]).
pub struct MyCtx {
    // TODO: e.g. `pub ingress: mol::SpscRing<u32, 256>,`
}

/// An effectful atom acting on [`MyCtx`].
#[derive(Clone, Copy)]
pub struct MyEffectfulAtom;

impl Atom for MyEffectfulAtom {
    type Input = u32;
    type Output = u32;
}

impl EffectfulAtom<MyCtx> for MyEffectfulAtom {
    #[inline(always)]
    fn apply(&self, ctx: &mut MyCtx, input: u32) -> u32 {
        // TODO: act on `ctx` (read rings, update counters, ...) and
        // produce the output. Never allocate here.
        let _ = ctx;
        input
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effectful_atom_applies() {
        let atom = MyEffectfulAtom;
        let mut ctx = MyCtx {};
        assert_eq!(atom.apply(&mut ctx, 1), 1);
    }
}
