//! Mol: the category of stateful transformations.
//!
//! Objects are types. Morphisms are equivalence classes of pairs
//! `(S, step)` with `step : S × A → B × S` (deterministic Mealy
//! machines), quotiented by state-space bijection. Sequential Mol
//! composition `then(m, n)` is the pipeline `m; n` (categorical
//! `n ∘ m`) and products the states. Tensor `par` is `⊗`.
//!
//! Classes, read off any representative's state space (residual
//! hybrid): PureMol when `S ≅ ()`; EffMol(Ctx) when `S ≅ Ctx`, with
//! Kleisli composition threading one context; HybridMol otherwise.
//! Mol composition of two effectful machines has state `Ctx × Ctx`
//! and is hybrid (`E ∘ E ⊆ H`). Kleisli composition is a different
//! operator (`E ∘_K E ⊆ E`).
//!
//! FIFO rings obey occupancy `0 ≤ w − r ≤ 2^k − 1` under bitmask
//! indexing, not the LIFO equations. Stacks obey `push; pop = id`.
//! The reactor combinator is delayed feedback: `Tr(σ) = Δ`, so
//! yanking fails.
//!
//! Discipline (standard \[A\], \[MOL\], \[R\], \[ALLOC\], \[CACHE\],
//! \[CONC\], \[TEST\]): zero allocation in declared hot paths;
//! power-of-two lock-free rings with the in-flight occupancy bound;
//! cache-line-aligned shared structures with hot/cold separation;
//! per-core state; no shared mutable state except lock-free rings.

pub mod atom;
pub mod buffer;
pub mod compose;
pub mod feedback;
pub mod layout;
pub mod mem;
pub mod molecule;
pub mod ring;
pub mod simd;
pub mod stack;

pub use atom::{Atom, EffectfulAtom, PureAtom};
pub use buffer::{Buffer, Pool, PoolGuard, SetLenError};
pub use compose::{kleisli_then, par, then, Compose, KleisliCompose, Par};
pub use feedback::{delay, fixed_iter, tr, Delay, FixedIter, Tr};
pub use layout::{cache_line_size, CachePadded, HotCold, PaddedCounter};
pub use mem::{huge_page, leak_box, zeroed, HugePageGuard};
pub use molecule::{EffectfulMolecule, HybridMolecule, Molecule, PureFn, PureMolecule};
pub use ring::{MpmcRing, SpscRing};
pub use simd::{checksum_finalize, sum_u16, sum_u16_scalar, u16_checksum};
pub use stack::Stack;
