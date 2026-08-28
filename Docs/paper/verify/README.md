# fds-verify: proof-checking tools for the thesis (Appendix A)

Std-only Rust binaries (no crates.io dependencies; build offline with `cargo build`).
Each tool prints PASS/FAIL per check and exits non-zero on any failure.
Run outputs are captured in `logs/<tool>.log`; `logs/SUMMARY.log` aggregates them.

| Binary | Theorem checked | What it verifies |
|--------|-----------------|------------------|
| `kb_completion` | the completion theorem | Knuth–Bendix completion of the stack/pointer fragment (`push(pop(x))→x`, `pop(push(x))→x`): termination, all critical pairs join (confluence), the original rules present, the system decides the theory on samples. These equations are LIFO/pointer identities; they fail for FIFO content. |
| `normal_forms` | the normal-form theorems | Finite normal-form enumeration over a small atom signature: finiteness of distinct normal forms within the depth bound; completeness (equal normal forms ⟺ equal composed function) on all pairs; example equivalences. |
| `contraction` | the iteration-bound theorem | Iteration-bound arithmetic with `d₀ = d(x₀, F x₀)`: `k(α,ε,d₀) = ⌈ln(ε(1−α)/d₀)/ln α⌉`. Bound finite and ≥ 1; simulation `x_{n+1}=αx_n` from `x₀ = d₀/(1−α)` reaches tolerance; monotonicity in α and ε. |
| `bisim` | the congruence theorem | Behavioral equivalence (partition refinement) is a congruence on small finite mealy machines: equivalence relation (reflexive/symmetric/transitive) and `M≈M′ ⇒ M∘N≈M′∘N, N∘M≈N∘M′, M⊗N≈M′⊗N` over a 2,260-machine universe. |
| `batch_amort` | the syscall-amortization theorem | Syscall-amortization bound `cost(batch_n) ≤ n·cost(single)` and non-increasing amortized cost for n = 1..=1024. |
| `affine_typer` | the no-allocation theorem | Linear combinator calculus Λ: sequential composition of morphisms and the tensor bifunctor; every variable consumed exactly once; contraction and weakening rejected; evaluation preserves the leaf multiset and never increases node count (syntax tree, not dataplane heap). |

Run all six:

```sh
cargo build
for b in kb_completion normal_forms contraction bisim batch_amort affine_typer; do
  cargo run --bin "$b" | tee "logs/$b.log"
done
```

The thesis build (`../build.sh --verify`) runs these and checks `logs/SUMMARY.log` for any FAIL.
