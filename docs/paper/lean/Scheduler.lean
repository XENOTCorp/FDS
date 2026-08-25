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

/-!
# NT62 cycle — the decay-then-update attractor

Between requests the scheduler decays demand (`tick_decay`):
`d -> (7*d)/8`. The next request updates `d -> d + 1 - d/8` (the EMA of
`f` above with n = 1). The composite

    cycle d = f 1 (decay d)

keeps the firewall datapath inside [7, 13] once it is there, and pulls
every state at or above 14 strictly downward. The code test
`decay_cycle_contracts_into_attractor` in `src/kernel/sched.rs` runs the
same seven-state check; this file machine-checks it.
-/

/-- `tick_decay`: `d -> (7*d) >> 3`, the integer form. -/
def decay (d : Nat) : Nat := (7 * d) / 8

/-- One decay followed by one n = 1 update. -/
def cycle (d : Nat) : Nat := f 1 (decay d)

/-- The attractor [7, 13] is invariant under the cycle (all seven
states, checked exhaustively). -/
theorem cycle_keeps_attractor (d : Nat) (hlo : 7 ≤ d) (hhi : d ≤ 13) :
    7 ≤ cycle d ∧ cycle d ≤ 13 := by
  have h : d = 7 ∨ d = 8 ∨ d = 9 ∨ d = 10 ∨ d = 11 ∨ d = 12 ∨ d = 13 := by omega
  rcases h with rfl | rfl | rfl | rfl | rfl | rfl | rfl
  all_goals native_decide

/-- Above the attractor the cycle is a strict contraction:
`cycle d < d` for every `d >= 14`, so iterating the cycle converges
into [7, 13]. -/
theorem cycle_contracts_above (d : Nat) (h : 14 ≤ d) : cycle d < d := by
  unfold cycle decay f
  -- cycle d = (7d/8) + 1 - (7d/8)/8 <= (7d/8) + 1, and (7d/8) <= d - 2,
  -- so cycle d <= d - 1 < d.
  have hdiv : 7 * d / 8 ≤ d - 2 :=
    (Nat.div_le_iff_le_mul_add_pred (a := 7 * d) (b := 8) (c := d - 2) (by omega : 0 < 8)).mpr
      (by omega)
  have hsub : (7 * d / 8) + 1 - (7 * d / 8) / 8 ≤ (7 * d / 8) + 1 := Nat.sub_le _ _
  omega

end Scheduler
