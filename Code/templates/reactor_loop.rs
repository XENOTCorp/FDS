//! Template: a REACTOR LOOP: drain the ingress ring through a molecule
//! into the egress ring (thesis ch. 10: delayed feedback, not a JSV
//! trace; yanking fails, `Tr(σ) = Δ`).
//!
//! Copy this file and adapt. The loop is a fixed, branch-predictable
//! drain: pull one item, step the molecule, push the output. In a
//! real dataplane this is driven by an edge-triggered epoll drain to
//! EAGAIN (sub-project 4); the template shows the per-core ring wiring.

use mol::{Molecule, SpscRing};

/// A per-core reactor: one molecule, one ingress ring, one egress ring.
pub struct Reactor<M, const CAP: usize>
where
    M: Molecule<Input = u32, Output = u32>,
{
    pub molecule: M,
    pub ingress: SpscRing<u32, CAP>,
    pub egress: SpscRing<u32, CAP>,
    state: M::State,
}

impl<M, const CAP: usize> Reactor<M, CAP>
where
    M: Molecule<Input = u32, Output = u32>,
    M::State: Default,
{
    /// A reactor with empty rings and a default state.
    pub fn new(molecule: M) -> Self {
        Reactor {
            molecule,
            ingress: SpscRing::new(),
            egress: SpscRing::new(),
            state: M::State::default(),
        }
    }

    /// Drain the ingress ring, stepping the molecule once per item.
    /// Returns the number of items processed. Non-blocking; returns
    /// immediately when the ring is empty (drain-to-EAGAIN discipline,
    /// standard [IO]).
    pub fn poll_once(&mut self) -> usize {
        let mut processed = 0;
        while let Some(input) = self.ingress.try_pop() {
            let output = self.molecule.step(&mut self.state, input);
            // If egress is full, drop is not allowed for a reliable
            // transport: the caller should backpressure instead. The
            // template records the failure by leaving the item out.
            let _ = self.egress.try_push(output);
            processed += 1;
        }
        processed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trivial counter molecule for the template test.
    pub struct Counter;
    impl Molecule for Counter {
        type State = u32;
        type Input = u32;
        type Output = u32;

        #[inline(always)]
        fn step(&self, state: &mut u32, input: u32) -> u32 {
            *state += 1;
            input + *state
        }
    }

    #[test]
    fn reactor_drains_ingress() {
        let mut reactor = Reactor::<Counter, 8>::new(Counter);
        for i in 0..5 {
            assert!(reactor.ingress.try_push(i).is_ok());
        }
        let processed = reactor.poll_once();
        assert_eq!(processed, 5);
        assert_eq!(reactor.egress.try_pop(), Some(1));
        assert_eq!(reactor.egress.try_pop(), Some(3));
    }
}
