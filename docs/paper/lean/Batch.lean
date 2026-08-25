import Std

set_option linter.unusedSimpArgs false

/-!
# NT57 — The boundary enrichment law for batching

The kernel-boundary coordinate of the cost vector counts syscalls: the
boundary count β(M) of a molecule is the number of kernel crossings per
steady-state run.  Two laws make it a useful graded enrichment:

  1. tensor additivity: β(f ⊗ g) = β(f) + β(g) — disjoint factors pay
     disjoint syscalls (the NT25 tensor law for the boundary);
  2. batch subadditivity: batching m+n items costs at most the sum of
     the separate batches — and for the single-syscall batch, one
     boundary crossing replaces n: β(batch_n) = 1 < n for n ≥ 2.

Formalized as arithmetic on counts (per-item syscalls when unbunched,
one shared syscall when batched).
-/

namespace Batch

/-- Boundary count of `n` unbunched single-item syscalls. -/
def beta_single (n : Nat) : Nat := n

/-- Boundary count of one batched syscall of `n` items (0 when idle). -/
def beta_batch (n : Nat) : Nat := if n = 0 then 0 else 1

/-- Tensor additivity: the boundary count of the disjoint union is the
sum of the parts. -/
theorem tensor_additive (m n : Nat) :
    beta_single (m + n) = beta_single m + beta_single n := by
  unfold beta_single
  omega

/-- Batch subadditivity under the tensor: sharing one batch across the
two factors can only reduce the boundary count. -/
theorem batch_subadditive (m n : Nat) :
    beta_batch (m + n) ≤ beta_batch m + beta_batch n := by
  unfold beta_batch
  split <;> split <;> split <;> omega

/-- Batching fuses: one batched syscall replaces n single syscalls for
any n ≥ 2 (the measured recvmmsg/sendmmsg amortization law). -/
theorem batch_fuses {n : Nat} (h : 2 ≤ n) : beta_batch n < beta_single n := by
  unfold beta_batch beta_single
  have hn : n ≠ 0 := by omega
  rw [if_neg hn]
  omega

end Batch
