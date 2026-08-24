# FDS Sub-Project 2: Atom/Molecule Framework — Design Spec

**Date:** 2026-08-23
**Status:** Implemented and merged 2026-08-24 (design-approval loop completed; the four open decision points in §7 were answered by the author and are recorded there)
**Depends on:** Sub-project 1 (thesis defines Mol; standard defines policies [A], [MOL], [R], [ALLOC], [CACHE], [SIMD], [CONC], [TEST])
**Later sub-projects:** 3 (build tooling + config.json), 4 (transport engine) consume this framework.

---

## 1. Purpose

The Rust implementation of the Mol architecture from the thesis: a library crate `mol-core` (lib name `mol`) providing atoms, molecules, composition combinators, rings, buffers, and memory layout discipline — plus authoring templates (`.rs` files) for new atoms and molecules. Everything is expressed in the thesis's vocabulary: objects are types, morphisms are equivalence classes of `(S, step)` with `step : S × A → B × S`, composed by ∘ (sequential) and ⊗ (parallel), with PureMol / EffMol(Ctx) / HybridMol subcategories (NT1–NT8).

## 2. Deliverables

| # | Deliverable | Path |
|---|-------------|------|
| 1 | Workspace + core library crate | `Cargo.toml`, `crates/mol-core/` |
| 2 | Atom/molecule traits & combinators | `crates/mol-core/src/` |
| 3 | Authoring templates (.rs files) | `templates/` |
| 4 | Lock-free rings, buffers, event arrays | `crates/mol-core/src/ring.rs`, `buffer.rs` |
| 5 | Memory layer (hugepages, globals, alignment) | `crates/mol-core/src/mem.rs` |
| 6 | Baseline build config | `.cargo/config.toml` (adaptive overrides come from sub-project 3) |
| 7 | Law tests, static asserts, zero-alloc tests | `crates/mol-core/tests/` |

## 3. Repo Layout (additions)

```
FDS/
├── Cargo.toml                     # workspace
├── .cargo/config.toml             # baseline profile flags
├── crates/mol-core/               # package mol-core, lib name mol
│   ├── src/
│   │   ├── lib.rs                 # trait re-exports
│   │   ├── atom.rs                # Atom, PureAtom, EffectfulAtom<Ctx>
│   │   ├── molecule.rs            # Molecule, Pure/Effectful/HybridMolecule, PureFn
│   │   ├── compose.rs             # then (∘), par (⊗), arrays for ⊗ⁿ
│   │   ├── ring.rs                # SPSC/MPMC lock-free power-of-two rings
│   │   ├── buffer.rs              # Buffer, Pool (lock-free arena), PoolGuard
│   │   ├── mem.rs                 # hugepage mmap, Box::leak/OnceLock, zeroed init
│   │   ├── layout.rs              # alignment helpers, hot/cold split, padded atomics
│   │   └── simd.rs                # bounds-safe SIMD helpers (checksums, batch ops)
│   └── tests/
│       ├── laws.rs                # property-style law tests (NT1/NT3/NT8/NT9/NT11/NT12/NT46)
│       ├── static_asserts.rs      # const size/alignment assertions
│       ├── zero_alloc.rs          # counting-allocator hot-path probe
│       └── templates.rs           # compiles each template verbatim (`#[path]`)
└── templates/
    ├── pure_atom.rs
    ├── effectful_atom.rs
    ├── hybrid_molecule.rs
    └── reactor_loop.rs
```

Note: the planned `ctx.rs` module was dropped — the context is a generic parameter `Ctx` (author decision §7.2), so no separate trait/module is needed.

## 4. Design

### 4.1 Traits (monomorphized; no `dyn` in hot paths)

- `trait Atom { type Input; type Output; }` with marker sub-traits `PureAtom` (no state) and `EffectfulAtom<Ctx>` (state = Ctx, generic over the application's context type).
- `trait Molecule { type State: 'static; type Input; type Output; fn step(&self, &mut State, Input) -> Output; }` — the mealy step `S × A → B × S` realized as in-place state mutation plus a returned output; `State: 'static` so states live in preallocated per-core structures.
- `PureMolecule` (State = ()), `EffectfulMolecule<Ctx>` (State = Ctx), `HybridMolecule<Spure, Ctx>` (State = (Spure, Ctx)) — matching the paper's subcategories, via blanket impls.
- Combinators: `then(a, b)` = sequential composition ∘ (NT1 associativity); `par(a, b)` = ⊗ (NT3); `[M; N]` arrays for parallel ⊗ⁿ (requires `Input`/`Output: Copy`); normalization soundness via NT18 (compose then normalize by the rewrite rules). `PureFn<F>` wraps a closure as a molecule (state = ()); closures themselves are not blanket-implemented as atoms (E0207: unconstrained type params), so applications define concrete atom types.

### 4.2 Rings & buffers

- Power-of-two capacity with bitmask indexing; SPSC invariant in-flight ≤ capacity − 1 (NT48); MPMC is the Vyukov bounded queue (holds CAP items, sequence-number epochs); drain-to-exhausted semantics (standard [R]).
- All buffers/rings/event arrays/connection state preallocated at startup; `Box::leak`/`OnceLock` for globals; `heapless`/`arrayvec`/`static_assertions` accepted as dependencies (author decision §7.4); **no `Vec`/`String`/`format!` in hot paths** (standard [ALLOC]).
- Memory initialized through `MaybeUninit` with explicit writes; no uninitialized reads (standard [SEC]).

### 4.3 Memory & layout

- `huge_page` = private anonymous `mmap` advised with `MADV_HUGEPAGE` (2 MiB THP pages when the kernel provides them; never required; graceful failure → `None`).
- Shared structures `#[repr(align(64))]`; hot and cold connection fields in separate cache lines; frequently written counters padded to their own cache lines (false-sharing avoidance); `#[repr(C)]` for `HotCold`; `static_assertions::const_assert_eq!` on sizes/alignments at compile time (standard [CACHE]).
- Per-core data structures preferred; no shared mutable state between threads except lock-free rings (standard [CONC]).

### 4.4 SIMD helpers

- Bounds discipline first: every vector operation operates on `chunks_exact` slices only — never past the slice end — with a scalar remainder loop; unaligned loads via `_mm256_loadu` (standard [SIMD]).
- AVX2 fast path (`sum_u16_avx2`, two `_mm256_sad_epu8` even/odd passes) gated by `is_x86_feature_detected!("avx2")`, with a scalar portable fallback (`sum_u16_scalar`) and RFC 1071 `checksum_finalize`. No external SIMD crate.

### 4.5 Baseline build config

- Workspace `Cargo.toml`: release `opt-level=3`, `lto=fat`, `codegen-units=1`, `panic=abort`, `overflow-checks=false`, `debug-assertions=false`; dev `overflow-checks=true`, `debug-assertions=true`.
- `.cargo/config.toml`: `relocation-model=pic` (portable baseline).
- `target-cpu=native` and hardware-specific `target_feature` sets come from sub-project 3's adaptive build script.
- MSRV pinned to 1.97.1 (`rust-version` in the workspace; author decision §7.3).

### 4.6 Templates

Four authoring templates as `.rs` files (pure atom, effectful atom, hybrid molecule, reactor loop), each: doc header with the Mol definition it implements, hot/cold state layout, step function skeleton with bounds/init discipline, and a unit test. The hybrid template's state is `((HotState, ColdState), Ctx)` so the hot/cold split is real (both structs live in the state). Templates are compile-verified verbatim by `tests/templates.rs`.

### 4.7 Testing

- Law tests (property-style, deterministic sweeps — no external RNG): `then` associativity (NT1), interchange with stateful molecules (NT11, states agree up to the canonical reassociation), tensor symmetry up to swap (NT3), batch element independence (NT46), determinism (NT8), pipeline equations (NT9), SPSC/MPMC FIFO across wraparound (NT12). `crates/mol-core/tests/laws.rs`.
- `static_assertions` on sizes/alignments of every shared type (`tests/static_asserts.rs`).
- Zero-allocation hot-path probe: counting `GlobalAlloc`; the full reactor pipeline (ingress → molecule → egress, checksums, pool alloc/return, MPMC, array/tensor, closure carrier) must not allocate (`tests/zero_alloc.rs`).
- Busy-wait threaded stress tests are `#[ignore]`d: the default suite must stay fast (hard budget: whole suite ≤ 5 min; currently ~0.15 s); run them explicitly with `cargo test -p mol-core -- --ignored --test-threads=1`.
- Sanitizer/fuzz jobs deferred: no parser atoms exist until sub-project 4, and no CI config lives in this repo yet.

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

## 7. Design Decisions (author rulings, 2026-08-23)

1. **Crate name: `mol-core`** (lib name `mol`). The pure Mol framework gets its own crate; the FDS crate for tcpip/udp/sctp (sub-project 4) consumes it and the templates.
2. **Context is a generic parameter**: `EffectfulAtom<Ctx>`, `EffectfulMolecule<Ctx>`, `HybridMolecule<Spure, Ctx>`. No fixed context trait/module.
3. **MSRV pinned to 1.97.1** (the toolchain on the machine; `rust-version` in the workspace manifest).
4. **`heapless`, `arrayvec`, `static_assertions` accepted** as dependencies (zero-dependency was offered but rejected).

## 8. Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| Trait design fights the compiler's fusion (NT1–NT4 optimizations) | Monomorphization-first design; law-test pipelines; zero-alloc probe asserts no heap traffic |
| Hugepages unavailable | Runtime detection with normal-page fallback (`huge_page` → `None`); config flag (D-11) |
| Lock-free ring subtlety (memory ordering bugs) | SPSC/MPMC proofs per NT48; threaded stress tests (`#[ignore]`, serial); minimal orderings (Relaxed/Acquire/Release) |
| Over-abstraction creeps in | Standard [INT]: no framework mandates; templates show the minimal pattern |
