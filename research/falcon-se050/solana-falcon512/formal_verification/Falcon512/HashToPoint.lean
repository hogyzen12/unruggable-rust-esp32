/-
  Falcon512.HashToPoint — Uniformity of the rejection-sampled output.

  Falcon's HashToPoint reads SHAKE-256 output as a stream of 16-bit
  candidates and accepts iff `w < 5*Q`. The accepted residue is then
  used as a coefficient of the challenge polynomial `c`.

  This file proves the rejection bound `5*Q` is the right one: every
  residue r ∈ [0, Q) has exactly 5 preimages in the accept set, so the
  conditional distribution of `w mod Q` given acceptance is uniform —
  zero bias.

  Why this matters: a wrong threshold (e.g. accepting w < 6*Q, which
  exceeds 2^16) would either be incorrect (overflow) or, more subtly,
  give a non-uniform `c`, weakening the security argument. Accepting
  too small a threshold (e.g. w < 4*Q) would still be uniform but
  reject more candidates — wasted SHAKE work, not a soundness issue.
-/

import Falcon512.Defs
import Mathlib.Data.Finset.Image
import Mathlib.Data.Finset.Card
import Mathlib.Tactic.Linarith

namespace Falcon512.Spec.HashToPoint

open Falcon512.Spec

/-- The accept set: 16-bit candidates `w` with `w < 5*Q`. -/
def acceptSet : Finset Nat := Finset.range (5 * Q)

theorem acceptSet_card : acceptSet.card = 5 * Q := by
  unfold acceptSet
  exact Finset.card_range _

/-- The accept set fits in the 16-bit candidate range. -/
theorem acceptSet_fits_u16 : 5 * Q ≤ 2^16 := by
  unfold Q; omega

/-- 6Q would NOT fit — confirms 5Q is the *largest* multiple of Q
    below 2^16, hence the correct rejection threshold. -/
theorem six_q_exceeds_u16 : 6 * Q > 2^16 := by
  unfold Q; omega

/-- The set of accepted candidates `w ∈ [0, 5Q)` whose residue mod Q
    equals `r`. -/
def residueFiber (r : Nat) : Finset Nat :=
  acceptSet.filter (fun w => w % Q = r)

/-- For every residue `r < Q`, the fiber inside the accept set is
    exactly `{r, r+Q, r+2Q, r+3Q, r+4Q}` — i.e. the five values
    obtained by adding multiples of Q to `r`, all bounded by 5Q. -/
theorem residueFiber_eq_image (r : Nat) (hr : r < Q) :
    residueFiber r = (Finset.range 5).image (fun k => r + k * Q) := by
  ext w
  simp only [residueFiber, acceptSet, Finset.mem_filter, Finset.mem_range,
             Finset.mem_image]
  refine ⟨?_, ?_⟩
  · rintro ⟨hwlt, hwmod⟩
    -- w < 5Q ∧ w % Q = r ⇒ ∃ k < 5, w = r + k*Q.
    refine ⟨w / Q, ?_, ?_⟩
    · have hQ_pos : Q > 0 := q_pos
      exact (Nat.div_lt_iff_lt_mul hQ_pos).mpr hwlt
    · -- Use the div+mod identity in the form we need.
      have hdm : Q * (w / Q) + w % Q = w := Nat.div_add_mod w Q
      -- Rewrite Q * (w/Q) as (w/Q) * Q to match goal shape.
      have hcomm : Q * (w / Q) = (w / Q) * Q := Nat.mul_comm _ _
      omega
  · rintro ⟨k, hk5, rfl⟩
    refine ⟨?_, ?_⟩
    · -- r + k*Q < 5*Q from r < Q and k ≤ 4.
      have hk_le : k ≤ 4 := by omega
      have hkQ : k * Q ≤ 4 * Q := Nat.mul_le_mul_right Q hk_le
      linarith
    · -- (r + k*Q) % Q = r since r < Q. Step 1: kill the k*Q via add_mul_mod_self_right.
      have h1 : (r + k * Q) % Q = r % Q := Nat.add_mul_mod_self_right r k Q
      rw [h1]
      exact Nat.mod_eq_of_lt hr

/-- Every residue's fiber has cardinality exactly 5. -/
theorem residueFiber_card (r : Nat) (hr : r < Q) :
    (residueFiber r).card = 5 := by
  rw [residueFiber_eq_image r hr]
  rw [Finset.card_image_of_injective _ ?_, Finset.card_range]
  -- Injectivity of `k ↦ r + k * Q` on ℕ.
  intro a b hab
  simp only at hab
  -- hab : r + a * Q = r + b * Q
  have h1 : a * Q = b * Q := by linarith
  have hQ_pos : Q > 0 := q_pos
  exact Nat.eq_of_mul_eq_mul_right hQ_pos h1

/-- **Uniformity theorem.** Given a uniform random `w ∈ [0, 5Q)`,
    the residue `w mod Q` is uniformly distributed over `[0, Q)`.

    Stated as a counting claim: every residue has the same number of
    preimages (= 5). The probability statement
    `Pr[w mod Q = r | w < 5Q] = 1/Q` follows by dividing both sides
    of `residueFiber_card` by `acceptSet_card = 5Q`. -/
theorem hash_to_point_uniform (r : Nat) (hr : r < Q) :
    (residueFiber r).card * Q = acceptSet.card := by
  rw [residueFiber_card r hr, acceptSet_card]

end Falcon512.Spec.HashToPoint
