# FDS Sub-Project 1: Thesis Paper + Software Standard — Design Spec

**Date:** 2026-08-23
**Status:** Design presented in three parts and approved by the author, with amendments (arXiv max_results=200 per query; raw data staged in ~/tmp/; Rust-only tooling, no Python). This spec is the consolidated record.
**Author of spec:** Grok (xAI), for XENOT
**Applies to:** Sub-project 1 of the FDS roadmap. Later sub-projects (Atom/Molecule framework templates, build tooling + config.json, transport engine) are explicitly out of scope here and get their own spec → plan → implementation cycles.

---

## 1. Purpose

FDS ("Fast Data Transmission") is a Rust project for TCP/UDP/SCTP communication optimized to the silicon (latency/throughput), usable as the foundation for web servers, DNS, FTP, and custom protocols. This sub-project produces the two foundational documents that everything else is expressed in:

1. **A thesis-length academic paper** (LaTeX → PDF) formalizing the Atom/Molecule architecture — the symmetric monoidal category **Mol** of stateful transformations (deterministic mealy machines) — with ~50 provably true theorems, full proofs, and an arXiv-grounded bibliography.
2. **A curated software standard** (`standard.md`) for atoms, molecules, situations, and system-design decision matrices, inspired by `~/arcss.txt`, deliberately general and completely enforceable for any type of application, with policy nuance (scope + per-situation application masks) so enforcement is optimal per situation without overengineering.

## 2. Deliverables

| # | Deliverable | Path |
|---|-------------|------|
| 1 | Thesis paper sources + build script | `docs/paper/` (`thesis.tex`, `refs.bib`, `build.sh`) |
| 2 | Compiled PDF (≥ 30 pages, no images) | `docs/paper/thesis.pdf` (build artifact) |
| 3 | Proof-verification tooling (Rust, no Python) | `docs/paper/verify/` (cargo project) with PASS logs |
| 4 | Software standard | `docs/standard/standard.md` |
| 5 | arXiv staging data (removable) | `~/tmp/fds-arxiv/` (raw Atom XML + parsed dump) |
| 6 | Git repository for FDS | `git init` at repo root, spec committed |

## 3. Repo Layout

```
FDS/
├── name                      # existing ("FDS: Fast Data Transmission")
├── .gitignore                # target/, *.pdf, ~/tmp staging is outside repo
├── docs/
│   ├── superpowers/specs/    # this spec (committed)
│   ├── paper/
│   │   ├── thesis.tex        # main LaTeX document
│   │   ├── refs.bib          # curated bibliography (~40–60 entries)
│   │   ├── build.sh          # latexmk -pdf; --verify flag; no-images check
│   │   └── verify/           # Rust cargo project (proof-checking tools)
│   └── standard/
│       └── standard.md       # the software standard
```

Later sub-projects add `src/`, `build/`, `config/`; not created now.

## 4. Thesis Design

### 4.1 Identity
- **Title:** *Mol: The Category of Stateful Transformations — An Algebraic Theory of Zero-Allocation Dataplane Construction*
- **Author:** XENOT; date 2026. (Changeable; flagged for user confirmation.)
- **Framing:** synthesis + application thesis. Classical results restated and fully proven inside Mol are tagged **[C]** with provenance; statements original to this thesis are tagged **[N]** and proven with elementary machinery. No conjectures, no proof sketches presented as proofs.
- **Style:** pure text/math/tables. **No figures, no images, no `\includegraphics` anywhere** (user directive: "don't process images ever"). Engine: pdflatex via latexmk.

### 4.2 Section Outline (thesis-length, ~40 pp + appendices + bibliography)

1. Introduction — motivation; thesis statement; contributions (incl. NT1–NT52); the scope filter; roadmap.
2. Preliminaries — categories/functors/natural transformations; monoidal & symmetric monoidal; traced monoidal; monads, algebras over monads, Kleisli; coalgebras; Lawvere theories; operads, multicategories, clubs; effectus theory (partiality/recursion); linear logic & geometry of interaction (brief); accessible & locally presentable categories; universal algebra & clones.
3. The Category Mol — objects = types; morphisms = equivalence classes of pairs `(S, step)`, `step : S × A → B × S`, quotiented by the state-space bijection condition; composition, identity, associativity; symmetric monoidal structure (tensor = tuple, unit = `()`); PureMol, EffMol(Ctx), HybridMol; atoms; theorems NT1–NT8.
4. Lawvere Theories for Dataplane Components — ring/parser/transport theories; models in Mol; rewrite rules; NT9–NT12.
5. The Rewrite Engine — Gröbner/Knuth–Bendix completion of the runtime's equational theories; complete equational theory of the runtime; multicategories/clubs for typed composition trees; NT13–NT19 (NT15 computationally verified).
6. Free Molecules and Canonical Normal Forms — free symmetric monoidal category F(Σ); universal property; finiteness; normal forms; precomputed lookup table; NT20–NT24.
7. Enrichments and Quantitative Semantics — resource-sensitive monoidal categories; cost vectors (time, memory, cache misses); triangle inequality + additivity; global optimality; refinement poset; complexity closure; NT25–NT32.
8. Kleisli Adjunction and Pure/Effectful Decomposition — M = q ∘ e ∘ p, uniqueness, sliding/hoisting; NT33–NT37.
9. Coalgebraic Minimization — minimal realization, behavioral preservation, compositional bounds; NT38–NT41.
10. Traced Monoidal Structure, Feedback, Fixed-Iteration Loops — Joyal–Street–Verity; Banach contraction; iteration bound; Markov updates/Dobrushin; information-geometry coordinate selection as future work; NT42–NT45.
11. Operadic Composition and Batching — n-ary batch ops; reassociation; syscall amortization; ring capacity invariant; NT46–NT48.
12. Linear Logic, Cut Elimination, Zero Allocation — deforestation; GoI token interpretation; Curry–Howard remark (molecules as linear types); NT49–NT50.
13. Situations and Applications — situation-based modeling in mathematical notation: situation = (A→B, Φ, c̄); solution = molecule M with M ⊢ Φ and cost(M) ≤ c̄; decision matrices as formal optimization; lookup-table dispatch; NT51–NT52.
14. Testing Strategies — equational law testing, bisimulation, model-based, fuzzing, cost-annotated regression.
15. Related Work — all covered fields.
16. Conclusion — summary table, limitations, future work.
- Appendix A — proof-checking scripts and PASS outputs.
- Appendix B — optimization and testing catalogs as tables.

### 4.3 Theorem Catalog (NT1–NT52)

All 52 theorems listed below carry full proofs in the text. Tags: [N] = statement original to this thesis (proved with elementary means); [C] = classical result restated and fully proven inside Mol (provenance cited).

**Ch. 3 — Mol foundations (NT1–NT8)**
- NT1 [N] Mol is a category: composition of (S, step) classes is associative with identity.
- NT2 [N] The state-space bijection quotient is well-defined: equivalent machines compose to equivalent machines.
- NT3 [C] Mol is symmetric monoidal: tensor = tuple product, unit = (), coherent braiding (Mac Lane coherence).
- NT4 [N] PureMol is equivalent to the category of types and total functions.
- NT5 [N] EffMol(Ctx) is isomorphic to the Kleisli category of State Ctx (algebras over the monad).
- NT6 [N] Behavioral equivalence (bisimulation) is a congruence for ∘ and ⊗; Mol/≈ is a symmetric monoidal category.
- NT7 [N] PureMol and EffMol are closed under tensor.
- NT8 [N] Pure molecules and tensor products of pure molecules are deterministic; behavioral equivalence preserves determinism.

**Ch. 4 — Lawvere theories (NT9–NT12)**
- NT9 [N] Ring theory equations (push∘pop = id; pop∘push = id when non-empty) hold in every model and are complete for the free ring model.
- NT10 [N] Parser theory: sequence distributes over choice; choice associative; id laws; left factoring sound.
- NT11 [N] Transport theory: batch_recv = recv⊗⋯⊗recv; interchange law (f⊗g);(h⊗k) = (f;h)⊗(g;k) holds.
- NT12 [N] Ring cancellation: adjacent push∘pop is removable with behavior preserved.

**Ch. 5 — Rewrite engine (NT13–NT19)**
- NT13 [N] The ring rewrite system is terminating (lexicographic path measure).
- NT14 [N] It is locally confluent; all critical pairs join, hence confluent (Newman's lemma).
- NT15 [N] Knuth–Bendix completion of the ring theory terminates in a complete system (script-verified).
- NT16 [N] Parser left-factoring rewrites are confluent and terminating; unique left-factored normal forms.
- NT17 [N] Batch-flattening rewrites are confluent on batch trees; unique flattened normal form.
- NT18 [N] Normalization soundness: NF(M) ≈ M and cost(NF(M)) ≤ cost(M) when rules are cost-nonincreasing.
- NT19 [N] The full runtime equational theory is decidable for finite signatures via normalization.

**Ch. 6 — Free molecules (NT20–NT24)**
- NT20 [C] The free symmetric monoidal category F(Σ) on atom signature Σ exists (initial algebra; accessible/local presentability).
- NT21 [C] Universal property: every molecule over Σ factors uniquely (up to coherent iso) through F(Σ).
- NT22 [N] Finiteness: finite Σ with bounded types gives finite hom-sets in F(Σ).
- NT23 [N] Every molecule has a unique normal form in F(Σ) up to coherence.
- NT24 [N] Completeness: M ≈ M′ iff NF(M) = NF(M′); normal forms form a precomputable lookup table.

**Ch. 7 — Enrichments (NT25–NT32)**
- NT25 [N] Cost enrichment well-defined: additive composition cost, triangle inequality, tensor subadditivity.
- NT26 [N] Global optimality: nonnegative additive costs ⇒ min-cost molecules = shortest paths in the finite type graph (Dijkstra complete).
- NT27 [N] Fusion bound: c(fused g∘f) ≤ c(f)+c(g), strict when the intermediate type never materializes.
- NT28 [N] Refinement substitution: f ⊑ f′ ∧ g ⊑ g′ ⇒ g∘f ⊑ g′∘f′; behavior preserved, cost monotone.
- NT29 [N] Maximal refinement: finite refinement lattices have unique maximal element per behavior class; most-refined dispatcher sound and cost-minimal.
- NT30 [N] Complexity closure: atoms in a class C closed under ∘ and ⊗ yield molecules in C.
- NT31 [C] Descriptive complexity: finite-state molecules realize exactly the regular behaviors; minimal-state molecules realize minimal DFAs.
- NT32 [N] Projection reduction: if step depends on state only through π, then S_min ≅ π(S).

**Ch. 8 — Kleisli and trace (NT33–NT37)**
- NT33 [N] Every hybrid molecule decomposes M = q ∘ e ∘ p (p,q pure, e effectful), unique up to isomorphism.
- NT34 [C] Sliding: Tr(f;M) = f;Tr(M) for pure f (Joyal–Street–Verity).
- NT35 [C] Vanishing and yanking: Tr(Tr(M)) = Tr(M); Tr(σ) = id.
- NT36 [C] Superposing: Tr(M)⊗N = Tr(M⊗N).
- NT37 [N] Hoisting shrinks loop-state footprint; cache footprint of loop state monotone in state size.

**Ch. 9 — Coalgebraic minimization (NT38–NT41)**
- NT38 [C] Every finite-state molecule has a minimal state space, unique up to isomorphism (Nerode/Hopcroft).
- NT39 [C] Minimization preserves behavior (Nerode quotient equivalence).
- NT40 [N] Compositional state bound: |S_min(g∘f)| ≤ |S_f|·|S_g|; |S_min(f⊗g)| ≤ |S_f|·|S_g|.
- NT41 [N] Minimization compatible with equivalence: M ≈ M′ ⇒ S_min(M) ≅ S_min(M′).

**Ch. 10 — Fixed-iteration loops (NT42–NT45)**
- NT42 [C] Contraction state update (rate α < 1) converges to a unique fixed point (Banach).
- NT43 [N] Iteration bound: k(α, ε, d₀) = ⌈ln(ε(1−α)/d₀)/ln α⌉ iterations suffice; unrolled loop within ε, branch-predictable.
- NT44 [N] Contraction rates combine: max(α₁,α₂) under tensor; α₁+α₂ sequential (sup metric).
- NT45 [C] Markov updates: total-variation distance contracts by the Dobrushin coefficient.

**Ch. 11 — Operads and batching (NT46–NT48)**
- NT46 [N] Batch flattening: batch(batch(a,b),batch(c,d)) = batch(a,b,c,d).
- NT47 [N] Syscall amortization: cost(batch_n) ≤ n·cost(single) + fixed(n), fixed(n)/n → 0.
- NT48 [N] Ring capacity invariant: capacity 2^k with bitmask indexing correct iff in-flight ≤ 2^k − 1.

**Ch. 12 — Linear logic and zero copy (NT49–NT50)**
- NT49 [C] Cut elimination = deforestation: every pure pipeline fuses to a single morphism (Girard, proved for Mol).
- NT50 [N] Linearity: atoms consume input exactly once ⇒ well-typed hot path never allocates/deallocates.

**Ch. 13 — Situations (NT51–NT52)**
- NT51 [N] Situation solvability: bounded types + additive costs ⇒ feasible set of (A→B, Φ, c̄) finite and decidable; decision-matrix computation terminates.
- NT52 [N] Zero-copy roundtrip: parse/serialize inverse on well-formed domain ⇒ parse;serialize ≈ id; roundtrip elimination sound.

### 4.4 Proof-Checking Plan

- Every theorem: complete pencil-and-paper proof in the text; proofs are self-checked during authoring (each step justified).
- Computational claims verified by executable Rust tools (no Python anywhere), logs captured and included in Appendix A:
  - `kb_completion` — Knuth–Bendix completion of ring theory + critical-pair join check (NT15, supports NT13–14, NT19).
  - `normal_forms` — exhaustive normal-form enumeration on small signatures; finiteness + completeness checks (NT22–NT24).
  - `contraction` — NT43 bound arithmetic on sampled (α, ε, d₀).
  - `bisim` — exhaustive congruence check of behavioral equivalence on small finite machines (NT6).
  - `batch_amort` — NT47 cost bound on sample batch sizes.
- Machine-checked formalization in a proof assistant: noted as future work, not claimed.

## 5. Software Standard Design

### 5.1 Identity
- Path: `docs/standard/standard.md` (Markdown, readable like ARCSS).
- Relationship to ARCSS: *inspired by* — policies adopted policy-by-policy where they buy performance or security; no blind adoption; ARCSS remains the general standard; this standard is the Atom/Molecule-specific companion.
- Relationship to thesis: definitions reference thesis sections (NT numbers) instead of restating proofs.

### 5.2 Design Principles
1. **General policies.** A policy is never FDS-specific trivia ("enable TCP_QUICKACK"); it is a general statement any application type can satisfy and be checked against. FDS implementation details (socket-option catalogs, crate list, SIMD intrinsic notes) live in a non-normative appendix.
2. **Completely enforceable for any type of application.** Every policy's Enforcement names a universal mechanism — a lint/compiler check, a CI job that runs on any project, a property test harness, or a mandatory review checklist item — never one that depends on the application's type or framework.
3. **Nuance for optimal enforcement per situation.** Policies carry **Scope** (applicability predicate over situations), **Level** (MUST/SHOULD/MAY, possibly scope-qualified), and a **Situation Application Matrix** (per-situation default mask: which policies bind at which level; override only with written justification). Enforcement is complete (every situation has a determined interpretation) and optimal (only applicable policies bind, at the right strength).
4. **Integration guarantee.** Policies bind resources, invariants, and behavior — never application architecture. No mandated framework, runtime, or business-logic structure. Stated as policy class [INT], enforced by construction: any policy that would constrain non-hot-path application structure is out of scope.
5. **No overengineering.** Conformance is MUST/SHOULD/MAY + one waiver clause (documented deviation + compensating control, reviewed). No criticality-class ladder.

### 5.3 Conformance & Enforcement Model
- Levels: MUST / SHOULD / MAY, with scope qualification.
- Waiver: single clause, documented deviation + compensating control + reviewer.
- Enforcement: per-policy universal mechanism (automated where possible, procedural review otherwise).
- Situation application masks: default mapping by situation type (data-path, control-path, startup, shutdown, offline); project override with written justification.

### 5.4 Policy Catalog (13 categories)

Each policy: Statement / Rationale (cites NT where applicable) / Wrong / Correct / **Scope** / **Level** / Enforcement.

| Cat | General policy domain |
|-----|------------------------|
| [A] Atoms | Atoms are pure or effectful, have no hidden state, are total on their declared domain |
| [MOL] Molecules | Composition via ∘/⊗; hot paths avoid dynamic dispatch; dense-integer dispatch over chained conditionals |
| [R] Rings & buffers | Preallocation before the hot path; power-of-two ring capacity with bitmask indexing; bounded in-flight invariant (NT48) |
| [ALLOC] Allocation | Zero allocation in declared hot paths; statically sized or startup-preallocated collections; initialized memory |
| [CACHE] Cache & layout | Cache-line alignment for shared structures; hot/cold separation; false-sharing avoidance; layout justified by access pattern |
| [SIMD] SIMD | Vector ops never out of bounds; alignment requirements satisfied; feature-gated with fallback |
| [IO] Transport I/O | Nonblocking data-path I/O; drain-until-exhausted; batching to amortize syscalls; options justified by latency/throughput targets |
| [CONC] Concurrency | Per-core state preferred; no shared mutable state except lock-free primitives; orderings justified |
| [SEC] Security | Untrusted input length-validated before use; truncation detected; no uninitialized reads; resource caps declared; dependency audit |
| [OBS] Observability | Lock-free counters; pull-based metrics; zero-cost when compiled out |
| [PLUGIN] Plugins | ABI versioning; health checks; safe fallback |
| [TEST] Testing | Property-based law tests; fuzz for parsers/input; differential tests vs reference; cost/zero-alloc regression |
| [INT] Integration | The guarantee: policies never mandate application architecture |

### 5.5 Decision Matrices (D-1…D-12)

Each: inputs → decision rule → rationale (governing NT) → applicable policy level output (consistent with the situation masks).

- D-1 Ring/buffer sizing (L3-aware) · D-2 SoA vs AoS · D-3 Protocol selection (UDP/TCP/SCTP) · D-4 Batch size (NT47) · D-5 Polling: epoll busy-poll vs io_uring SQPOLL vs AF_XDP · D-6 Zero-copy vs copy (NT52) · D-7 Dispatch: monomorphization vs indirect · D-8 SIMD width · D-9 Lock-free vs locking · D-10 Hot/cold field split · D-11 Allocation policy · D-12 Plugin placement.

### 5.6 Situations

- Formal definition (matches thesis Ch. 13): situation = (A→B, Φ, c̄); solution = molecule M with M ⊢ Φ and cost(M) ≤ c̄; decision matrix = minimization over feasible set (finite/decidable by NT51).
- Canonical recipes as worked applications of the general policies: web server (HTTP/TCP), DNS (UDP+TCP), FTP (control + data), custom protocol.

### 5.7 Document Layout

Preamble → Vocabulary (math notation, referencing thesis) → Conformance & enforcement model (incl. nuance mechanism) → Policy catalog → Decision matrices → Situations → Integration guarantee → Glossary → Appendix: implementation guidance (non-normative: socket options, crates, SIMD notes).

## 6. Bibliography Plan

### 6.1 arXiv acquisition (per user directive)
- Endpoint: `http://export.arxiv.org/api/query` with `search_query`, `start`, `max_results`.
- **max_results=200 per query**; all returned entries are read by the authoring process.
- **3-second delay between consecutive queries** (arXiv documented limit; never exceeded).
- **12 query clusters**, terms OR-combined for maximum recall per query:
  1. traced monoidal categories
  2. Lawvere theories / algebraic theories
  3. monads, Kleisli categories, state monad
  4. coalgebra, automata minimization
  5. operads, multicategories, clubs
  6. linear logic, geometry of interaction
  7. term rewriting, Knuth–Bendix, Gröbner bases
  8. effectus theory
  9. Markov categories, categorical probability
  10. sequential machines, mealy machines
  11. implicit complexity, descriptive complexity
  12. high-performance networking, zero-copy, dataplane
- **Staging:** raw Atom XML + parsed dump saved under `~/tmp/fds-arxiv/` (outside the repo; removable after the bibliography is finalized).
- **Tooling: Rust only** (a small cargo binary: `arxiv-fetch` for querying/rate-limiting/downloading, `arxiv-parse` for Atom XML → metadata extraction → candidate `.bib` entries). **No Python anywhere in the project.**

### 6.2 Curation
- From the ~12×200 raw results, curate ~40–60 entries actually cited in the thesis into `docs/paper/refs.bib`.
- Classical sources not on arXiv added manually: Mac Lane (Categories for the Working Mathematician); Joyal–Street–Verity (Traced monoidal categories, 1996); Lawvere (Functorial semantics, 1963); Girard (Linear logic, 1987); Wadler (Theorems for free!, 1989); Brzozowski (Derivatives of regular expressions, 1964); Hopcroft (An n log n algorithm for minimizing states, 1971); Adámek–Rosický (Locally Presentable and Accessible Categories).

## 7. Build & Verification

- `docs/paper/build.sh`: `latexmk -pdf` (pdflatex), auto-reruns; exits non-zero on LaTeX error; `--verify` also runs the Rust proof-checking tools and the no-images check.
- **No images:** `build.sh` asserts zero `\includegraphics` in sources (grep check, CI-style).
- **Gates:** pdflatex zero errors; page count ≥ 30; no undefined references; all verify tools print PASS; logs included in Appendix A of the thesis.

## 8. Non-Goals (later sub-projects)

- Atom/Molecule framework templates (.rs) — sub-project 2 (spec → plan → implementation to follow).
- Build tooling (hardware-adaptive bash) + config.json — sub-project 3.
- Transport engine (TCP/UDP/SCTP dataplane) — sub-project 4.
- The original prompt's full performance-technique catalog (recvmmsg, io_uring, AF_XDP, affinity, etc.) is captured in the standard's non-normative implementation appendix and decision matrices, but the engine itself is not built here.

## 9. Constraints & Conventions

- No Python anywhere in the project; tooling in Rust (Julia only if Rust proves impractical for a given task).
- arXiv API: ≤ 1 query per 3 seconds; max_results ≤ 2000 per slice (we use 200); batching to maximize information per query.
- No images in the paper, ever.
- All 52 theorems must be provably true with full proofs; computational claims script-verified; no conjecture presented as theorem.
- User directives override skill defaults (per using-superpowers).

## 10. Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| A theorem in the catalog turns out unprovable as stated | Full proofs are written before finalization; any theorem that cannot be proven as stated is weakened/stated precisely or moved to future work — never shipped as a sketch. Catalog says "only provably true". |
| arXiv rate limiting / transient failures | 3s pacing, retry with backoff, staged raw XML so refetching isn't needed. |
| pdflatex page target (≥30) not met | Outline budgeted ~40pp; appendix tables and proofs fill out; adjust section depth. |
| Standard too prescriptive for apps (overengineering) | [INT] integration guarantee + scope/mask nuance mechanism; policy-by-policy adoption review against ARCSS. |
| Verify tooling in Rust is a pain | Julia permitted as fallback by user; no Python. |
