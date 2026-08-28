# FDS STANDARD: Atoms, Molecules, Situations

**A Curated Prescriptive Standard for the Atom/Molecule Architecture**

August 2026 / XENOT

**Contents**

1. Preamble
2. Vocabulary
3. Conformance and Enforcement Model
4. The Nuance Mechanism
5. Policy Catalog
6. Decision Matrices D-1 … D-12
7. Situations
8. Integration Guarantee
9. Glossary
10. Appendix: Implementation Guidance (Non-Normative)

---

## 1. Preamble

**Purpose.** This standard governs the construction of software in the Atom/Molecule architecture: systems built from *atoms* (minimal pure or effectful transformations), assembled into *molecules* (stateful transformations with an explicit state space), deployed to satisfy *situations* (a typed interface, a behavioral specification, and a cost budget). It is the engineering constitution of FDS ("Fast Data Transmission"), a Rust project for TCP/UDP/SCTP communication optimized to the silicon (latency- and throughput-first) and it is the companion discipline for any application built on FDS: web servers, DNS, FTP, custom protocols, or anything else.

The standard is deliberately **curated**: it adopts a policy only when the policy buys performance or security for the architecture, and it avoids overengineering: there is no criticality-class ladder, no Level-III formal machinery as a general requirement, and no lifecycle or style machinery beyond what the data plane needs. The standard is deliberately **general**: no policy names a socket option, a crate, a framework, or a language-internal feature; policies are stated so that any type of application can satisfy them and be checked against them. FDS-specific implementation material (socket-option catalogs, candidate crates, SIMD intrinsic notes) lives exclusively in the non-normative Appendix (Section 10). The standard is deliberately **enforceable**: every policy names an enforcement mechanism that is universal (a lint or compiler check, a continuous-integration job, a property-test harness, or a mandatory review item), and the mechanism runs for any application type regardless of its domain or structure.

**Relationship to ARCSS.** This standard is inspired by *ARCSS: Affine Resource Constrained Software Standard* (XENOT, August 2026), the author's exhaustive prescriptive standard for verifiable, resource-bounded software construction. The relationship is **policy-by-policy adoption, not blind copying**. ARCSS remains the general standard for this project and for all application code that is not governed by this document. This standard adopts from ARCSS those disciplines that directly serve the Atom/Molecule architecture's guarantees (purity and effect isolation, hard resource boundedness, locality and layout discipline, input validation at trust boundaries, verification by property tests, and supply-chain integrity) and deliberately declines the rest: the C0–C3 criticality ladder (replaced by one level system with nuance), energy and wear accounting, cache-oblivious preference, secure deletion, information-flow control, formal Level-III proof gates, semantic-versioning migration windows, and general naming/documentation policies all remain ARCSS's province. Where this standard is silent, ARCSS applies. Where this standard binds a policy, it binds at one of three levels (MUST/SHOULD/MAY); nuance is carried by per-policy scope predicates and per-situation application masks (Section 4), never by a class ladder. A project conforming to this standard must still conform to ARCSS for everything this standard does not govern; where the two documents both address a concern, this standard's statement governs the concern for the situations it covers, and ARCSS governs the rest.

**Relationship to the thesis.** Definitions in this standard reference the thesis *Mol: The Category of Stateful Transformations. An Algebraic Theory of Zero-Allocation Dataplane Construction* (XENOT, 2026; `Docs/paper/thesis.pdf`), which states and proves the theorems cited throughout this document as **the category axioms–the zero-copy roundtrip**. This standard is normative for engineering; the thesis is normative for mathematics. Every claim of the form "by the ring-capacity invariant" means: the thesis proves the theorem, and this standard relies on it. This standard never restates proofs, and it never weakens a theorem into a hope: a policy cites a theorem only where the theorem genuinely grounds the policy's rationale. If any citation appears doubtful to a reader, the thesis is the authority for the mathematics and this standard is the authority for the engineering obligation.

---

## 2. Vocabulary

Terms used in this document have the meanings fixed here, which agree with the thesis. Mathematical notation is informal but faithful to the thesis's treatment.

**Types.** A type A is the carrier of the values a computation accepts or produces: an *object* in the categorical vocabulary of the thesis. A transformation is written `f : A → B` (a morphism from A to B).

**Atoms.** An *atom* is a transformation with no internal composition: the smallest unit out of which data paths are built. Two kinds exist. A **pure atom** is a total function `f : A → B`: same input, same output, no observable effect (thesis the pure-molecules-as-functions theorem: PureMol is equivalent to the category of types and total functions). An **effectful atom** is an arrow of EffMol(Ctx), the Kleisli category of the state monad over an effect context Ctx (the Kleisli characterization): it computes `f : A → B` while touching exactly the capabilities that Ctx names (I/O handles, clocks, allocation) and nothing else. An atom's contract declares its input domain (a subset of A, possibly all of A), its output type B, and (if effectful) its context Ctx.

**State space and step function.** A stateful transformation is a pair `(S, step)` with `step : S × A → B × S` (the category axioms): given state `s ∈ S` and input `a ∈ A`, it produces output `b ∈ B` and next state `s′ ∈ S`. The state space S is the complete, declared memory of the transformation; there is no other state (policy A-02).

**Molecules.** A *molecule* is a stateful transformation `M : A → B`, i.e., a morphism of the category Mol: formally, an equivalence class of pairs `(S, step)` quotiented by the state-space bijection condition (the bijection-quotient theorem). A **pure molecule** lives in PureMol and computes without effects; pure molecules and tensor products of pure molecules are deterministic (the pure-molecules-as-functions theorem, the determinism theorem). An **effectful molecule** acts within a context Ctx (the Kleisli characterization). A **hybrid molecule** mixes pure and effectful steps; every hybrid molecule decomposes uniquely as `M = q ∘ e ∘ p`, where `p` and `q` are pure and `e` is effectful (Kleisli decomposition). Molecules compose sequentially by **∘** and in parallel by **⊗** (the tensor is the tuple product; unit is `()`; the symmetric monoidal structure). Composition is associative (the category axioms), preserves behavioral equivalence (the congruence of behavioral equivalence), and lifts refinement with cost monotonicity (refinement substitution).

**Behavioral equivalence.** Two molecules are *behaviorally equivalent*, written `M ≈ M′`, when they produce indistinguishable input/output behavior: bisimulation, a congruence for ∘ and ⊗ (the congruence of behavioral equivalence). Every molecule has a **normal form** `NF(M)` under the runtime's equational theory: `NF(M) ≈ M`, `NF(M)` is unique up to coherence, and equivalence is decidable by comparing normal forms (unique normal forms, the normal-form lookup table; the full theory is decidable for finite signatures, the decidability of the equational theory). **Refinement**, written `f ⊑ f′`, means `f′` preserves the behavior of `f` at no greater cost (refinement substitution); maximal refinement of a behavior class is unique and its dispatcher is cost-minimal (maximal refinement). **Minimization** replaces M by its minimal-state realization `S_min(M)`, unique up to isomorphism, behavior-preserving, with compositional state bounds (minimal state spaces–minimization and equivalence).

**Rings and buffers.** A *ring* is a bounded, first-in-first-out buffer with `push` and `pop` operations that obey the ring equations `push ∘ pop = id` and `pop ∘ push = id` (on a non-empty ring) in every model (the ring equations); the ring rewrite system is terminating, locally confluent, and complete (the rewrite theorems), so ring traffic can be normalized. A ring of capacity `2^k` indexed by bitmask is correct iff the number of in-flight elements is at most `2^k − 1` (the ring-capacity invariant). A *buffer* is any preallocated, bounded storage region used by a data path. Rings and buffers are allocated and sized before the hot path begins and are never resized on it (policy R-01).

**Situations.** A *situation* is a triple `(A→B, Φ, c̄)`:
- `A → B` is the interface: the input type and output type of the work to be done;
- `Φ` is a behavioral specification predicate: a decidable statement of which behaviors the solution must exhibit (e.g., "the response echoes the request id; every input produces exactly one output; errors are reported as E");
- `c̄ = (t̄, m̄, k̄)` is a cost budget vector: budgets for time, memory, and cache misses (thesis Ch. 13).

A **solution** to a situation is a molecule `M : A → B` such that `M ⊢ Φ` (M satisfies the specification: equivalently `M ≈ M_Φ`, where `M_Φ` is the canonical molecule of Φ) and `cost(M) ≤ c̄` componentwise. The *feasible set* of solutions is finite and decidable for bounded types and additive costs (situation solvability); the decision matrices of Section 6 select the cost-minimal solution as a shortest path in the finite type graph (shortest-path optimality).

**Hot path and control path.** The *hot path* of a situation is the steady-state execution of its data-path molecule: the per-message or per-batch work whose cost budget is tight. The *control path* is the low-frequency, latency-tolerant work of the same situation: setup, handshakes, configuration, teardown, error recovery. What counts as hot is a property of the situation, declared in the situation record, not a property of the language or the framework.

**Situation kinds and situation records.** Every situation is classified into a *kind* (data-path, control-path, startup, shutdown, or offline; Section 4.3) and is described by a *situation record*: the triple `(A→B, Φ, c̄)`, the kind, the solution molecule with its composition tree, the policy levels that bind (via the mask), and the decision-matrix outcomes that were applied. The situation record is where compliance with this standard is demonstrated.

---

## 3. Conformance and Enforcement Model

**Levels.** Every policy binds at exactly one base level, possibly qualified by scope:

- **MUST**: the requirement is mandatory. Non-compliance is a defect; the only lawful non-compliance is a documented waiver (below).
- **SHOULD**: the requirement is mandatory unless a documented, reviewed reason for deviating is recorded with the situation record. No waiver is needed; the recorded reason suffices.
- **MAY**: the policy is permitted guidance. It imposes no obligation; where it is used, its stated discipline must be followed.

The effective level of a policy for a particular situation is determined by the policy's Scope and the Situation Application Matrix (Section 4). A policy that is not applicable to a situation imposes nothing on it.

**The single waiver clause.** This standard has exactly one waiver mechanism, applicable to all policies and all levels:

> A **waiver** is a record containing (a) the policy identifier and the situation it deviates from; (b) a written description of the deviation and why strict compliance is impractical; (c) at least one **compensating control** that restores the property the policy protects (the relevant resource bound, invariant, or behavior) with strength equal to what the policy would have provided; and (d) the identity of a named **reviewer** who has approved it.

Waivers are recorded in the project's waiver register, which is itself a reviewed artifact. A waiver is not a relaxation of the standard; it is an audited deviation with compensating strength. There is no other exemption mechanism, and no waiver may be used to waive a policy for an entire situation class without the review that the register demands.

**Enforcement mechanisms.** Every policy's Enforcement names one or more of four **universal mechanisms**. Each mechanism works for any type of application because none of them depends on the application's domain, framework, or structure:

- **M1: Static check (compiler or lint).** A check that runs over source or build artifacts without executing the program: a compiler diagnostic, a deny-level lint, a build-time assertion, a static analysis gate. It runs on any project that can build at all.
- **M2: CI job.** A continuous-integration job that runs on every change: a test run, a coverage gate, an allocation or cost regression, a sanitizer run, a dependency audit, a reproducibility check, a load probe. The job template is identical for every project; only its targets are parameterized.
- **M3: Property-test harness.** The project's test suite contains property-based tests; the policy names the law or property class the harness must contain. Property tests run under any test runner, on any application.
- **M4: Mandatory review item.** A checklist item that must be ticked and signed off in code review, with the required evidence recorded. The checklist is the same for every project, independent of application type.

These four mechanisms cover every policy in this standard. A policy is enforced when its named mechanism runs and passes; for M4, "passes" means the review records the required evidence. Where a policy's enforcement is automated, the check must be part of the build or the CI gate so that it blocks merge, not an advisory report.

**Conformance.** A project conforms to this standard when, for every situation it declares, every policy applicable to that situation passes its enforcement gate at the effective level the mask assigns, and every waiver and mask override on file satisfies the review discipline. Conformance is demonstrated in the situation records and the waiver register. Non-applicable policies impose nothing, and no project is required to declare situations it does not have: an application with no data path has, by construction, no data-path obligations.

**Completeness and optimality.** The conformance model is *complete*: for every policy and every situation, the situation has a determined interpretation: the policy is not applicable, or it binds at a specific level. It is *optimal*: a policy binds only where its discipline buys performance or security, and at the strength the situation's cost-criticality warrants. The mechanism that achieves this (Scope, Level, and the Situation Application Matrix) is the subject of the next section.

---

## 4. The Nuance Mechanism

This standard achieves **complete** and **optimal** enforcement through three per-policy attributes (Scope, Level, and the mask row) and one per-situation artifact (the situation record's kind declaration). The design goal is that no situation is over-governed and no situation is ungoverned: every policy either binds a given situation at exactly the right strength or binds it not at all.

### 4.1 Scope

Every policy carries a **Scope**: a decidable applicability predicate over situations, written in the vocabulary of Section 2. If `Scope(P)(s)` is false for situation `s`, policy `P` does not bind `s` at any level: it imposes nothing on that situation. Scope predicates are what keep the standard curated: a policy like ALLOC-01 (zero allocation) applies only to *situations with a declared hot path*, so an application whose only situation is a configuration loader is untouched by it.

Examples of scope predicates used in this standard:

- "situations whose solution molecule contains a ring" ([R]);
- "situations with a declared hot path" (many performance policies);
- "situations that process untrusted input" (security policies);
- "situations whose molecule declares laws / has a reference / has a declared hot path" (testing policies);
- "all situations" (the integration guarantee and a few safety invariants).

Scope predicates must be decidable: a reviewer or a tool must be able to determine, for a given situation record, whether the predicate holds. A scope predicate that cannot be evaluated is a defective scope.

### 4.2 Level

Every policy carries a **Level**: a base MUST/SHOULD/MAY as defined in Section 3, possibly qualified by its scope. The Level field of a policy states the strongest level the policy can reach; the mask row fixes the level in each situation kind.

### 4.3 Situation kinds

Every situation record declares a **kind**, drawn from a fixed set of five. The kinds partition all possible points of execution in any application:

- **D: Data-path**: the situation's molecule executes per message or per batch in steady state; its cost budget is tight (a per-message or per-batch time budget; a zero-allocation budget). This is where performance and security policies bind at their strongest.
- **C (Control-path**: low-frequency, latency-tolerant operations) connection setup, handshakes, configuration, error handling, command channels. Performance policies relax here; correctness and security policies mostly hold.
- **U: Startup**: one-time initialization before service begins. Allocation-heavy work lives here.
- **S: Shutdown**: teardown and draining after service stops. Boundedness still holds where resources are released.
- **O: Offline**: build-time, test-time, and tooling contexts. There is no runtime cost budget; performance policies lapse, while testing, supply-chain, and integration-guarantee policies bind at full strength (they are obligations about the project, executed by the CI job).

Every point of execution in any application is one of these five: steady-state per-message work (D), occasional control work (C), one-time setup (U), teardown (S), or not runtime at all (O). The classification is therefore complete; a situation record that cannot be classified is defective.

### 4.4 The Situation Application Matrix

The **Situation Application Matrix** is the default per-situation mask. Its rows are policies; its columns are the five situation kinds; each cell holds the effective level (MUST, SHOULD, MAY, or "n/a" for not applicable) of that policy for a situation of that kind.

For a situation `s` of kind `K` and a policy `P`:

- if `Scope(P)(s)` is false, the effective level is "n/a" (not applicable), whatever the cell says;
- otherwise the effective level is `matrix[P][K]`.

The matrix is part of this standard: Tables 4-1 and 4-2 below are normative, and the Level field of every policy in Section 5 is consistent with them (the Level field states the scope-qualified reading; the matrix is the consolidated default). A project may **override** a cell for a *specific* situation only with written justification, recorded in that situation's record and signed off by a reviewer under the discipline of Section 3's waiver register. An override changes the level at which an applicable policy binds one situation; it is a lighter instrument than a waiver, which excuses non-compliance with a binding policy. Overrides are how a project records, for example, "our control path is actually latency-critical; IO-03 is raised to MUST for situation S-17": visible, reviewed, and never silent.

Two properties follow by construction.

**Completeness of enforcement.** For every policy P and every situation s, exactly one of three things holds: `Scope(P)(s)` is false (P imposes nothing on s); or `Scope(P)(s)` is true and `matrix[P][kind(s)]` is one of MUST/SHOULD/MAY (a determined level); and the third case, an empty cell for an in-scope policy, does not occur because the matrix has no empty cells. The kind classification covers every situation. Therefore every situation has a determined interpretation under every policy: no policy is left "sort of applicable," and no situation is left without an answer.

**Optimality of enforcement.** The mask binds each policy only where its discipline buys performance or security: the performance policies reach MUST on the data path (D) and relax or lapse on the control path and offline; the safety and security invariants hold across all runtime kinds; the testing and supply-chain policies bind offline, where their obligations are actually executed. Only applicable policies bind, at the right strength, per situation. The default mask was set by the same curation criterion as the standard itself (adopt what buys performance or security, nothing else) and the override path keeps the mask optimal per project without weakening the default for anyone else.

### 4.5 Interaction with the decision matrices

The decision matrices of Section 6 consume situations and output policy levels. Those outputs are consistent with the mask by construction: a matrix selects a *design* within the design space that the situation's binding levels already delimit. D-1 may choose a ring capacity; it does not create an obligation to have a ring: R-02 binds only situations whose solution contains a ring (its Scope). D-4 may choose a batch size; the obligation to batch at that size is IO-03's, at the level the mask gives. A matrix therefore refines the design space within fixed levels; it never contradicts the mask and never invents a policy.

### 4.6 The default mask

Cells use MUST, SHOULD, MAY, and "n/a" (not applicable). Rows are policy identifiers as defined in Section 5; the Level fields there restate the scope-qualified reading of these rows.

**Table 4-1: Default mask, policies [A] through [CACHE]**

| Policy | D | C | U | S | O |
|---|---|---|---|---|---|
| A-01 Atom purity classification | MUST | MUST | MUST | MUST | n/a |
| A-02 No hidden state | MUST | MUST | MUST | MUST | n/a |
| A-03 Totality on declared domain | MUST | MUST | MUST | MUST | n/a |
| A-04 Domain declaration | MUST | SHOULD | SHOULD | SHOULD | n/a |
| MOL-01 Composition by ∘ and ⊗ | MUST | MUST | n/a | n/a | n/a |
| MOL-02 No dynamic dispatch on the hot path | MUST | n/a | n/a | n/a | n/a |
| MOL-03 Dense-integer dispatch | MUST | n/a | n/a | n/a | n/a |
| MOL-04 Minimal state | MUST | SHOULD | n/a | n/a | n/a |
| MOL-05 Behavioral specification declared | MUST | MUST | MUST | n/a | n/a |
| R-01 Preallocation before the hot path | MUST | SHOULD | SHOULD | n/a | n/a |
| R-02 Power-of-two capacity, bitmask indexing | MUST | SHOULD | n/a | n/a | n/a |
| R-03 Bounded in-flight invariant | MUST | MUST | n/a | SHOULD | n/a |
| R-04 Ring laws hold in every model | MUST | MUST | n/a | n/a | n/a |
| ALLOC-01 Zero allocation in declared hot paths | MUST | n/a | n/a | n/a | n/a |
| ALLOC-02 Statically sized or preallocated collections | MUST | SHOULD | SHOULD | n/a | n/a |
| ALLOC-03 Initialized memory | MUST | MUST | MUST | MUST | n/a |
| ALLOC-04 Capacity-bounded allocation, defined overflow | MUST | MUST | SHOULD | SHOULD | n/a |
| CACHE-01 Cache-line alignment | MUST | SHOULD | n/a | n/a | n/a |
| CACHE-02 Hot/cold separation | MUST | n/a | n/a | n/a | n/a |
| CACHE-03 False-sharing avoidance | MUST | n/a | n/a | n/a | n/a |
| CACHE-04 Layout justified by access pattern | MUST | SHOULD | n/a | n/a | n/a |

**Table 4-2: Default mask, policies [SIMD] through [INT]**

| Policy | D | C | U | S | O |
|---|---|---|---|---|---|
| SIMD-01 Vector ops never out of bounds | MUST | MUST | n/a | n/a | n/a |
| SIMD-02 Alignment satisfied | MUST | MUST | n/a | n/a | n/a |
| SIMD-03 Feature-gated with fallback | MUST | MUST | n/a | n/a | n/a |
| IO-01 Nonblocking data-path I/O | MUST | n/a | n/a | n/a | n/a |
| IO-02 Drain-until-exhausted | MUST | n/a | n/a | n/a | n/a |
| IO-03 Batching to amortize syscalls | MUST | MAY | n/a | n/a | n/a |
| IO-04 Options justified by targets | MUST | SHOULD | SHOULD | n/a | n/a |
| CONC-01 Per-core state preferred | MUST | SHOULD | n/a | n/a | n/a |
| CONC-02 Lock-free primitives only on the data path | MUST | SHOULD | n/a | n/a | n/a |
| CONC-03 Memory orderings justified | SHOULD | MAY | n/a | n/a | n/a |
| CONC-04 Concurrency model declared | MUST | MUST | SHOULD | n/a | n/a |
| SEC-01 Length-validated before use | MUST | MUST | n/a | n/a | n/a |
| SEC-02 Truncation detected | MUST | MUST | MUST | MUST | n/a |
| SEC-03 No uninitialized reads at trust boundaries | MUST | MUST | n/a | n/a | n/a |
| SEC-04 Resource caps declared | MUST | MUST | SHOULD | n/a | n/a |
| SEC-05 Dependency audit | MUST | MUST | MUST | MUST | MUST |
| OBS-01 Lock-free counters | MUST | MAY | n/a | n/a | n/a |
| OBS-02 Pull-based metrics | MUST | MAY | n/a | n/a | n/a |
| OBS-03 Zero-cost when compiled out | SHOULD | MAY | n/a | n/a | n/a |
| PLUGIN-01 ABI versioning | MUST | MUST | MUST | n/a | n/a |
| PLUGIN-02 Health checks | SHOULD | SHOULD | SHOULD | n/a | n/a |
| PLUGIN-03 Safe fallback | MUST | SHOULD | n/a | n/a | n/a |
| TEST-01 Property-based law tests | n/a | n/a | n/a | n/a | MUST |
| TEST-02 Fuzz for parsers and input handlers | n/a | n/a | n/a | n/a | MUST |
| TEST-03 Differential tests against a reference | n/a | n/a | n/a | n/a | MUST |
| TEST-04 Cost and zero-allocation regression | n/a | n/a | n/a | n/a | MUST |
| INT-01 Policies bind resources, invariants, behavior n/a never architecture | MUST | MUST | MUST | MUST | MUST |
| INT-02 No mandated framework, runtime, or business-logic structure | MUST | MUST | MUST | MUST | MUST |
| INT-03 Scope discipline by construction | MUST | MUST | MUST | MUST | MUST |

For the [TEST] policies the runtime cells (D–S) are marked "n/a" because the obligations execute in the offline (CI) context; their Scope states which situations' molecules the obligation covers: a data-path molecule's law tests run in CI, not in the data path. The [INT] policies bind everywhere because they are the standard's contract with application authors, asserted as standing review questions in every review.

---

## 5. Policy Catalog

Thirteen policy categories govern the architecture. Each category names a general policy domain; each policy carries Statement / Rationale (citing the governing theorem where one exists) / Wrong / Correct / Scope / Level / Enforcement. Code samples are illustrative, in the spirit of the companion ARCSS standard; they are not normative. **Policy Statements are deliberately general**: they never name socket options, crates, frameworks, or language-internal features: those appear only in the non-normative Appendix (Section 10). Every policy's Enforcement names a universal mechanism (M1 static check, M2 CI job, M3 property-test harness, M4 review item) that works for any type of application. The Level field of each policy is the scope-qualified reading of its mask row in Tables 4-1 and 4-2.

The categories:

- **[A] Atoms**: atoms are pure or effectful, have no hidden state, and are total on their declared domain.
- **[MOL] Molecules**: composition via ∘ and ⊗; hot paths avoid dynamic dispatch; dense-integer dispatch over chained conditionals.
- **[R] Rings & buffers**: preallocation before the hot path; power-of-two capacity with bitmask indexing; bounded in-flight invariant.
- **[ALLOC] Allocation**: zero allocation in declared hot paths; statically sized or startup-preallocated collections; initialized memory.
- **[CACHE] Cache & layout**: cache-line alignment; hot/cold separation; false-sharing avoidance; layout justified by access pattern.
- **[SIMD] SIMD**: vector ops never out of bounds; alignment satisfied; feature-gated with fallback.
- **[IO] Transport I/O**: nonblocking data-path I/O; drain-until-exhausted; batching to amortize syscalls; options justified by targets.
- **[CONC] Concurrency**: per-core state preferred; no shared mutable state except lock-free primitives; orderings justified.
- **[SEC] Security**: untrusted input length-validated before use; truncation detected; no uninitialized reads; resource caps declared; dependency audit.
- **[OBS] Observability**: lock-free counters; pull-based metrics; zero-cost when compiled out.
- **[PLUGIN] Plugins**: ABI versioning; health checks; safe fallback.
- **[TEST] Testing**: property-based law tests; fuzz for parsers and input; differential tests vs. a reference; cost/zero-allocation regression.
- **[INT] Integration**: the guarantee: policies bind resources, invariants, and behavior only, never application architecture.

### [A] Atoms

#### A-01: Atom Purity Classification

**Statement.** Every atom is declared either pure or effectful. A pure atom is a total function with no observable effect. An effectful atom acts only through the declared effect context Ctx of its situation, and its effects are limited to the capabilities that context contains. No atom is both; no atom performs an undeclared effect.

**Rationale.** The thesis splits Mol into PureMol (total functions, the pure-molecules-as-functions theorem) and EffMol(Ctx) (the Kleisli category of the state monad, the Kleisli characterization), and proves that every hybrid molecule factors uniquely as `q ∘ e ∘ p` with p and q pure and e effectful (Kleisli decomposition). The classification is what makes that decomposition possible, and with it the zero-allocation data path (linearity and zero allocation) and the determinism of pure fragments (the determinism theorem). An atom that is both pure and effectful has no home in the decomposition and no decidable behavior.

**Wrong.** An atom that computes a checksum and, on the side, writes a line to a log file; or an atom that reads a global clock without declaring a context that contains a clock.

**Correct.** The checksum is declared pure (a total function of its input), and the log line is returned as a value that the caller emits in the effectful layer. The clock-reading atom declares Ctx = {clock} and is classified effectful.

**Scope.** All situations whose solution contains atoms (every runtime situation).

**Level.** MUST (mask row A-01).

**Enforcement.** M1: a static check that every declared-pure atom contains no effectful construct and no access to ambient state; M4: review item "atom classification and effect-context membership stated."

#### A-02: No Hidden State

**Statement.** An atom's state is exactly its declared state space S. Atoms do not read or write ambient, global, or thread-local state; the only state an effectful atom may touch is its own S and the capabilities named in its Ctx.

**Rationale.** The behavior of a molecule is determined by its `(S, step)` (the category axioms). Hidden state makes the real state space larger than the declared one, which breaks the quotient that defines Mol (the bijection-quotient theorem), invalidates the minimal-state bounds (the compositional state bounds), and destroys the determinism of pure fragments (the determinism theorem). Hidden state is how determinism and testability leak; it is also invisible to the cost model, so it can exceed the memory budget `m̄` without being charged (the cost-enrichment theorem).

**Wrong.** An atom that increments a process-wide counter on every call, reading and writing a global the situation never declared.

**Correct.** The counter is a field of S; the atom's step function reads and writes it through its own state, and the situation record declares S.

**Scope.** All situations whose solution contains atoms.

**Level.** MUST (mask row A-02).

**Enforcement.** M1: a static check (deny-level lint) that atom code cannot access module-global mutable state; M4: review of every atom's state declaration.

#### A-03: Totality on Declared Domain

**Statement.** Every atom is total on its declared domain. For every input in the declared domain the atom returns a defined result in its declared output type. Inputs outside the domain are rejected by a defined mechanism (a documented error outcome) never by undefined behavior, panics, or reads outside the input.

**Rationale.** PureMol is the category of total functions (the pure-molecules-as-functions theorem); totality is what makes a transformation a function at all, and what makes the cost bounds of the cost-enrichment theorem–shortest-path optimality hold over the entire declared domain rather than over a sampled subset. An atom that silently returns garbage, crashes, or reads out of bounds on a plausible input breaks `M ⊢ Φ` for every situation that uses it (situation solvability).

**Wrong.** An atom that indexes into an input array without a length check and, on a short input, reads out of bounds.

**Correct.** The atom checks the length against its declared domain first (A-04, SEC-01); an out-of-domain input produces the defined error outcome.

**Scope.** All situations whose solution contains atoms.

**Level.** MUST (mask row A-03).

**Enforcement.** M1: compile-time bounds checks and lint gates on unchecked indexing in atom bodies (in a checked language these are compiler diagnostics); M3: property tests over the declared domain and its boundaries; M2: CI runs the harness.

#### A-04: Domain Declaration

**Statement.** Every atom declares its input domain (a subset of its input type, possibly the whole type) and its output type in the atom's contract. Finite domains are declared by enumeration or by an explicit bound; infinite domains are declared by a named equivalence-class partition. The declaration is decidable: a reviewer or a tool can determine whether a given input is in the domain.

**Rationale.** The declared domain is what totality (A-03) and coverage (TEST-01 ff.) are measured against, and the situation semantics of the thesis require a decidable description of what the solution must accept (situation solvability). Without a declared domain, no atom-level guarantee can be stated or checked, and the feasible set of solutions is not a set at all.

**Wrong.** An atom whose documentation says "handles any input" with no partition of the infinite cases.

**Correct.** The atom's contract names the domain explicitly: "lengths 0…65535," or "any byte string, partitioned into the well-formed / truncated / trailing-garbage classes": with the partition recorded.

**Scope.** All situations whose solution contains atoms.

**Level.** MUST on the data path; SHOULD on the control path, at startup, and at shutdown (mask row A-04).

**Enforcement.** M4: review item "every atom names a domain"; M3/M2: tests exercise the declared domain, its boundaries, and each equivalence class.

### [MOL] Molecules

#### MOL-01: Composition by ∘ and ⊗

**Statement.** Molecules are assembled only by sequential composition (∘) and tensor product (⊗). Orchestration through ad-hoc callbacks, entangled mutable shared objects, or inheritance-style structure is not molecule assembly. The composition tree of every molecule is explicit and recorded in the situation record.

**Rationale.** Composition in Mol is associative (the category axioms), respects behavioral equivalence (the congruence of behavioral equivalence), and lifts refinement with cost monotonicity (refinement substitution). The tensor product makes independent parts explicit: the basis of per-core partitioning (CONC-01) and of parallelism claims (complexity closure). Ad-hoc wiring has none of these properties: it is invisible to the type graph, so its behavior and its cost are both outside the decidable theory (the decidability of the equational theory, situation solvability).

**Wrong.** A "pipeline" built by passing a mutable object through a chain of callbacks, with no recorded composition.

**Correct.** An explicit composition `g ∘ f`, and `f ⊗ h` for independent branches, with the composition tree recorded.

**Scope.** All situations whose solution is a molecule.

**Level.** MUST (mask row MOL-01).

**Enforcement.** M1: a static structure check that composition sites use the declared operators (or the project's equivalent); M4: review records the composition tree for every non-trivial molecule.

#### MOL-02: No Dynamic Dispatch on the Hot Path

**Statement.** On declared hot paths, molecule composition is resolved statically: the identity of the steps executed for a message is fixed at build time, or is selected by a dense static table (MOL-03). Dynamic dispatch (indirect calls through erased interfaces) is not used on declared hot paths.

**Rationale.** The thesis proves that the most-refined dispatcher is sound and cost-minimal (maximal refinement) and that normal forms form precomputable lookup tables (unique normal forms–the normal-form lookup table); statically resolved, branch-predictable composition is what makes the fixed-iteration bounds of the iteration bound and the cost bounds of the cost-enrichment theorem–shortest-path optimality hold in practice. Dynamic dispatch on the hot path adds an unpredictable indirection whose cost is invisible to the type graph and therefore unaccounted in `c̄`.

**Wrong.** A per-message processing loop that routes each message through an indirect call through an erased interface, resolved at runtime per message.

**Correct.** The per-message path is resolved at build time (monomorphized); the small set of message kinds is dispatched through a dense table (MOL-03).

**Scope.** Situations with a declared hot path.

**Level.** MUST (mask row MOL-02).

**Enforcement.** M1: a lint or build check that declared hot-path code contains no indirect-dispatch construct; M4: review confirms the hot path is statically resolved.

#### MOL-03: Dense-Integer Dispatch over Chained Conditionals

**Statement.** Where a hot path selects among a fixed set of alternatives (message kinds, command codes, record types) dispatch uses dense integer keys and a lookup structure (a table, a jump table, an indexed array), not a chain of conditionals comparing against each alternative. The alternatives are assigned codes with no gaps.

**Rationale.** A dense key is a finite signature; the thesis' normal-form tables are exactly dense lookup tables over finite signatures (unique normal forms–the normal-form lookup table), and the most-refined dispatcher is the cost-minimal one (maximal refinement). Chained conditionals scale the branch cost linearly in the number of alternatives and defeat branch prediction on wide distributions; a dense table makes dispatch a single indexed step with a precomputable, bounded shape.

**Wrong.** On the hot path: `if kind == A {…} else if kind == B {…} else if kind == C {…}` and so on.

**Correct.** Alternatives carry dense integer codes; dispatch is one indexed step into a static table (or an indexed match over a dense enum), with the table defined once.

**Scope.** Hot-path situations whose molecule selects among three or more fixed alternatives.

**Level.** MUST where three or more alternatives are selected on a hot path; not applicable otherwise (mask row MOL-03).

**Enforcement.** M1: a lint that flags conditional chains on declared hot paths; M4: review of the dispatch structure.

#### MOL-04: Minimal State

**Statement.** A molecule carries no more state than its behavior requires: it is realized at, or reduced to, its minimal state space `S_min`. Redundant state (fields never read on any path, states that duplicate each other's futures) is eliminated.

**Rationale.** Every finite-state molecule has a minimal state space unique up to isomorphism (minimal state spaces), minimization preserves behavior (minimization-preserves-behavior), compositional state bounds hold (the compositional state bounds), and minimization is compatible with behavioral equivalence (minimization and equivalence). Because the cache footprint of loop state is monotone in state size (the hoisting theorem), smaller state is faster state; redundancy also enlarges the search space of the decision problem (shortest-path optimality).

**Wrong.** A protocol molecule carrying a "state" field that is written but never read, or three flags that are always equal.

**Correct.** The state space is minimal: distinct states have distinguishable futures (the Nerode quotient), and the implementation is that automaton.

**Scope.** Situations with a declared hot path; all protocol-carrying situations.

**Level.** MUST on the data path; SHOULD on the control path (mask row MOL-04).

**Enforcement.** M3: property tests assert behavioral equivalence between the molecule and its reduced version (bisimulation, the congruence of behavioral equivalence); M4: review examines the state declaration for redundant fields.

#### MOL-05: Behavioral Specification Declared

**Statement.** Every molecule that is a candidate solution to a situation names its behavioral specification Φ and its intended equivalence class under ≈. The claim "M satisfies Φ": `M ⊢ Φ`, i.e., `M ≈ M_Φ`: is recorded in the situation record and tested.

**Rationale.** A solution to a situation is a molecule with `M ⊢ Φ` (situation solvability). An unnamed Φ makes "solution" undecidable; a named Φ makes the feasible set finite and decidable (situation solvability) and equivalence checkable by normal forms (the normal-form lookup table).

**Wrong.** A module that "implements the protocol" without stating which behaviors are in and which are out.

**Correct.** The situation record states Φ (e.g., "the response carries the request id; every input produces exactly one output; errors are reported as E; nothing is emitted after close"), and tests compare the molecule against the specification's reference behaviors (TEST-03).

**Scope.** All situations declaring a solution.

**Level.** MUST (mask row MOL-05).

**Enforcement.** M4: review item "Φ stated in the situation record"; M3: tests compare the molecule against the specification's reference behaviors.

### [R] Rings & Buffers

#### R-01: Preallocation Before the Hot Path

**Statement.** Rings and buffers used by a data path are allocated and sized before the hot path begins: at startup, on connection or worker setup, or on the control path. The hot path never allocates, resizes, or reallocates a ring or buffer.

**Rationale.** The linearity theorem (linearity and zero allocation) makes a well-typed hot path allocation-free; preallocation is its construction-level realization. Allocation on the hot path re-introduces the very costs (boundedness, latency, cache) that the cost-enrichment theorem–shortest-path optimality budget against, and turns a static capacity into a runtime question.

**Wrong.** A data path that grows its receive buffer when a message is larger than expected.

**Correct.** The buffer is sized for the situation's declared maximum message and batch at setup (D-1); an over-limit message is the defined error outcome.

**Scope.** Situations with a declared hot path that contains rings or buffers.

**Level.** MUST on the data path; SHOULD on the control path and at startup (mask row R-01).

**Enforcement.** M1: a static check that declared hot-path code contains no allocation or resizing constructs; M2: CI runs the declared hot-path allocation regression (TEST-04).

#### R-02: Power-of-Two Capacity with Bitmask Indexing

**Statement.** Ring capacity is a power of two, and ring indices are computed by bitmask (`index & (capacity − 1)`). Capacity is a named constant of the ring's contract.

**Rationale.** the ring-capacity invariant: a ring of capacity `2^k` with bitmask indexing is correct iff the number of in-flight elements is at most `2^k − 1`; the bitmask replaces a modulus, making the index step a single machine operation and the capacity test a single compare: costs that belong in the per-message budget.

**Wrong.** A ring whose capacity is, say, 1000, indexed by `i % 1000`.

**Correct.** Capacity is 1024, indexing is `i & 1023`, and the contract states in-flight ≤ 1023.

**Scope.** Situations whose solution contains a ring.

**Level.** MUST on the data path; SHOULD on the control path (mask row R-02).

**Enforcement.** M1: a compile-time assertion that ring capacities are powers of two; M4: review of the ring contract.

#### R-03: Bounded In-Flight Invariant

**Statement.** At every instant, the number of in-flight elements in a ring is at most `capacity − 1` (for power-of-two rings; the ring's declared maximum otherwise). Exceeding the bound produces a defined outcome (backpressure on the control path, a push failure, or a documented error) never silent overwrite or corruption.

**Rationale.** This is the correctness half of the ring-capacity invariant (bitmask indexing is correct iff in-flight ≤ `2^k − 1`), and the ring equations `push ∘ pop = id` and `pop ∘ push = id` (the ring equations) hold only when pop never observes an empty ring and push never overwrites a live slot. Silent overwrite makes the ring a lossy channel whose behavior is not a function of its input: a violation of totality (A-03) at the molecule level.

**Wrong.** A ring that overwrites the oldest slot when full, discarding an element the consumer never saw.

**Correct.** The ring's push checks the bound and reports full; the situation's backpressure or error policy is invoked.

**Scope.** Situations whose solution contains a ring.

**Level.** MUST on the data path and the control path; SHOULD at shutdown while draining (mask row R-03).

**Enforcement.** M3: property tests assert the invariant (in-flight ≤ capacity − 1) and the ring laws over randomized sequences; M1: debug assertions in the ring implementation; M2: CI runs the property harness.

#### R-04: Ring Laws Hold in Every Model

**Statement.** Every ring implementation satisfies the ring equations behaviorally: push after pop is invisible on a non-empty ring, and pop after push returns the pushed element. Rewriting `push ∘ pop` away is behavior-preserving.

**Rationale.** The ring theory is complete for the free ring model (the ring equations), its rewrite system is terminating and confluent (the rewrite theorems), and rewriting by cost-nonincreasing rules preserves behavior and does not raise cost (normalization soundness). These laws are what allow the runtime and the builder to normalize ring traffic.

**Wrong.** A ring whose pop on a full ring returns the wrong element because indexing and capacity disagree.

**Correct.** The ring is tested against the laws (TEST-01) and normalized ring traffic is behavior-preserving.

**Scope.** Situations whose solution contains a ring.

**Level.** MUST (mask row R-04).

**Enforcement.** M3: property tests for `push ∘ pop ≈ id` and `pop ∘ push ≈ id` on non-empty rings; M2: CI runs them.

### [ALLOC] Allocation

#### ALLOC-01: Zero Allocation in Declared Hot Paths

**Statement.** Declared hot paths perform no allocation and no deallocation. All working storage is preallocated (R-01) or statically sized; temporary values live in preallocated scratch or in the declared state.

**Rationale.** The linearity theorem (linearity and zero allocation) proves that a well-typed hot path never allocates or deallocates, and normalization soundness guarantees that removing such steps by normalization is behavior-preserving and cost-nonincreasing. Allocation on the hot path is also invisible in the type graph: it can appear only as an effect, which the Atom/Molecule discipline confines to Ctx (A-01).

**Wrong.** A per-message path that builds a fresh collection per message and drops it.

**Correct.** The per-message path reuses preallocated buffers and scratch (zero allocation) verified by the allocation regression.

**Scope.** Situations with a declared hot path.

**Level.** MUST (mask row ALLOC-01).

**Enforcement.** M1: a static or build-time check that declared hot-path code contains no allocation; M2: CI runs the zero-allocation regression (TEST-04), which fails on any allocation in a declared hot path.

#### ALLOC-02: Statically Sized or Startup-Preallocated Collections

**Statement.** Collections used by a situation are statically sized where possible, or preallocated to their declared maximum at startup or on the control path. Runtime growth is bounded by a declared cap, and growth past the cap is a defined error.

**Rationale.** Static bounds make maximum consumption a compile-time or load-time property (the memory budget `m̄` of the situation; the cost-enrichment theorem) and a collection whose size is unknown is a cost that cannot enter the budget vector and cannot be optimized over (shortest-path optimality). Boundedness is the affine/resource discipline of ARCSS adopted here because boundedness is what keeps `m̄` decidable (situation solvability).

**Wrong.** A connection table that grows without bound as connections arrive.

**Correct.** The table is preallocated to `MAX_CONNECTIONS`; exceeding it produces the defined "connection refused" outcome.

**Scope.** All runtime situations containing collections.

**Level.** MUST on the data path; SHOULD on the control path and at startup (mask row ALLOC-02).

**Enforcement.** M1: capacity declarations checked as compile-time or load-time assertions; M4: review confirms every collection's cap is declared and enforced.

#### ALLOC-03: Initialized Memory

**Statement.** Memory acquired by an atom or molecule is initialized before first read: every byte an atom reads was written since acquisition. There are no reads of uninitialized memory anywhere in the runtime path.

**Rationale.** Reading uninitialized memory is undefined behavior; a molecule that can do so is not a total function on its declared domain (A-03, the pure-molecules-as-functions theorem), and its behavior is not a function of its input at all. The determinism guarantee for pure molecules (the determinism theorem) and the equivalence notion (the congruence of behavioral equivalence) presuppose defined values.

**Wrong.** A buffer that is allocated and then parsed before being filled.

**Correct.** The buffer is zero-filled or written before use; the parse step reads only defined bytes.

**Scope.** All runtime situations.

**Level.** MUST (mask row ALLOC-03).

**Enforcement.** M1: static checks for reads of uninitialized memory (the language checker's diagnostics where available); M2: CI runs the sanitizer, which flags uninitialized reads.

#### ALLOC-04: Capacity-Bounded Allocation with Defined Overflow

**Statement.** Every allocation site carries a declared maximum size. Exceeding it is a defined error outcome: never silent partial growth, never process termination by exhaustion, never undefined behavior. Allocation of attacker-influenceable sizes is gated on the length validation of SEC-01.

**Rationale.** Boundedness makes consumption a static property (ALLOC-02, the cost-enrichment theorem) and keeps the cost vector decidable (situation solvability). Unbounded allocation influenced by untrusted input is a denial-of-service vector (SEC-04).

**Wrong.** An allocation whose size is taken from untrusted input without a cap check.

**Correct.** The size is checked against the declared cap (SEC-01) before allocation; over-cap is the defined error.

**Scope.** All runtime situations that allocate.

**Level.** MUST on the data path and the control path; SHOULD at startup and shutdown (mask row ALLOC-04).

**Enforcement.** M1: a static or lint check for unguarded allocation of variable size; M4: review confirms caps at allocation sites.

### [CACHE] Cache & Layout

#### CACHE-01: Cache-Line Alignment for Hot and Shared Structures

**Statement.** Structures accessed on a hot path, and structures shared between threads or cores, are aligned to the cache-line size of the target (the situation record names the target: commonly 64 bytes). Alignment is documented in the structure's contract.

**Rationale.** Cache-line alignment prevents a structure from straddling lines (which doubles the transfers per access) and is a precondition for vector access (SIMD-02). The cost vector `c̄` counts cache misses (the cost-enrichment theorem); alignment is a static way of spending that budget well, and the cache-footprint monotonicity of the hoisting theorem makes compact hot state pay directly.

**Wrong.** A hot per-connection state record that straddles a cache-line boundary and is touched on both sides.

**Correct.** The record is declared aligned to the target line size and fits within one or a known small number of lines.

**Scope.** Situations with a declared hot path; structures shared across threads.

**Level.** MUST on the data path; SHOULD on the control path (mask row CACHE-01).

**Enforcement.** M1: static/compile-time alignment assertions on the structure declarations; M4: review of the layout documentation.

#### CACHE-02: Hot/Cold Separation

**Statement.** Fields of a record that are accessed on the hot path are stored together, compactly, and separately from fields accessed only on the control path. A hot record does not carry cold fields in its hot footprint.

**Rationale.** The cache footprint of loop state is monotone in state size (the hoisting theorem), and hot/cold splitting is the layout form of state minimization (MOL-04): it shrinks the hot working set to what the hot path actually reads. Cold fields dragged into hot lines waste the miss budget (the cost-enrichment theorem).

**Wrong.** A per-connection record whose hot fields and rarely-read metadata fields share one structure and one cache line.

**Correct.** The hot portion (a few cache lines) and the cold portion (metadata, configuration) are separate; the cold part is fetched only when needed (D-10).

**Scope.** Situations with a declared hot path whose state has both hot and cold fields.

**Level.** MUST on the data path; not applicable otherwise (mask row CACHE-02).

**Enforcement.** M2: CI profiling job records hot-path cache misses; M4: review requires the hot/cold field-split argument: which fields are hot and why.

#### CACHE-03: False-Sharing Avoidance

**Statement.** Per-core (per-worker) state never shares a cache line with another core's per-core state. Per-core state is padded or partitioned so that each core's slots lie on distinct lines.

**Rationale.** False sharing makes two independent molecules (tensor factors, the symmetric monoidal structure) pay for each other's cache traffic; it converts the per-core independence that CONC-01 establishes into hidden contention. The cost is a miss-budget tax invisible to the type graph (the cost-enrichment theorem) that violates the independence assumptions of the per-core decomposition (complexity closure).

**Wrong.** An array of per-core counters in consecutive words; core A's increment evicts core B's line.

**Correct.** Counters are spaced onto separate cache lines (per-core partition with padding), and each core touches only its own line.

**Scope.** Situations with per-core state and a declared hot path.

**Level.** MUST (mask row CACHE-03).

**Enforcement.** M2: CI profiling job flags high cache-line contention on per-core structures; M4: review of the per-core layout.

#### CACHE-04: Layout Justified by Access Pattern

**Statement.** The layout of every hot structure is justified by its dominant access pattern, and the justification is recorded in the situation record. The SoA/AoS decision (D-2) is explicit; layouts are not chosen ad hoc.

**Rationale.** Layout is the concrete realization of the cost vector (the cost-enrichment theorem): for a given access trace, the layout determines which accesses miss. An unexamined layout is an unexamined cost. The thesis' quantitative semantics make layout a first-class optimization target (the cost-enrichment theorem–shortest-path optimality); the standard's job is to make the choice explicit and checkable.

**Wrong.** A hot structure laid out "the way it was convenient to write," with no record of why.

**Correct.** The situation record states the dominant access pattern (field-wise vs. record-wise; vectorized or not) and the D-2 outcome.

**Scope.** Situations with a declared hot path containing data structures.

**Level.** MUST on the data path; SHOULD on the control path (mask row CACHE-04).

**Enforcement.** M4: review item "layout justified by access pattern" (the D-2 entry in the situation record); M2: CI profiling corroborates.

### [SIMD] SIMD

#### SIMD-01: Vector Operations Never Out of Bounds

**Statement.** A vectorized operation processes exactly the declared range and no more: vector lanes cover a prefix of the range, and the remainder is handled by an explicit scalar path. No vector load, store, or gather reads or writes outside the allocated range, under any feature set.

**Rationale.** A vector op is a tensor of scalar ops (the transport theory: batching distributes), and the scalar remainder is what makes the whole a total function on the declared range (A-03, the pure-molecules-as-functions theorem). An out-of-bounds vector access is undefined behavior and breaks totality exactly as an unchecked scalar read does.

**Wrong.** A vectorized loop that reads 16 elements when the buffer holds 10, relying on padding that "happens" to exist.

**Correct.** The loop vectorizes only whole vectors (the first 8 or 0 elements) and processes the remainder scalar.

**Scope.** Situations containing vectorized atoms.

**Level.** MUST (mask row SIMD-01).

**Enforcement.** M1: compile-time bounds checks and sanitizer coverage over vectorized loops; M3: property tests over range lengths including non-multiples of the lane count; M2: CI runs the sanitizer and the tests.

#### SIMD-02: Alignment Satisfied Before Vector Access

**Statement.** Before a vector path runs, the alignment its instructions require is established: statically (the data is declared aligned) or by a runtime check that selects the scalar path when alignment is absent. Misaligned vector access is never executed.

**Rationale.** Misaligned vector access is undefined behavior or a fault; it violates totality (A-03). The alignment check is a pure predicate that selects between the vector and scalar branches: the same refinement structure as feature gating (SIMD-03; refinement substitution).

**Wrong.** A vector path that assumes the allocator returned 32-byte-aligned memory without a guarantee.

**Correct.** The buffer type carries the alignment (declared or guaranteed), or the vector path verifies alignment and falls back to the scalar path.

**Scope.** Situations containing vectorized atoms.

**Level.** MUST (mask row SIMD-02).

**Enforcement.** M1: static alignment assertions on the types the vector path consumes; M4: review confirms the alignment argument or the runtime check.

#### SIMD-03: Feature-Gated with Fallback

**Statement.** Vector paths are gated on the target's feature support (detected at build time, or at runtime where the target varies) and are paired with a behaviorally equivalent scalar fallback. The two paths are the same molecule: same domain, same behavior, same bounds; only the cost differs.

**Rationale.** Feature gating is the common-case specialization structure of refinement substitution (refinement substitution): the vector path and the scalar path both refine the same behavior, and selection is sound because behavior is preserved (the congruence of behavioral equivalence). The cost difference is exactly what the cost vector wants (the cost-enrichment theorem): the same behavior at lower cost on capable hardware, correct behavior everywhere.

**Wrong.** Code that assumes a wide vector extension exists and faults on hardware without it.

**Correct.** The vector path is enabled only when the feature is detected; otherwise the scalar path runs; both paths are tested against the same laws.

**Scope.** Situations containing vectorized atoms.

**Level.** MUST (mask row SIMD-03).

**Enforcement.** M2: CI job runs the test suite on a target without the feature and on one with it; M3: law tests run against both paths; M4: review confirms the fallback is behaviorally identical.

### [IO] Transport I/O

#### IO-01: Nonblocking Data-Path I/O

**Statement.** Data-path I/O is nonblocking: the data path never blocks waiting for readiness or completion. Readiness and completion are managed by the situation's event machinery on the control path; the data path consumes and produces only what is ready.

**Rationale.** Blocking on the data path makes the per-message cost unbounded: a cost that cannot enter `c̄` (the cost-enrichment theorem) and a schedule that defeats the batching laws (syscall amortization). Nonblocking I/O keeps the data-path molecule a function of its ready inputs (A-03, the pure-molecules-as-functions theorem), with latency bounded by the budget.

**Wrong.** A receive loop that blocks in the read until a message arrives.

**Correct.** The data path is invoked on readiness and drains what is ready (IO-02); blocking waits live on the control path.

**Scope.** Situations with a declared hot path that performs I/O.

**Level.** MUST (mask row IO-01).

**Enforcement.** M1: a static check (or lint) that hot-path I/O uses nonblocking operations only; M4: review confirms the data path never blocks.

#### IO-02: Drain-Until-Exhausted

**Statement.** Once a data source signals readiness, the data path drains it until it reports empty (the would-block result), then returns to the event machinery. A readiness event is consumed by draining, not by processing one message and leaving the rest for another event.

**Rationale.** Draining is the I/O form of batching: each drain amortizes the syscall/event cost over all available work (syscall amortization). Draining one item per event multiplies the fixed cost per message and can starve the machinery with event churn.

**Wrong.** On each readiness event, reading a single message.

**Correct.** On readiness, reading until the source reports empty, bounded by the batch and in-flight budgets (D-4, R-03).

**Scope.** Data-path situations performing I/O.

**Level.** MUST (mask row IO-02).

**Enforcement.** M2: CI load job asserts the drain behavior under sustained input (messages processed per event at or above the declared minimum); M4: review of the drain loop.

#### IO-03: Batching to Amortize Syscalls

**Statement.** Data-path I/O transfers are batched: multiple messages are transferred per syscall or event where the situation's budget allows, with the batch size chosen by D-4. Single-message transfers are the exception, justified by the latency budget: not the default.

**Rationale.** syscall amortization: `cost(batch_n) ≤ n · cost(single) + fixed(n)`, with `fixed(n)/n → 0`. The syscall cost is the fixed part; batching amortizes it over n messages. Batching is also the concrete form of the batch laws (the transport theory, batch flattening) that make batch trees flatten.

**Wrong.** One write per small response.

**Correct.** Responses are accumulated into the preallocated batch buffer and written in one transfer up to the batch budget (D-4).

**Scope.** Data-path situations performing I/O.

**Level.** MUST on the data path; MAY on the control path (mask row IO-03).

**Enforcement.** M2: CI job measures transfers per syscall (or the equivalent) on the declared hot path against a floor; M4: review of the batch-buffer sizing (D-4).

#### IO-04: Options Justified by Latency/Throughput Targets

**Statement.** Transport and I/O configuration choices (protocol selection via D-3, option settings, queue sizes, polling strategy via D-5, copy strategy via D-6) are justified against the situation's declared latency and throughput targets and recorded in the situation record. No configuration knob is set "because it is usually faster."

**Rationale.** The cost budget `c̄` is a vector of declared targets; a configuration choice is a point in the feasible set of the decision problem (situation solvability), and the optimal choice is the one minimizing cost under the budget (shortest-path optimality). Unjustified options are choices outside any budget: they cannot be evaluated, so they cannot be optimal.

**Wrong.** Enabling every optimization in the catalog "for performance," with no targets and no measurement.

**Correct.** The situation declares its targets (e.g., "p99 per message under the declared load"); the D-3/D-5/D-6 outcomes and their measurements are recorded.

**Scope.** All runtime situations performing I/O.

**Level.** MUST on the data path; SHOULD on the control path and at startup (mask row IO-04).

**Enforcement.** M2: CI (or a release-gate job) compares measured latency/throughput against the declared targets; M4: review item "every transport option justified by targets."

### [CONC] Concurrency

#### CONC-01: Per-Core State Preferred

**Statement.** State is partitioned per core (or per worker) wherever the workload permits: each data-path unit owns its state and touches only its partition. Cross-partition communication is by message passing or lock-free primitives (CONC-02), never by shared locks on the data path.

**Rationale.** Per-core partitioning is the tensor structure of the situation (the symmetric monoidal structure, complexity closure): independent molecules compose without interference (the determinism theorem), and partitioned state keeps each core's working set cache-resident (the hoisting theorem). Shared mutable state, by contrast, reintroduces interference and destroys the per-core cost model (the cost-enrichment theorem).

**Wrong.** One shared connection table, guarded by a lock, touched by all workers on the data path.

**Correct.** Each worker owns its connections (its partition); only the control path sees the global view.

**Scope.** Situations with a declared hot path and concurrency.

**Level.** MUST on the data path; SHOULD on the control path (mask row CONC-01).

**Enforcement.** M4: review of the concurrency model and the partition boundaries (a mandatory architecture item); M2: CI race-detection job.

#### CONC-02: Shared Mutable State Only via Lock-Free Primitives on the Data Path

**Statement.** On declared hot paths, shared mutable state is limited to lock-free primitives (atomic operations) with declared invariants. Locks, mutexes, and blocking synchronization are not used on the data path; they are confined to the control path and to startup/shutdown.

**Rationale.** Lock-free primitives keep the per-operation cost bounded and independent of contention patterns, so the cost stays in `c̄` (the cost-enrichment theorem); they preserve the determinism of pure fragments (the determinism theorem). Locks on the data path make latency a function of other threads' behavior: an unbounded interference that no budget can contain.

**Wrong.** A mutex-protected hot counter, or a locked queue, on the data path.

**Correct.** The counter is atomic with a documented invariant; queues are per-core or lock-free bounded rings (R-03).

**Scope.** Data-path situations with any shared state.

**Level.** MUST on the data path; SHOULD on the control path (mask row CONC-02).

**Enforcement.** M1: a static check (or lint) that declared hot-path modules contain no blocking-synchronization constructs; M4: review of shared-state invariants.

#### CONC-03: Memory Orderings Justified

**Statement.** Where atomics or lock-free structures are used, the memory-ordering choices are documented with an argument, and the weakest ordering that is correct for the declared invariant is used. Unexamined strong orderings (defaulting everything to sequentially consistent) are avoided on the data path.

**Rationale.** Ordering strength is a cost on the data path (fences and barriers stall the pipeline) and a correctness property at the same time; an unexamined ordering either overpays (the cost-enrichment theorem budget) or under-delivers (breaking the invariant, the congruence of behavioral equivalence). The weakest-correct-ordering discipline keeps the cost vector honest.

**Wrong.** An atomic counter updated with sequentially-consistent ordering in a per-message loop "because it is the default."

**Correct.** The counter's invariant (e.g., "approximate sample; relaxed suffices") is documented and the relaxed ordering is used; or the release/acquire pair is argued for the publication it implements.

**Scope.** Situations using atomics or lock-free structures.

**Level.** SHOULD on the data path; MAY on the control path (mask row CONC-03).

**Enforcement.** M4: review item "ordering argument recorded for every atomic"; M2: CI runs the concurrency stress job.

#### CONC-04: Concurrency Model Declared Per Situation

**Statement.** Every situation with concurrency declares its concurrency model in the situation record: single-threaded, per-core partitions, or shared-state with lock-free primitives. The primitives used are instances of the declared model; mixing models is allowed only where the record explains the boundary.

**Rationale.** A declared model makes the concurrency policies checkable (which primitives are legal, which invariants to look for) and makes the per-situation mask meaningful. This is the concurrency-model discipline of ARCSS, adopted in its per-situation form.

**Wrong.** A situation that uses threads, atomics, and a lock with no recorded model.

**Correct.** The record says "per-core partitions; atomics only for sampling counters," and the code matches.

**Scope.** All situations with concurrency.

**Level.** MUST on the data path and the control path; SHOULD at startup (mask row CONC-04).

**Enforcement.** M4: review item "concurrency model recorded"; M2: CI race-detection job runs.

### [SEC] Security

#### SEC-01: Untrusted Input Length-Validated Before Use

**Statement.** Lengths, counts, and sizes derived from untrusted input are validated against their declared caps before any use: before allocation (ALLOC-04), before indexing, and before branching on them. Validation is a pure filter applied at the trust boundary (the parser) and validated lengths are the only lengths the rest of the molecule sees.

**Rationale.** A length from untrusted input is a control input to allocation and indexing; unvalidated, it makes the molecule's resource use a function of the attacker (a denial-of-service vector; SEC-04) and its memory safety a matter of luck (A-03). Early validation is the parser left-factoring of the security world: a cheap filter applied before expensive operations (parser left factoring), and it is what keeps the cost vector decidable (situation solvability).

**Wrong.** A parser that allocates a buffer whose length comes straight from the packet header.

**Correct.** The header length is checked against the declared maximum (and against the bytes actually present) before anything is allocated or indexed.

**Scope.** Situations that process untrusted input.

**Level.** MUST (mask row SEC-01).

**Enforcement.** M1: a static taint-style check (or lint) that untrusted lengths reach only validated sites; M3: fuzz harness (TEST-02) and boundary property tests; M2: CI runs them.

#### SEC-02: Truncation Detected

**Statement.** Narrowing conversions and wrapping arithmetic on lengths, counts, and offsets are detected, not silent. A truncation or wraparound is a defined error or a checked alternative; silent loss of magnitude is forbidden.

**Rationale.** Truncation silently changes the declared domain of a value (a length that becomes small after narrowing is no longer the length it was) breaking totality (A-03) and turning a security check into a false one. The checked alternative keeps the molecule a total function (the pure-molecules-as-functions theorem).

**Wrong.** Storing a 64-bit length into a 32-bit field "because it will fit," losing the top bits.

**Correct.** The narrowing is a checked conversion that fails with a defined error when the value does not fit.

**Scope.** All runtime situations.

**Level.** MUST (mask row SEC-02).

**Enforcement.** M1: static diagnostics for unchecked narrowing conversions and wrapping arithmetic on lengths; M4: review of conversion sites.

#### SEC-03: No Uninitialized Reads at Trust Boundaries

**Statement.** Atoms that process untrusted input never read uninitialized memory: every byte read while parsing or deciding was defined before the read. Uninitialized content is never copied into outputs, logs, or error messages.

**Rationale.** Uninitialized memory may contain prior-tenancy data (kernel or other processes' content) so reading or emitting it is an information leak, and reading it at all is undefined behavior (A-03). This is the confidentiality half of ALLOC-03, binding at trust boundaries.

**Wrong.** A parser that reads past the filled region of a receive buffer and copies the garbage into an error message.

**Correct.** The parser's reads are bounded by the validated length (SEC-01); all memory is initialized (ALLOC-03); outputs contain only defined bytes.

**Scope.** Situations that process untrusted input.

**Level.** MUST (mask row SEC-03).

**Enforcement.** M1: sanitizer coverage of parse paths; M2: CI sanitizer job; M3: fuzz harness with uninitialized-read detection.

#### SEC-04: Resource Caps Declared

**Statement.** Every situation declares its resource caps (maximum in-flight, maximum connections or sessions, maximum memory, maximum batch) and the caps are enforced with defined overflow outcomes. Caps are per-situation and recorded in the situation record.

**Rationale.** Caps are the situation's bounded-resource declaration: they make the cost vector decidable (situation solvability) and the ring invariant checkable (the ring-capacity invariant). Unbounded resources are the classic exhaustion attack and make the memory budget `m̄` meaningless (the cost-enrichment theorem).

**Wrong.** A server with no connection limit, whose connection table grows until the process dies.

**Correct.** `MAX_CONNECTIONS` is declared; excess connections receive the defined "busy" outcome; the table is preallocated (ALLOC-02).

**Scope.** All runtime situations.

**Level.** MUST on the data path and the control path; SHOULD at startup (mask row SEC-04).

**Enforcement.** M4: review item "caps declared and enforced"; M2: CI load job asserts overflow outcomes under declared caps.

#### SEC-05: Dependency Audit

**Statement.** All dependencies are pinned (versions and integrity hashes), their provenance is recorded, and a vulnerability/audit scan runs on every change. New dependencies pass the same gate before they are admitted.

**Rationale.** A dependency is code that enters the trust boundary; unpinned or unaudited dependencies are unexamined input to the built molecule. Pinning makes builds reproducible; auditing keeps known defects out. The other security policies govern the project's own code; this one governs what the project imports. It is the supply-chain discipline of ARCSS, adopted because a data plane is only as trusted as its dependencies.

**Wrong.** Floating dependency versions resolved at build time, with no scan.

**Correct.** A locked dependency graph with integrity hashes; an audit job in CI; provenance recorded for every dependency.

**Scope.** All situations, including offline.

**Level.** MUST (mask row SEC-05).

**Enforcement.** M2: CI job that checks the locked manifest and runs the audit scan; M4: review of new dependencies.

### [OBS] Observability

#### OBS-01: Lock-Free Counters on the Data Path

**Statement.** Metrics collected on the data path use lock-free atomic counters with declared invariants; metric updates never lock, block, or allocate. The weakest correct ordering is used (CONC-03).

**Rationale.** Observability is a tensor factor of the molecule (the symmetric monoidal structure): it must not interfere with the measured behavior (the determinism theorem) or its cost budget (the cost-enrichment theorem). A locking metric would turn measurement into a perturbation; the measurement cost must stay in `c̄` like any other cost.

**Wrong.** A per-message metric increment guarded by a mutex.

**Correct.** An atomic counter with a documented invariant ("exact count" or "approximate sample"), updated with the weakest correct ordering.

**Scope.** Data-path situations with metrics.

**Level.** MUST on the data path; MAY on the control path (mask row OBS-01).

**Enforcement.** M1: a static check (or lint) that data-path metric sites contain no locking or blocking constructs; M4: review of counter invariants.

#### OBS-02: Pull-Based Metrics

**Statement.** Metrics are collected by pull (exported on request or on a sampling timer on the control path) never by blocking push from the data path. The data path records; the control path or an observer drains.

**Rationale.** Push from the data path converts a bounded data-path cost into an unbounded one (the sink may be slow); pull keeps all data-path costs bounded and in `c̄` (the cost-enrichment theorem). This is the observability form of the bounded-in-flight discipline (R-03): the sink is a consumer the data path never waits for.

**Wrong.** A data path that synchronously writes a metric line to a file per message.

**Correct.** The data path updates counters; a sampling job on the control path exports snapshots.

**Scope.** Data-path situations with metrics.

**Level.** MUST on the data path; MAY on the control path (mask row OBS-02).

**Enforcement.** M2: CI profiling job asserts that data-path metric updates are lock-free and nonblocking; M4: review of the metric path.

#### OBS-03: Zero-Cost When Compiled Out

**Statement.** Compiling metrics out produces a molecule behaviorally identical to the instrumented one, at no greater cost: the metric updates are removable by behavior-preserving, cost-nonincreasing rewrites. The molecule without metrics is the same molecule.

**Rationale.** The normalization-soundness theorem: normalization removes steps with behavior preserved and cost not increased when the rules are cost-nonincreasing. Metrics that change behavior (a metric that allocates, or whose absence changes control flow) would violate this; metrics that do not can be compiled out exactly as the theorem describes.

**Wrong.** Metrics whose collection affects timing-dependent behavior, or whose removal changes code paths.

**Correct.** Metrics are pure side-records (counter updates) that compile away; tests run with metrics on and off and assert behavioral equivalence (bisimulation, the congruence of behavioral equivalence).

**Scope.** Data-path situations with metrics.

**Level.** SHOULD on the data path; MAY on the control path (mask row OBS-03).

**Enforcement.** M3: property tests assert behavioral equivalence with and without metrics; M2: CI runs both configurations; M4: review of the metric design.

### [PLUGIN] Plugins

#### PLUGIN-01: ABI Versioning

**Statement.** Plugin interfaces are versioned; the version is checked at load time, and a mismatch is a defined load failure: the plugin is not used. Interface evolution is a new version, never a silent change.

**Rationale.** A plugin is a molecule composed into the host across an external boundary; the interface is the signature Σ of the composition (the free construction). Versioning makes the signature decidable and the equivalence classes of plugins finite and checkable (finiteness of normal forms and the lookup table, the decidability of the equational theory): two plugins with the same versioned interface are comparable; a mismatched one is out of the signature.

**Wrong.** A plugin that calls host functions by raw position with no version handshake.

**Correct.** The plugin advertises the interface version; the host verifies it before composing; mismatch is a clean load failure.

**Scope.** Situations that load plugins.

**Level.** MUST (mask row PLUGIN-01).

**Enforcement.** M1: build/load-time check of version compatibility; M4: review of the interface-versioning contract.

#### PLUGIN-02: Health Checks

**Statement.** Plugins expose a health predicate (a pure, cheap function of the plugin's state) and the host evaluates it on a schedule or before composition. An unhealthy plugin is treated as failed.

**Rationale.** The host composes the plugin into a molecule whose behavior must satisfy Φ (situation solvability); a plugin that has drifted from its contract (the congruence of behavioral equivalence) makes the composite's behavior undetermined. A health predicate is the decidable proxy: the host verifies that the plugin still refines its interface (refinement substitution) without trusting it.

**Wrong.** A plugin that is loaded, composed, and never checked until it misbehaves at runtime.

**Correct.** The plugin exposes a health predicate; the host checks it on the control path and reacts (PLUGIN-03).

**Scope.** Situations that load plugins.

**Level.** SHOULD (mask row PLUGIN-02).

**Enforcement.** M2: CI job exercises load, health-check, and failure paths; M4: review of the health contract.

#### PLUGIN-03: Safe Fallback

**Statement.** Every plugin composition has a defined fallback: on load failure, health failure, or runtime misbehavior, the host substitutes a built-in molecule that refines the same behavioral specification, or fails closed into a defined degraded behavior. The data path never depends on an unchecked plugin.

**Rationale.** Fallback is the refinement substitution of refinement substitution (the built-in refines the same Φ at known cost) and fail-closed keeps the composite's behavior within Φ (situation solvability). A data path that hard-depends on an unchecked plugin makes Φ undecidable at run time.

**Wrong.** A data path that dispatches into a plugin with no alternative when the plugin is slow or broken.

**Correct.** The plugin slot has a built-in default molecule; the host switches to it on any failure, and the switch itself happens on the control path.

**Scope.** Situations that load plugins.

**Level.** MUST where plugins are composed into a data-path situation; SHOULD where composed into a control-path situation; not applicable otherwise (mask row PLUGIN-03).

**Enforcement.** M2: CI job injects plugin failures and asserts the fallback path preserves behavior; M3: law tests for the fallback molecule; M4: review of the fallback design.

### [TEST] Testing

#### TEST-01: Property-Based Law Tests

**Statement.** Every molecule ships property-based tests of its equational laws: ring laws (`push ∘ pop ≈ id`; `pop ∘ push ≈ id` on non-empty rings), batch laws (flattening, batch flattening), parser laws (left-factoring normal forms, parser left factoring), transport interchange (the transport theory), and any law the molecule's theory declares. Tests are generated over the declared domain and its boundaries.

**Rationale.** The laws are the specification: they are complete for their theories (the ring equations, parser left factoring, batch-flattening rewrites), and normalization is sound (normalization soundness). Property tests that check the laws on randomized inputs check exactly the identities the runtime and the builder rely on (the normal-form lookup table).

**Wrong.** A ring tested only with hand-written sequential cases.

**Correct.** A property harness generates arbitrary push/pop sequences and asserts the ring laws and the in-flight invariant (R-03).

**Scope.** Situations whose molecules declare laws.

**Level.** MUST: the obligation executes offline (mask row TEST-01: cell O = MUST).

**Enforcement.** M3: the property-test harness contains the law tests; M2: CI runs the harness with a fixed seed corpus plus fresh seeds.

#### TEST-02: Fuzz for Parsers and Input Handlers

**Statement.** Every atom or molecule that parses or otherwise consumes untrusted input is fuzzed: arbitrary, structure-mutating, and boundary inputs are fed under a sanitizer, and any crash, hang, uninitialized read, or out-of-bounds access is a defect.

**Rationale.** Parsers sit at the trust boundary (SEC-01); totality on the declared domain (A-03) is exactly what fuzzing checks, and parser left factoring's left-factoring gives fuzzing a well-formed target: the normal-form parser. Fuzzing is the empirical complement to the domain partition (A-04).

**Wrong.** A parser "tested" only with the examples in the protocol document.

**Correct.** The parser runs under a fuzz harness with coverage feedback, in CI, with sanitizers on.

**Scope.** Situations that process untrusted input.

**Level.** MUST: the obligation executes offline (mask row TEST-02: cell O = MUST).

**Enforcement.** M2: CI job runs the fuzz corpus (time-bounded) with sanitizer instrumentation; a crash or hang blocks the change.

#### TEST-03: Differential Tests Against a Reference

**Statement.** Where a reference implementation or a reference behavioral model exists, the candidate molecule is tested differentially: the same inputs are run through both, and the outputs are compared for behavioral equivalence. For molecules with a specification Φ, the reference is `M_Φ`, the canonical molecule of the specification.

**Rationale.** Differential testing checks `M ≈ M_Φ` (the congruence of behavioral equivalence): behavioral equivalence, the congruence that makes equivalence decidable by normal forms (the normal-form lookup table). It is the direct test of the solution predicate (situation solvability).

**Wrong.** A reimplemented protocol tested only against its own tests.

**Correct.** The implementation is run against a reference implementation (or the specification's reference behaviors) over a shared corpus, with behavioral equivalence asserted.

**Scope.** Situations whose molecule has a reference or a specification.

**Level.** MUST where a maintained reference exists; not applicable otherwise: the obligation executes offline (mask row TEST-03: cell O = MUST).

**Enforcement.** M2: CI job runs the differential harness; M3: shared property corpus for both implementations.

#### TEST-04: Cost and Zero-Allocation Regression

**Statement.** Declared hot paths carry cost regression tests: a CI job measures the hot-path allocation count and the cost-budget components (time per message or batch; cache-miss profile where measurable) and fails on regression beyond the declared tolerance. Zero-allocation hot paths are asserted to allocate zero.

**Rationale.** The cost vector `c̄` is part of the situation (situation solvability); a regression that silently exceeds it changes the feasible set: the molecule may no longer be a solution. normalization soundness (cost-nonincreasing normalization) and linearity and zero allocation (allocation-free hot paths) give the checks their grounding: these are properties, not aspirations, so they can be asserted in CI.

**Wrong.** A hot path whose allocation count is never measured and grows with each refactor.

**Correct.** The situation record declares the budget; CI measures and fails on violation.

**Scope.** Situations with a declared hot path.

**Level.** MUST: the obligation executes offline (mask row TEST-04: cell O = MUST).

**Enforcement.** M2: CI job runs the allocation/cost regression (counts allocations in declared hot paths; times the declared workload) and fails the build on regression.

### [INT] Integration

#### INT-01 (Policies Bind Resources, Invariants, and Behavior) Never Architecture

**Statement.** The policies of this standard bind what a molecule does (its behavior, Φ), what it may consume (its resource bounds, `c̄`), and what it preserves (its invariants and laws). No policy of this standard binds how an application is organized (its framework, its runtime, its module structure, its business logic) beyond what is needed to state a molecule's interface, behavior, and budget.

**Rationale.** This is the standard's contract with application authors: the Atom/Molecule architecture governs the data plane, where performance and security are bought, and the policies stop there. Binding application architecture would buy nothing the architecture needs and would make the standard unusable for general application code: the failure mode this standard is curated to avoid.

**Wrong.** A reading of this standard that "requires" an application to adopt a particular event-loop framework because IO-01 says the data path is nonblocking.

**Correct.** IO-01 binds the data-path situation: a molecule; the application may implement its event machinery in any framework, on the control path.

**Scope.** All situations.

**Level.** MUST (mask row INT-01).

**Enforcement.** M4: a standing review question: "this change does not impose application architecture"; and by construction (INT-03), any policy application that would constrain non-hot-path application structure is out of scope.

#### INT-02: No Mandated Framework, Runtime, or Business-Logic Structure

**Statement.** This standard mandates no framework, no runtime, and no business-logic structure. An application may use any event loop, any I/O library that meets the policies' resource and behavior obligations, and any application-level design for code that is not a molecule. The standard's vocabulary (atoms, molecules, situations) is a discipline for the data plane, not a required project layout.

**Rationale.** Enforcement must be complete (every application can conform) and optimal: conformance costs only what performance or security is bought. A mandated framework would make the standard unenforceable for applications that cannot adopt it, violating the universal-enforcement requirement. ARCSS remains the general standard for application structure; this standard does not duplicate it.

**Wrong.** A policy that reads as "the data path must use the project's framework types for all I/O": a framework mandate.

**Correct.** The policy binds the property (nonblocking, batched, bounded); the implementation is the application's choice, checked against the property.

**Scope.** All situations.

**Level.** MUST (mask row INT-02).

**Enforcement.** M2: the conformance job checks policies, not structure (no check inspects framework choice); M4: review guards that no policy is extended into structure.

#### INT-03: Scope Discipline by Construction

**Statement.** Any proposed application of this standard that would constrain non-hot-path application structure, or impose a framework, is out of scope by construction: the policies' scope predicates (Section 4) are applicability tests, and a situation that is not a declared data-path or control-path situation of a molecule is governed by none of the performance policies. The default mask binds the standard to where it buys value.

**Rationale.** The nuance mechanism is what makes the integration guarantee enforceable rather than aspirational: policies apply to situations (typed interfaces with budgets) not to application code in general; the mask (Section 4) fixes the levels; and the override path requires written justification, so any widening of the standard's reach is visible and reviewed.

**Wrong.** Applying ALLOC-01 (zero allocation) to an application's configuration-loading code "because it is code."

**Correct.** ALLOC-01 binds only declared hot paths (its scope); configuration loading on the control path is not in scope.

**Scope.** All situations.

**Level.** MUST (mask row INT-03).

**Enforcement.** M4: review item "policy scope respected: no policy applied outside its situation scope"; M2: conformance job checks only in-scope policies.

---

## 6. Decision Matrices D-1 … D-12

A decision matrix resolves a design choice within a situation. Each matrix below states: **inputs** (the measurements and declarations the choice depends on), **decision rule** (the selection procedure), **rationale** (the governing theorem), and **output** (the policy levels that bind as a result: always consistent with the masks of Tables 4-1/4-2; a matrix selects a design within already-fixed levels, it never creates an obligation).

Inputs must be recorded with the decision in the situation record; an unrecorded input makes the decision unrepeatable, and a decision that cannot be repeated cannot be audited. Where an input is a measurement, the measurement method is part of the record (this is the IO-04/TEST-04 discipline: targets and measurements belong to the situation).

### D-1: Ring and Buffer Sizing (L3-aware)

**Inputs.** Message size μ (maximum and typical); maximum in-flight I (from the SEC-04 caps and the concurrency model); batch volume B (from D-4); the L3 working-set budget W available to this ring (a documented fraction of measured L3, or the L2 budget for the hottest rings); latency budget.

**Decision rule.**
1. Choose K = the smallest power of two with K − 1 ≥ I.
2. If K·μ ≤ W and the batch fits in K, accept K.
3. Otherwise reduce I (tighter backpressure) or shrink B until K·μ ≤ W; if no such K exists, split the ring: per-core rings under CONC-01, each with I/C in-flight per core.
4. Record K, μ, I, W, and the chosen batch size.

**Rationale.** the ring-capacity invariant (bitmask ring correct iff in-flight ≤ K − 1); the hoisting theorem (cache footprint of state is monotone in state size: the ring should stay L3-resident); syscall amortization (batch volume couples the ring and batch budgets).

**Output.** R-02 and R-03 at MUST for data-path situations (mask rows R-02/R-03); R-01 at MUST (the ring is preallocated before the hot path). ALLOC-01 is unaffected: the ring's allocation happened off-path by construction.

### D-2: Structure-of-Arrays vs Array-of-Structures

**Inputs.** Dominant access pattern (field-wise iteration, vectorized access, or whole-record traversal); record size; cache-line size; update locality (which fields are written together); SIMD usage (D-8).

**Decision rule.** Field-wise or vectorized access → SoA. Whole-record sequential traversal with acceptable line waste → AoS. Mixed → split the hot fields into a separate SoA hot block (CACHE-02), leaving the cold fields behind. Record the pattern and the choice.

**Rationale.** the cost-enrichment theorem (cache misses are components of `c̄`; layout determines the miss rate); complexity closure (SoA is the tensor of the field morphisms, and the tensor structure is closed under composition).

**Output.** CACHE-04 at MUST for data-path situations (mask row CACHE-04); CACHE-02 as applicable (hot/cold split).

### D-3: Protocol Selection (UDP/TCP/SCTP)

**Inputs.** Message boundaries (datagram vs. stream); reliability requirement; ordering requirement; partial-delivery requirement; latency target; head-of-line-blocking tolerance; message-size distribution; acceptable congestion behavior.

**Decision rule.**
- Datagram semantics with loss and reordering tolerable → UDP; if reliability or ordering is required, add it as a molecule overlay on the UDP datagram situation (the overlay obeys the same policies).
- Stream semantics with full reliability and ordering required → TCP.
- Multiple independent streams, or partial delivery, → SCTP.
- Latency target below the retransmit floor of a reliable stream → UDP plus an overlay, or UDP with loss masking.
Record the choice and its justification in the situation record (IO-04).

**Rationale.** the cost-enrichment theorem–shortest-path optimality (the cost vector decides: the protocol fixes the per-message fixed-cost floor; choose the feasible protocol with the lowest floor); syscall amortization (batching interacts: datagram protocols batch natively, stream protocols batch records within the stream).

**Output.** IO-04 at MUST (the choice is justified against the declared targets). The choice itself is not policed; the justification is.

### D-4: Batch Size (syscall amortization)

**Inputs.** Fixed syscall/event cost t_s (measured or estimated); per-message processing cost t_m; latency budget L; buffer budget (from D-1); arrival pattern.

**Decision rule.** Maximize n such that `n·t_m + t_s ≤ L` and `n·μ ≤` the buffer budget. Stop increasing n once the marginal gain (`fixed(n)/n`, the amortized fixed cost) falls below a recorded threshold (syscall amortization's diminishing returns). n ≥ 1 always. Record n, the measurement, and the threshold.

**Rationale.** syscall amortization: `cost(batch_n) ≤ n · cost(single) + fixed(n)`, with `fixed(n)/n → 0`; the optimal n is where the amortized fixed cost is dominated by the per-message cost, subject to the latency budget.

**Output.** IO-03 at MUST for data-path situations (mask row IO-03): the data path batches at the computed n; deviation requires a latency-budget justification in the situation record.

### D-5: Polling Strategy

**Inputs.** Packet/request rate R; event rate; the syscall-cost share of per-message cost; zero-copy requirement; latency target; availability of a dedicated core; operational complexity budget.

**Decision rule.**
- Moderate R with a small syscall-cost share → a classic readiness event loop.
- High R where syscall amortization pays → kernel-side submission/completion batching.
- Extreme R combined with a zero-copy requirement and a dedicated core → a zero-copy kernel ring (direct packet access).
- Busy-polling only where the CPU is otherwise idle and the target is below the ordinary event-loop latency.
Record the choice and its measured justification (IO-04). Implementation references for the polling options live in the Appendix; the decision itself is made against the inputs, not against implementation trivia.

**Rationale.** syscall amortization (amortization: kernel-side batching reduces the fixed cost per batch); the zero-copy roundtrip (zero-copy roundtrip elimination is sound only when the domain is well-formed and the transformation is invertible: see D-6).

**Output.** IO-01, IO-02, IO-03 at MUST for data-path situations (mask rows); the choice is documented under IO-04.

### D-6: Zero-Copy vs Copy (the zero-copy roundtrip)

**Inputs.** The transformation applied (does the data pass through parse then serialize, or is it consumed in place?); inverse well-formedness: is `parse; serialize ≈ id` on the declared domain?; data lifetime vs. buffer lifetime; alignment requirements (SIMD-02); the cache effect of copying.

**Decision rule.** Zero-copy when the data is consumed in place and `parse; serialize ≈ id` holds on the domain (the zero-copy roundtrip), or when the data passes through untouched. Copy when the transformation breaks the inverse, or when the data lifetime is short and the copy stays in cache: a short-lived cache-resident copy is cheaper than pinning a remote region. Record the choice and the inverse well-formedness argument.

**Rationale.** the zero-copy roundtrip (roundtrip elimination is sound: on a well-formed domain, parse followed by serialize is the identity, so the roundtrip can be eliminated); cut elimination as deforestation (deforestation: a pure pipeline fuses, so intermediate materialization is removable without changing behavior).

**Output.** ALLOC-01 at MUST for data-path situations (zero-copy serves the zero-allocation budget); SIMD-02 as applicable (alignment before vectorized consumption of zero-copied data); IO-04 (the choice is justified).

### D-7: Dispatch: Monomorphization vs Indirect

**Inputs.** Size of the alternative set; hotness (invocations per unit time); code-size budget; branch predictability of the selector.

**Decision rule.** Hot path with a small alternative set → static resolution (monomorphized, or dense table under MOL-03). Cold path or a large set → indirect dispatch is acceptable. When the selector is data-dependent, measure mispredictions; if they exceed the budget, restructure (D-7 revisits after measurement). Record the choice.

**Rationale.** maximal refinement (the most-refined dispatcher is sound and cost-minimal); unique normal forms–the normal-form lookup table (normal forms are precomputable dense tables over finite signatures).

**Output.** MOL-02 at MUST for data-path situations; MOL-03 at MUST where the set has three or more alternatives on the hot path (mask rows).

### D-8: SIMD Width

**Inputs.** Element type and lane semantics; alignment guarantee (SIMD-02); feature set available at compile time and run time (SIMD-03); range length and remainder; L1 residency of the working set.

**Decision rule.** Choose the widest feature-gated width whose alignment requirement is satisfied and whose remainder is handled scalar (SIMD-01). Prefer the width that keeps the working set L1-resident (CACHE-01). Record the width, the gate, and the remainder strategy.

**Rationale.** the transport theory (a vector op is a tensor of scalar ops; the batch law distributes); the pure-molecules-as-functions theorem (totality on the declared range: the scalar remainder completes the tuple).

**Output.** SIMD-01 at MUST; SIMD-02 and SIMD-03 at MUST for situations with vectorized atoms (mask rows).

### D-9: Lock-Free vs Locking

**Inputs.** Contention rate; critical-section cost; ordering requirements; per-core partitionability (CONC-01).

**Decision rule.** Partition first: per-core state, no sharing at all (CONC-01). Then message passing between partitions. Then lock-free primitives with declared invariants for counters and metadata (the OBS-01 pattern). Locks only on the control path and at startup/shutdown, with documented invariants; never on the data path. Record the choice and the invariants.

**Rationale.** the determinism theorem (independent tensor factors are deterministic (partitioning removes interference); the cost-enrichment theorem (interference must not enter the budget) locks on the data path make cost a function of other threads); complexity closure (the per-core decomposition is closed under composition).

**Output.** CONC-02 at MUST for data-path situations; CONC-03 at SHOULD (orderings argued); CONC-01 as applicable (mask rows).

### D-10: Hot/Cold Field Split

**Inputs.** Per-field access frequencies (measured); record size; cache-line size; update locality.

**Decision rule.** Split fields whose access frequencies differ by an order of magnitude or more. The hot portion must fit one cache line (or a small known number); the cold portion may be packed separately or kept sparse. Record the split and the frequency evidence.

**Rationale.** the hoisting theorem (cache footprint of loop state is monotone in state size); the cost-enrichment theorem (the miss budget is a component of `c̄`).

**Output.** CACHE-02 at MUST for data-path situations (mask row CACHE-02).

### D-11: Allocation Policy

**Inputs.** Hot-path allocation count (target: zero); sizes and lifetimes; variability of sizes; startup budget.

**Decision rule.** No allocation on declared hot paths (ALLOC-01). Working structures are statically sized or preallocated at startup (ALLOC-02). Residual bounded per-request allocation is confined to the control path with declared caps (ALLOC-04). Record the policy outcome for the situation.

**Rationale.** linearity and zero allocation (linearity ⇒ a well-typed hot path never allocates or deallocates); shortest-path optimality (the cost-minimal design is a shortest path in the finite type graph: an unbounded allocation site is outside the graph); the ring-capacity invariant (rings need fixed capacity to be correct).

**Output.** ALLOC-01, ALLOC-02, ALLOC-04 at the levels of their mask rows.

### D-12: Plugin Placement

**Inputs.** Plugin trust level; performance requirements (data path or cold); update frequency; failure impact; isolation needs.

**Decision rule.** In-process when latency-critical and the trust level is accepted, with ABI versioning (PLUGIN-01), health checks (PLUGIN-02), and a built-in fallback (PLUGIN-03). Out-of-process, or cold-path-only, when isolation or update frequency demands it. Never on the data path without a fallback. Record the placement and its rationale.

**Rationale.** refinement substitution (refinement substitution: the fallback refines the same Φ at known cost); finiteness of normal forms and the lookup table (a versioned signature makes the plugin interface decidable and comparable).

**Output.** PLUGIN-01, PLUGIN-02, PLUGIN-03 at the levels of their mask rows for plugin-loading situations.

---

## 7. Situations

### 7.1 Formal definition

A **situation** is a triple `(A→B, Φ, c̄)`:

- `A → B`: the interface: input type A, output type B;
- `Φ`: a behavioral specification predicate: a decidable statement of the behavior the solution must exhibit;
- `c̄ = (t̄, m̄, k̄)`: a cost budget vector: budgets for time, memory, and cache misses.

A **solution** to a situation is a molecule `M : A → B` such that:

1. `M ⊢ Φ`: M satisfies the specification, equivalently `M ≈ M_Φ` where `M_Φ` is the canonical molecule of Φ (behavioral equivalence, the congruence of behavioral equivalence, decided by normal forms, the normal-form lookup table); and
2. `cost(M) ≤ c̄`: componentwise cost dominance, where costs are additive over composition and subadditive over tensor (the cost-enrichment theorem).

The **feasible set** (all molecules satisfying 1 and 2) is finite and decidable for bounded types and additive costs (situation solvability). The decision problem "find the solution of minimal cost" is therefore a finite search: a shortest-path computation over the finite type graph (shortest-path optimality). The decision matrices of Section 6 are the practical form of that minimization: each matrix restricts the search to the designs consistent with the situation's binding levels, and the conjunction of the matrices selects the point in the feasible set that the situation record documents.

The **decision matrix** of a situation is, formally, the minimization over the feasible set. This standard operationalizes it as the twelve D-matrices applied in sequence; the situation record's matrix entries are the witness that the minimization was performed and is repeatable.

### 7.2 The situation record

Every situation an application declares is described by a **situation record** containing:

1. the triple `(A→B, Φ, c̄)` and the situation **kind** (Section 4.3);
2. the solution molecule `M`, its composition tree (MOL-01), its state space S (A-02, MOL-04), and its specification claim `M ⊢ Φ` (MOL-05);
3. the policy levels that bind (the mask outcome for the kind, filtered by each policy's scope predicate);
4. the decision-matrix outcomes (which D-matrices were applied, their inputs, and their results);
5. any mask overrides and waivers, with justifications and reviewer sign-off.

The situation record is where conformance is demonstrated. A project with no data path has no data-path obligations; a project that declares a data path declares its situations and shows, per record, that each binding policy passed its enforcement gate.

### 7.3 Canonical recipes

The following recipes are worked applications of the general policies to the canonical FDS application types. They are not new policies; they show how the situation records of these applications are filled in.

#### Recipe 7.1: Web server (HTTP over TCP)

**Situations.**
- *(Listen → Accepted)*: startup/control path: nonblocking accept, per-connection state allocated here (R-01, ALLOC-02), caps declared (SEC-04: `MAX_CONNECTIONS`).
- *(Conn, Request → Response)*: data path: per-connection ring (D-1), HTTP parser (A-04 domain partition: well-formed / truncated / trailing-garbage), request molecule, response serializer, write batching (D-4). Keep-alive pipelines requests into the batch (batch flattening batch flattening).
- *(Idle → Close)*: control path: teardown, draining (R-03 at shutdown).

**Molecules.** The per-request molecule factors as `q ∘ e ∘ p` (Kleisli decomposition): `p` parses and validates the request (pure; fuzzed, TEST-02; length caps, SEC-01); `e` performs I/O through Ctx (nonblocking, IO-01; drains, IO-02; batches, IO-03); `q` serializes the response (pure; the inverse of `p` on the well-formed domain, the zero-copy roundtrip: a serialization roundtrip in a proxy situation is eliminated, D-6). State: per-connection ring and parse state, minimal (MOL-04). Dispatch on method codes is dense-integer (MOL-03); the parser's choice structure follows left-factored normal forms (parser left factoring).

**Binding notes.** [R] all at MUST; [ALLOC]-01 at MUST (zero allocation per request); [SEC]-01/02/03/04 at MUST; [IO]-01/02/03/04 at MUST; [TEST]-01 (parser and batch laws), -02 (fuzz the parser), -03 (differential against a reference HTTP stack), -04 (zero-alloc regression) at MUST offline; D-3 → TCP (stream reliability), D-4 → batch size, D-6 → copy unless the pipeline is a pure proxy, D-7 → static dispatch on method codes.

#### Recipe 7.2: DNS server (UDP + TCP)

**Situations.**
- *(Query packet → Response packet)* (data path over UDP: datagram parse (SEC-01: query size cap, name-length caps, compression-pointer bounds) a parser law, the parser theory and left factoring), cache lookup (bounded cache, ALLOC-02), response build, multi-message batching (IO-03, D-4), ring of outstanding queries (R-02/R-03, D-1).
- *(Stream query → Response)*: data path over TCP for large responses: length-prefixed framing (SEC-01), same molecule core.
- *(Cache maintenance)*: control path: eviction, zone loading at startup (U).

**Binding notes.** Two data-path situations, two protocols, both justified by message-size targets under D-3 (datagram for the common case, stream for the large-response case). [TEST]-01 asserts the name-parse laws; [TEST]-02 fuzzes names and compression; [TEST]-03 is differential against a reference resolver on a shared corpus. The cache is a bounded collection (ALLOC-02) whose caps are declared (SEC-04).

#### Recipe 7.3: FTP server (control + data channels)

**Situations.**
- *(Command line → Reply)* (control path: a low-rate command channel. Dense-integer dispatch on the command codes (MOL-03) the command set is fixed and enumerable); the command grammar is a finite automaton realized at minimal state (MOL-04); line-length caps (SEC-01); NOOP and keep-alive handling.
- *(Transfer command → Data stream)*: data path: the transfer situations (LIST/RETR/STOR). Per-connection rings (D-1), batching (IO-03, D-4), zero-allocation transfer path (ALLOC-01), nonblocking data-channel I/O (IO-01/02).
- *(Session setup)*: control path: passive/active connection negotiation, data-channel setup at startup of the transfer.

**Binding notes.** This is the canonical control-plus-data split: the protocol's own architecture (control channel, data channel) maps directly onto the situation kinds, and the mask binds the performance policies at MUST only on the data channel. The control channel is where state, health, and commands live, and where blocking and allocation are permitted at SHOULD/MAY strength.

#### Recipe 7.4: Custom protocol

**Procedure.** An application author with a novel wire format proceeds as follows:

1. Declare the situation triple `(A→B, Φ, c̄)`: the wire format's messages are the interface; Φ states the protocol's behavioral contract (request-response matching, error behavior, ordering); `c̄` states the latency, memory, and cache budgets (SEC-04 caps included).
2. Classify the kind(s), steady-state message processing is D; setup and handshake are C or U.
3. Apply the D-matrices: D-3 (protocol choice), D-1 (rings), D-4 (batch), D-6 (zero-copy vs copy, given the wire format's inverse well-formedness), D-7 (dispatch on message kinds), D-8 (SIMD for fixed-width fields, e.g., header arrays), D-9 (concurrency), D-10/D-11 as the state shape demands, D-12 if plugins are involved.
4. Build the solution molecule from atoms in the `q ∘ e ∘ p` shape, recording the composition tree and the state space.
5. Ship the law tests (TEST-01), fuzz the parser (TEST-02), compare against the reference or the specification (TEST-03), and declare the cost regression (TEST-04).

The recipe is the general method; the other recipes are its worked instances.

---

## 8. Integration Guarantee

**The guarantee.** The policies of this standard bind **resources, invariants, and behavior**: what a molecule consumes, what it preserves, and what it does. They **never bind application architecture**: no framework, no runtime, no module layout, no business-logic structure is mandated, forbidden, or presupposed. Formally: for every policy, the set of situations it can bind is exactly the set that policy's Scope and the default mask assign; anything else is out of scope by construction (INT-03).

**What this means for application authors.**

- **You may build a normal application.** The data plane (the molecules that carry steady-state work under tight budgets) is governed by this standard. Everything else is yours: your event loop, your framework, your application-layer modules, your testing style outside the mandated harnesses. The performance policies bind only declared hot paths; a situation that is not a declared data-path situation is governed by none of them (INT-03).
- **The vocabulary is a discipline, not a layout.** You are not required to organize your project as a collection of "atoms" files and "molecules" files. You are required to be able to point at a situation record and say which molecule satisfies it, which state it holds, and which policies bind.
- **General standards still apply.** ARCSS remains the general standard for application code. This standard does not relax ARCSS; it governs the Atom/Molecule data plane and leaves the rest to ARCSS, so an application author has exactly one general authority (ARCSS) and one data-plane authority (this standard), with no overlap in the situations they bind.
- **The standard is enforced, not advisory.** Conformance is demonstrated in situation records and the waiver register; enforcement uses the four universal mechanisms, all of which run on any project. An application that declares no data path simply has no data-path obligations: completeness of enforcement does not mean omnipresence of enforcement.
- **The override path is visible.** If your application genuinely needs a different binding level for a situation, the mechanism exists (Section 4.4): a written justification and a reviewer sign-off in the situation record. What the standard forbids is silent deviation.
- **No weird restrictions.** A restriction is a policy that constrains what you may build without buying performance or security. This standard's curation criterion is exactly that: a policy exists only where it buys performance or security for the architecture. The [INT] category exists to make that criterion itself a policy, enforced in every review.

---

## 9. Glossary

Terms marked * are defined here for this standard's purposes; terms marked † are defined in ARCSS and used with their ARCSS meanings; all other terms are defined in Section 2.

- **Atom**: a transformation with no internal composition; the smallest unit of a data path. Pure (total function, no effects) or effectful (acts only through Ctx). See A-01–A-04.
- **Behavioral specification Φ**: a decidable predicate stating the behavior a solution to a situation must exhibit. See MOL-05.
- **Behavioral equivalence (≈)**: bisimulation: two molecules are equivalent when they produce indistinguishable input/output behavior; a congruence for ∘ and ⊗ (the congruence of behavioral equivalence); decidable by normal forms (the normal-form lookup table).
- **Buffer**: a preallocated, bounded storage region used by a data path; allocated before the hot path and never resized on it. See [R].
- **Ctx**: the effect context of an effectful atom or molecule: the set of capabilities (I/O handles, clocks, allocation) it may touch and nothing else. See the Kleisli characterization, A-01.
- **Compensating control**: the control that restores the property a waived policy would have provided; required by the single waiver clause (Section 3).
- **Control path**: the low-frequency, latency-tolerant work of a situation: setup, handshakes, configuration, teardown, error recovery. See Section 2.
- **Cost budget vector c̄**: `c̄ = (t̄, m̄, k̄)`: budgets for time, memory, and cache misses that a solution must meet componentwise (the cost-enrichment theorem, situation solvability).
- **Data path**: the steady-state per-message or per-batch execution of a data-path situation; the hot path. See Section 2.
- **Decision matrix**: a procedure (Section 6) that resolves a design choice within a situation, from recorded inputs; formally, the minimization over the feasible set (shortest-path optimality, situation solvability).
- **Dense dispatch**: selecting among a fixed set of alternatives by dense integer key into a lookup structure, instead of chained conditionals. See MOL-03.
- **Effectful atom / effectful molecule**: an atom/molecule that acts through Ctx (the Kleisli characterization). See A-01.
- **Feasible set**: the set of molecules satisfying `M ⊢ Φ` and `cost(M) ≤ c̄`; finite and decidable for bounded types and additive costs (situation solvability).
- **Feature gate**: the conditional enabling of a vector (or other) path on a detected hardware feature, paired with a behaviorally equivalent fallback. See SIMD-03.
- **Hot path**: the declared steady-state execution of a data-path situation; the scope predicate of the performance policies. See Section 2.
- **Hybrid molecule**: a molecule mixing pure and effectful steps; uniquely `q ∘ e ∘ p` (Kleisli decomposition).
- **In-flight**: elements placed in a ring but not yet removed; bounded by `capacity − 1` (the ring-capacity invariant, R-03).
- **Mask (Situation Application Matrix)**: the per-situation default assignment of effective policy levels (MUST/SHOULD/MAY/n/a) by situation kind; overridable only with written justification (Section 4.4).
- **Molecule**: a stateful transformation `M : A → B`; an equivalence class of `(S, step)` pairs (the category axioms–the bijection-quotient theorem). See Section 2.
- **Normal form NF(M)**: the unique (up to coherence) normal form of M under the runtime's equational theory; `NF(M) ≈ M`, and equivalence is decided by comparing normal forms (unique normal forms–the normal-form lookup table).
- **Offline situation**: build-time, test-time, and tooling contexts; no runtime cost budget. See Section 4.3.
- **Override**: a documented, reviewed change to the mask's binding level for one specific situation (Section 4.4). Distinct from a waiver.
- **Plugin**: a molecule composed into a host across a versioned external interface; governed by [PLUGIN].
- **Property test**: a test that asserts a law or property over generated inputs from a declared domain; the mechanism M3.
- **Pure atom / pure molecule**: a total function with no observable effects (the pure-molecules-as-functions theorem); deterministic (the determinism theorem).
- **Refinement (⊑)**: `f ⊑ f′`: f′ preserves the behavior of f at no greater cost (refinement substitution); maximal refinement is unique and its dispatcher cost-minimal (maximal refinement).
- **Ring**: a bounded FIFO with `push`/`pop` obeying the ring equations (the ring equations); power-of-two capacity with bitmask indexing when `2^k` (the ring-capacity invariant). See [R].
- **Scope (policy)**: a decidable applicability predicate over situations; false scope ⇒ the policy binds nothing. See Section 4.1.
- **Situation**: the triple `(A→B, Φ, c̄)`; the unit of conformance for this standard. See Section 7.
- **Situation kind**: one of data-path (D), control-path (C), startup (U), shutdown (S), offline (O). See Section 4.3.
- **Situation record**: the per-situation artifact demonstrating conformance: the triple, kind, solution molecule, binding levels, matrix outcomes, overrides, and waivers. See Section 7.2.
- **Solution**: a molecule M with `M ⊢ Φ` and `cost(M) ≤ c̄` (situation solvability).
- **State space S**: the complete declared memory of a transformation; `step : S × A → B × S`. See Section 2.
- **Step function**: the function `step : S × A → B × S` of a stateful transformation (the category axioms).
- **Tensor product (⊗)**: parallel composition of molecules; the tuple product with unit `()` (the symmetric monoidal structure).
- **Trust boundary**: the point where untrusted input enters the system; the site of validation and of the security policies [SEC].
- **Universal enforcement mechanism**: one of M1: static check), M2: CI job), M3: property-test harness), M4: mandatory review item); a mechanism that works for any application type. See Section 3.
- **Waiver**: the single clause excusing non-compliance with a binding policy: documented deviation + compensating control + named reviewer, recorded in the waiver register. See Section 3.
- **Zero allocation**: a declared hot path performing no allocation or deallocation; grounded in linearity and zero allocation and enforced by TEST-04.

---

## 10. Appendix: Implementation Guidance (Non-Normative)

**This appendix is explicitly non-normative.** It is FDS-specific reference material: socket-option catalogs, candidate crates, and SIMD notes for the FDS transport engine and the applications built on it. It is guidance, not policy. No policy of this standard depends on it, and no obligation in this standard is stated in its terms; the policies in Section 5 are deliberately general precisely so that they never reference a socket option, a crate, or an intrinsic. This appendix is where the FDS-specific knowledge lives so that the policies do not have to.

### 10.1 Socket-option catalogs per protocol

These catalogs support the IO-04 discipline (options justified by latency/throughput targets) and the decision matrices D-3, D-5, and D-6. They are starting points, not mandates; every option's use must be justified against the situation's declared targets and measured.

**UDP.**
- `SO_RCVBUF` / `SO_SNDBUF`: socket buffer sizing. For high-throughput datagram work, 4–16 MB per direction is a common working range; sizes must be justified by the D-1 buffer budget and the in-flight caps (R-03), not set blindly.
- `UDP_SEGMENT` / GSO (generic segmentation offload), send large segments in one call, letting the stack segment them; a batching aid for IO-03/D-4.
- `UDP_GRO` (generic receive offload) (receive coalescing of related datagrams; reduces per-packet fixed cost, interacts with the parser laws (a coalesced payload is several datagrams, which the parser must still handle one-by-one) the transport theory).
- `SO_ZEROCOPY`: send-side zero copy; relevant to D-6 (zero-copy is sound when the data passes through untouched or the roundtrip is invertible, the zero-copy roundtrip).

**TCP.**
- `TCP_NODELAY`: disable the delayed-ack/nagle interaction for latency-sensitive request-response situations; a D-4-adjacent latency choice (the batch is the molecule's own, not the stack's).
- `TCP_QUICKACK`: request immediate acks on a latency-critical connection; use where the target justifies the extra ack traffic.
- `TCP_DEFER_ACCEPT`: delay accept until data arrives; moves connection establishment off the hot path onto the control path (C/U).
- `TCP_FASTOPEN`: send data with the SYN, saving a round trip on session establishment. **Caveat:** the fast-open cookie is an amplification vector for spoofing attacks; enable only where the spoofing caveat is accepted and the SEC policies' caps (SEC-04) bound the exposure.
- `TCP_CORK`: accumulate a stream before flushing. **Caveat:** it adds latency while corked; use only where the throughput target dominates the latency budget, and prefer the molecule's own batching (IO-03/D-4) over stack-side corking.

**SCTP.**
- `SCTP_NODELAY`: per-message send semantics without delay; the SCTP analogue of `TCP_NODELAY`.
- `SCTP_EVENTS`: select which events the socket delivers; keep only the events the situation's state machine (MOL-04) actually consumes.
- `SCTP_INITMSG`: cap the number of streams and the initialization timeout for the association; a startup/control-path cap (SEC-04).
- `SCTP_PARTIAL_DELIVERY_POINT`: control when partial messages are delivered to the application; relevant when a message spans many fragments and the application can make progress on a prefix.
- `SCTP_MAX_BURST`: bound the number of packets sent per transmission burst; a throughput/rate cap.
- `SCTP_PEELOFF`: peel a branch of an association off into its own socket; the SCTP form of per-stream partitioning (CONC-01).
- `sctp_bindx`: bind a socket to multiple addresses for multihoming; a control-path setup operation.

### 10.2 Candidate crates

Candidate crates for FDS and applications built on it, listed as guidance. Their suitability is judged against the policies (boundedness, lock-freedom, alignment, provenance; SEC-05) not by brand. All are subject to the dependency audit (SEC-05).

- **socket2**: direct, typed control of socket options and flags; the idiomatic surface for the option catalogs above.
- **rustix**: small, direct syscall wrappers; a low-overhead path to the same surface.
- **libc**: raw FFI for the options and calls the two above do not cover; confine unsafe use per the project's TCB discipline.
- **io-uring**: kernel-side submission/completion batching; the D-5 kernel-side option.
- **core_affinity**: per-core pinning in support of CONC-01 partitioning.
- **heapless**: fixed-capacity collections; a direct implementation of ALLOC-02.
- **arrayvec**: fixed-capacity arrays with length tracking; the same discipline in array form.
- **static_assertions**: compile-time assertions for R-02 (power-of-two capacities), CACHE-01 (alignment), and ALLOC-02 (caps).
- **crossbeam-utils**: cache-padded cells for CACHE-03 (false-sharing avoidance).
- **memmap2**: memory-mapped buffers for cold-path or file-backed work; relevant to D-6 where mapping beats copying.
- **arc-swap**: lock-free read-mostly shared configuration; a CONC-02/OBS-01-pattern primitive for control-path configuration publication.
- **httparse**: a zero-copy HTTP parser; a candidate for the Recipe 7.1 parser molecule, subject to the same SEC-01/TEST-02 obligations as any parser.
- **nom**: a combinator parser library; a candidate basis for custom-protocol parsers (Recipe 7.4), whose laws (the parser theory and left factoring) are then tested by TEST-01.

### 10.3 SIMD notes

- **`std::arch`**: portable SIMD intrinsics gated by `is_x86_feature_detected!`-style checks; the standard surface for SIMD-03 (feature-gated with fallback).
- **`wide`**: a safer wrapper crate over the same intrinsics; its fixed-width lane semantics still require the SIMD-01 remainder discipline (whole vectors plus a scalar tail) and the SIMD-02 alignment guarantee.
- **Bounds discipline.** The non-negotiable part: vector loops process `len - (len % W)` lanes vectorized and the remainder scalar, never beyond `len`; alignment is established before the vector path (static declaration or runtime check); both paths are tested against the same laws (SIMD-03, TEST-01).

*End of standard.*
