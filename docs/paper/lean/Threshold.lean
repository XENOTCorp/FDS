import Std

set_option linter.unusedSimpArgs false

/-!
# NT58 — The affine crossover (threshold) theorem

Two realizations of *one* behavior class with payload-size-parameterized
cost over `Int`:

    c1(n) = a1*n + b1      c2(n) = a2*n + b2

When `a1 > a2` (R1 is steeper: cheaper per byte *below* the crossover,
R2 cheaper per byte *above*), the difference

    diff(n) = c2(n) - c1(n) = (a2-a1)*n + (b2-b1)

is strictly decreasing: there is at most one crossover, and the sharper
realization wins on each side of it.  This is the formal anchor of the
measured sendfile-vs-wire-cache crossover on this box
(c_byte(n) = 5.92e-4 n - 1.32 us, c_sendfile(n) = 2.90e-4 n + 40.45 us,
crossover at 134.9 KiB vs the measured ~128 KiB).
-/

namespace Threshold

/-- Payload-size-parameterized affine cost. -/
def cost (a b : Int) (n : Int) : Int := a * n + b

/-- One step of the difference shifts by the slope difference: diff(n+1) =
diff(n) + (a2 - a1). -/
theorem diff_step (a1 a2 b1 b2 : Int) (n : Int) :
    cost a2 b2 (n + 1) - cost a1 b1 (n + 1) =
      cost a2 b2 n - cost a1 b1 n + (a2 - a1) := by
  unfold cost
  simp [Int.mul_add, Int.add_assoc, Int.add_comm, Int.add_left_comm,
        Int.mul_comm, Int.mul_assoc, Int.mul_left_comm]
  omega

/-- Shifting by `k` adds `k * (a2 - a1)` to the difference. -/
theorem diff_add (a1 a2 b1 b2 : Int) (n k : Int) :
    cost a2 b2 (n + k) - cost a1 b1 (n + k) =
      cost a2 b2 n - cost a1 b1 n + k * (a2 - a1) := by
  unfold cost
  simp [Int.mul_add, Int.add_mul, Int.mul_sub, Int.sub_mul,
        Int.add_assoc, Int.add_comm, Int.add_left_comm,
        Int.mul_comm, Int.mul_assoc, Int.mul_left_comm]
  omega

/-- The difference is strictly decreasing in `n` when R2 is cheaper per
byte (`a2 < a1`): for `n < m`, `diff(m) < diff(n)`. -/
theorem diff_strictly_decreasing (a1 a2 b1 b2 : Int) (hslope : a2 < a1) :
    ∀ {n m : Int}, n < m →
      cost a2 b2 m - cost a1 b1 m < cost a2 b2 n - cost a1 b1 n := by
  intro n m hnm
  have hd : a2 - a1 ≤ -1 := by omega
  -- diff(m) = diff(n + (m-n)) = diff(n) + (m-n)*(a2-a1)
  have hshift := diff_add a1 a2 b1 b2 n (m - n)
  have hsum : n + (m - n) = m := by omega
  rw [hsum] at hshift
  -- the shift term is strictly negative: (m-n) >= 1 and (a2-a1) <= -1
  have hprod : (m - n) * (a2 - a1) < 0 := by
    have h1 : (m - n) * (a2 - a1) ≤ (m - n) * (-1) := by
      exact Int.mul_le_mul_of_nonneg_left hd (by omega : 0 ≤ m - n)
    have hmul : (m - n) * (-1) = -(m - n) := by simp [Int.mul_neg]
    have hneg : -(m - n) ≤ -1 := by omega
    omega
  omega

/-- Threshold optimality, sharp form.  If `n*` is the crossover point
(R1 at most as expensive as R2 at `n*`, and R2 strictly cheaper at
`n*+1`), then R1 (the steeper slope) wins at and below `n*`, and R2
(the shallower slope) wins at and above `n*+1`: the realization switch
happens exactly once. -/
theorem threshold_optimal (a1 a2 b1 b2 nstar : Int) (hslope : a2 < a1)
    (hcross : cost a1 b1 nstar ≤ cost a2 b2 nstar)
    (hnext : cost a2 b2 (nstar + 1) < cost a1 b1 (nstar + 1)) :
    (∀ n : Int, n ≤ nstar → cost a1 b1 n ≤ cost a2 b2 n) ∧
    (∀ n : Int, nstar + 1 ≤ n → cost a2 b2 n < cost a1 b1 n) := by
  constructor
  · intro n hn
    by_cases heq : n = nstar
    · subst n; omega
    · have hlt : n < nstar := by omega
      have hdec := diff_strictly_decreasing a1 a2 b1 b2 hslope (n := n) (m := nstar) hlt
      omega
  · intro n hn
    by_cases heq : n = nstar + 1
    · subst n; omega
    · have hlt : nstar + 1 < n := by omega
      have hdec := diff_strictly_decreasing a1 a2 b1 b2 hslope (n := nstar + 1) (m := n) hlt
      omega

end Threshold
