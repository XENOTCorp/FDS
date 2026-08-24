# FDS Sub-Project 2: Atom/Molecule Framework — Design Spec

**Date:** 2026-08-23
**Status:** Draft for review (written in advance per author request; design-approval loop still applies before implementation)
**Depends on:** Sub-project 1 (thesis defines Mol; standard defines policies [A], [MOL], [R], [ALLOC], [CACHE], [SIMD], [CONC], [TEST])
**Later sub-projects:** 3 (build tooling + config.json), 4 (transport engine) consume this framework.

---

## 1. Purpose

The Rust implementation of the Mol architecture from the thesis: a library crate `fds-core` providing atoms, molecules, composition combinators, rings, buffers, and memory layout discipline — plus authoring templates (`.rs` files) for new atoms and molecules. Everything is expressed in the thesis's vocabulary: objects are types, morphisms are equivalence classes of `(S, step)` with `step : S × A → B × S`, composed by ∘ (sequential) and ⊗ (parallel), with PureMol / EffMol(Ctx) / HybridMol subcategories (NT1–NT8).

## 2. Deliverables

| # | Deliverable | Path |
|---|-------------|------|
| 1 | Workspace + core library crate | `Cargo.toml`, `crates/fds-core/` |
| 2 | Atom/molecule traits & combinators | `crates/fds-core/src/` |
| 3 | Authoring templates (.rs files) | `templates/` |
| 4 | Lock-free rings, buffers, event arrays | `crates/fds-core/src/ring.rs`, `buffer.rs` |
| 5 | Memory layer (hugepages, globals, alignment) | `crates/fds-core/src/mem.rs` |
| 6 | Baseline build config | `.cargo/config.toml` (adaptive overrides come from sub-project 3) |
| 7 | Law tests, static asserts, zero-alloc tests | `crates/fds-core/tests/` |

## 3. Repo Layout (additions)

```
FDS/
├── Cargo.toml                     # workspace
├── .cargo/config.toml             # baseline profile flags
├── crates/fds-core/
│   ├── src/
│   │   ├── lib.rs                 # trait re-exports
│   │   ├── atom.rs                # PureAtom, EffectfulAtom
│   │   ├── molecule.rs            # Pure/Effectful/HybridMolecule, step types
│   │   ├── compose.rs             # then (∘), par (⊗), arrays for ⊗ⁿ
│   │   ├── ctx.rs                 # Ctx trait (preallocated runtime context)
│   │   ├── ring.rs                # SPSC/MPMC lock-free power-of-two rings
│   │   ├── buffer.rs              # preallocated packet/message buffers
│   │   ├── mem.rs                 # hugepage mmap, Box::leak/OnceLock, zeroed init
│   │   ├── layout.rs              # alignment helpers, hot/cold split, padded atomics
│   │   └── simd.rs                # bounds-safe SIMD helpers (checksums, batch ops)
│   └── tests/
│       ├── laws.rs                # property-based law tests (NT9–NT12, NT46)
│       ├── static_asserts.rs
│       └── zero_alloc.rs          # counting-allocator hot-path probe
└── templates/
    ├── pure_atom.rs
    ├── effectful_atom.rs
    ├── hybrid_molecule.rs
    └── reactor_loop.rs
```

## 4. Design

### 4.1 Traits (monomorphized; no `dyn` in hot paths)

- `trait Atom { type Input; type Output; }` with marker sub-traits `PureAtom` (no state) and `EffectfulAtom` (state = Ctx).
- `trait Molecule { type S; type A; type B; fn step(&mut S, A) -> (B, S); }` — the mealy step `S × A → B × S`; `S: 'static + Copy` for hot-path types (enforced with `PhantomData` where needed; see NT50 linearity).
- `PureMolecule` (S = ()), `EffectfulMolecule` (S = Ctx), `HybridMolecule` (S = S_pure × Ctx) — matching the paper's subcategories.
- Combinators: `then(a, b)` = sequential composition ∘ (NT1 associativity); `par(a, b)` = ⊗ (NT3); `[M; N]` arrays for parallel ⊗ⁿ; normalization soundness via NT18 (compose then normalize by the rewrite rules).

### 4.2 Rings & buffers

- Power-of-two capacity with bitmask indexing; invariant in-flight ≤ capacity − 1 (NT48); SPSC and MPMC lock-free variants; drain-to-exhausted semantics (standard [R]).
- All buffers/rings/event arrays/connection state preallocated at startup; `Box::leak` or `OnceLock` for globals; `heapless`/`arrayvec` collections; **no `Vec`/`String`/`format!` in hot paths** (enforced by lint config; standard [ALLOC]).
- Memory initialized with `MaybeUninit::zeroed()` then `assume_init()` only after full initialization (standard [SEC]: no uninitialized reads).

### 4.3 Memory & layout

- `mmap` with `MAP_HUGETLB` and `madvise(MADV_HUGEPAGE)` for large shared buffers, with graceful fallback to normal pages when hugepages are unavailable (runtime detection; decision matrix D-11).
- Shared structures `#[repr(align(64))]`; hot and cold connection fields in separate cache lines; frequently written counters padded to their own cache lines (false-sharing avoidance); `#[repr(C)]` where a struct crosses the C ABI; `static_assertions::const_assert_eq!` on sizes/alignments at compile time (standard [CACHE]).
- Per-core data structures preferred; no shared mutable state between threads except lock-free rings (standard [CONC]).

### 4.4 SIMD helpers

- Bounds discipline first: every vector operation operates on slices whose length and alignment are checked before the vectorized loop; masked/remainder handling never reads or writes out of bounds (standard [SIMD]).
- Portable SIMD (`wide`) with `std::arch` native fallback; SoA layout for SIMD-heavy field transforms, AoS for per-packet processing (standard [CACHE]).

### 4.5 Baseline build config (`.cargo/config.toml`)

- release: `opt-level=3`, `lto=fat`, `codegen-units=1`, `panic=abort`, `overflow-checks=false`, `debug-assertions=false`, `relocation-model=pic`, static linking target flags.
- dev: `overflow-checks=true`, `debug-assertions=true`.
- `target-cpu=native` and hardware-specific `target_feature` sets come from sub-project 3's adaptive build script; this file is the portable baseline.

### 4.6 Templates

Four authoring templates as `.rs` files (pure atom, effectful atom, hybrid molecule, reactor loop), each: doc header with the Mol definition it implements, state layout with hot/cold split, step function skeleton with bounds/init discipline, compile-time asserts, and the law tests to copy.

### 4.7 Testing

- Law tests (property-based): ring equations (NT9, NT12), parser left-factoring (NT10), interchange (NT11), batch flattening (NT46) — the equational testing strategy from the thesis Ch. 14.
- `static_assertions` on every shared type.
- Zero-allocation hot-path probe: counting allocator in a test build; any allocation in a declared hot path fails the test.
- Sanitizer job (miri/ASan/UBSan where toolchain allows) in CI; fuzz targets for any parser atoms.

## 5. Constraints

- No Python anywhere; Rust only.
- No OOB SIMD; alignment verified at compile time where possible, at runtime otherwise.
- Theorems are the rationale: trait/ring/buffer design must not contradict NT1–NT52.
- Conforms to standard policies [A], [MOL], [R], [ALLOC], [CACHE], [SIMD], [CONC], [TEST].

## 6. Non-Goals

- Transport engine (sub-project 4): sockets, epoll, recvmmsg etc. not here.
- Adaptive build script + config.json (sub-project 3).
- Plugins/ABI (later sub-project).
- Writing the paper/standard (sub-project 1, done).

## 7. Open Decision Points (for author)

1. Crate name: `fds-core` vs `atomos-core` vs other.
2. Whether `EffectfulAtom` state should be `Ctx` by default or generic over a context type parameter.
3. MSRV (minimum supported Rust version) — 1.97.1 present on machine; pinning policy.
4. Whether `heapless`/`arrayvec` are acceptable dependencies or zero-dependency preferred.

## 8. Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| Trait design fights the compiler's fusion (NT1–NT4 optimizations) | Monomorphization-first design; benchmark law-test pipelines; inspect `cargo asm` in review |
| Hugepages unavailable | Runtime detection with normal-page fallback; config flag (D-11) |
| Lock-free ring subtlety (memory ordering bugs) | SPSC/MPMC proofs per NT48; loom-style concurrency tests; minimal orderings (Relaxed/Acquire/Release) |
| Over-abstraction creeps in | Standard [INT]: no framework mandates; templates show the minimal pattern |
