//! Template: a HYBRID MOLECULE (HybridMol) — protocol state × context,
//! with hot/cold state separation.
//!
//! Copy this file, rename the structs, and fill in `step`. The state is
//! `(ProtocolState, Ctx)` = `Spure × Ctx` (thesis ch. 3, the pure/effectful classification), where
//! `ProtocolState` pairs the HOT fields (touched every step) and COLD
//! fields (touched rarely: peer info, timers) in separate cache lines so
//! they never share one (standard [CACHE], thesis ch. 7, the resource-graded cost enrichment).

use mol::Molecule;

/// The application context (see the effectful-atom template).
pub struct Ctx;

/// HOT state: fields touched on every step. Align to a cache line.
#[repr(align(64))]
pub struct HotState {
    pub sequence: u32,
}

/// COLD state: fields touched rarely (peer info, timers). Its own line.
#[repr(align(64))]
pub struct ColdState {
    pub peer_id: u32,
}

/// The hybrid molecule: state `((HotState, ColdState), Ctx)` — the pure
/// protocol state (hot/cold split) paired with the effectful context.
#[derive(Clone, Copy)]
pub struct MyHybridMolecule;

impl Molecule for MyHybridMolecule {
    type State = ((HotState, ColdState), Ctx);
    type Input = u32;
    type Output = u32;

    #[inline(always)]
    fn step(&self, state: &mut ((HotState, ColdState), Ctx), input: u32) -> u32 {
        // TODO: protocol logic over `hot` (every step) and `cold` (rare
        // events); `ctx` is the effectful context (rings, buffers,
        // counters). Never allocate here.
        let ((hot, cold), _ctx) = state;
        hot.sequence = hot.sequence.wrapping_add(1);
        // Cold fields are read rarely; referencing `peer_id` keeps the
        // field live in a fresh copy of the template.
        let _ = cold.peer_id;
        input.wrapping_add(hot.sequence)
    }
}

// The blanket impl makes MyHybridMolecule a HybridMolecule<(HotState,
// ColdState), Ctx>: `impl<Spure, Ctx, T: Molecule<State = (Spure, Ctx)>>
// HybridMolecule<Spure, Ctx> for T`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hybrid_molecule_steps() {
        let m = MyHybridMolecule;
        let mut state = (
            (HotState { sequence: 0 }, ColdState { peer_id: 7 }),
            Ctx,
        );
        assert_eq!(m.step(&mut state, 10), 11);
        assert_eq!(m.step(&mut state, 10), 12);
    }
}
