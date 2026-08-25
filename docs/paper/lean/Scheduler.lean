import Std

set_option linter.unusedSimpArgs false

/-!
# NT63 — The admission-scheduler EMA is a contraction with a fixed band

The Atomos integer admission scheduler maintains a demand estimate

    d' = d + n - d/8        (Nat; the shipped code is `d + n - (d >> 3)`)

over the nonnegative integers, where `n` is the observed demand.  This
file proves the elementary facts that make the tuning a theorem:

  1. the band [8n, 8n+7] is the fixed set of the update (the scheduler
     settles there);
  2. below the band the estimate strictly increases and stays ≤ 8n;
  3. above the band it strictly decreases;
  4. the update is nondecreasing in its state.

Together with NT42-NT44 this gives the iteration bound for the band
crossing time.  Formalized over `Nat` (u64 semantics; `n` the demand,
`x` the running estimate).
-/

namespace Scheduler

/-- The integer EMA update: `f n x = x + n - x/8`. -/
def f (n x : Nat) : Nat := x + n - x / 8

/-- The fixed band: for `x ∈ [8n, 8n+7]`, `f n x = x`. -/
theorem band_fixed {n x : Nat} (hlo : 8 * n ≤ x) (hhi : x ≤ 8 * n + 7) :
    f n x = x := by
  unfold f
  have hdiv : x / 8 = n := by
    have hx : x = 8 * (x / 8) + x % 8 := (Nat.div_add_mod x 8).symm
    have hmod : x % 8 < 8 := Nat.mod_lt x (by omega : 0 < 8)
    omega
  omega

/-- Below the band the update strictly increases the estimate. -/
theorem below_band_increases {n x : Nat} (h : x < 8 * n) : x < f n x := by
  unfold f
  have hdiv : x / 8 < n := by omega
  omega

/-- Below the band the update stays at or below the top of the band. -/
theorem below_band_bounded {n x : Nat} (h : x < 8 * n) : f n x ≤ 8 * n := by
  unfold f
  have hdiv : x / 8 < n := by omega
  omega

/-- Above the band the update strictly decreases the estimate. -/
theorem above_band_decreases {n x : Nat} (h : 8 * n + 8 ≤ x) : f n x < x := by
  unfold f
  have hdiv : n < x / 8 := by omega
  omega

/-- The update is nondecreasing in its state: larger estimates stay
larger after one step (with demand fixed). -/
theorem monotone {n a b : Nat} (hab : a ≤ b) : f n a ≤ f n b := by
  unfold f
  -- a + n - a/8 <= b + n - b/8  iff  b/8 - a/8 <= b - a
  have hdiv : a / 8 ≤ b / 8 := Nat.div_le_div_right hab
  omega

end Scheduler
