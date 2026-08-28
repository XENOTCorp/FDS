//! Mol: the category of stateful transformations.
//!
//! Implements the framework of the FDS thesis: objects are types, morphisms
//! are equivalence classes of pairs `(S, step)` with `step : S × A → B × S`
//! (deterministic mealy machines), composed by `∘` (sequential) and
//! `⊗` (tensor), with the subcategories PureMol, EffMol(Ctx), HybridMol
//!.
//!
//! Discipline (FDS standard policies \[A\], \[MOL\], \[R\], \[ALLOC\], \[CACHE\],
//! \[CONC\], \[TEST\]): zero allocation in declared hot paths; power-of-two
//! lock-free rings with the in-flight ≤ capacity − 1 invariant;
//! cache-line-aligned shared structures with hot/cold separation;
//! per-core state; no shared mutable state except lock-free rings.

pub mod atom;
pub mod buffer;
pub mod compose;
pub mod layout;
pub mod mem;
pub mod molecule;
pub mod ring;
pub mod simd;

pub use atom::{Atom, EffectfulAtom, PureAtom};
pub use buffer::{Buffer, Pool, PoolGuard, SetLenError};
pub use compose::{par, then, Compose, Par};
pub use layout::{CachePadded, HotCold, PaddedCounter, cache_line_size};
pub use mem::{HugePageGuard, huge_page, leak_box, zeroed};
pub use molecule::{EffectfulMolecule, HybridMolecule, Molecule, PureFn, PureMolecule};
pub use ring::{MpmcRing, SpscRing};
pub use simd::{checksum_finalize, sum_u16, sum_u16_scalar, u16_checksum};
