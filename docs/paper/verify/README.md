# fds-verify — proof-checking tools for the FDS thesis (Appendix A)

Std-only Rust binaries (no crates.io dependencies; build offline with `cargo build`).
Each tool prints PASS/FAIL per check and exits non-zero on any failure.
Run outputs are captured in `logs/<tool>.log`; `logs/SUMMARY.log` aggregates them.

| Binary | Theorem checked | What it verifies |
|--------|-----------------|------------------|
| `kb_completion` | NT15 | Knuth–Bendix completion of the ring-buffer equational theory (`push(pop(x))→x`, `pop(push(x))→x`): termination, all critical pairs join (confluence), the original rules present, the system decides the theory on samples. |
| `normal_forms` | NT22–NT24 | Finite normal-form enumeration over a small atom signature: finiteness of distinct normal forms within the depth bound; completeness (equal normal forms ⟺ equal composed function) on all pairs; example equivalences. |
| `contraction` | NT43 | Iteration-bound arithmetic `k(α,ε,d₀) = ⌈ln(ε(1−α)/d₀)/ln α⌉`: bound finite and ≥ 1; simulation `x_{n+1}=αx_n` reaches tolerance; monotonicity in α and ε. |
| `bisim` | NT6 | Behavioral equivalence (partition refinement) is a congruence on small finite mealy machines: equivalence relation (reflexive/symmetric/transitive) and `M≈M′ ⇒ M∘N≈M′∘N, N∘M≈N∘M′, M⊗N≈M′⊗N` over a 2,260-machine universe. |
| `batch_amort` | NT47 | Syscall-amortization bound `cost(batch_n) ≤ n·cost(single)` and non-increasing amortized cost for n = 1..=1024. |
| `affine_typer` | NT55 | Linear typing in the affine system Λ: every variable consumed exactly once, contraction and weakening rejected, and evaluation preserves the leaf multiset and never increases node count (no allocation). |

Run all six:

```sh
cargo build
for b in kb_completion normal_forms contraction bisim batch_amort affine_typer; do
  cargo run --bin "$b" | tee "logs/$b.log"
done
```

The thesis build (`../build.sh --verify`) runs these and checks `logs/SUMMARY.log` for any FAIL.
