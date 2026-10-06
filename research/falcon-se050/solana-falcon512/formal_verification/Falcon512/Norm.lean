/-
  Falcon512.Norm — Per-element algebraic primitives for the Rust fused-norm
  optimization.

  Scope: this file proves the per-coefficient ZMod-Q / Nat identities that
  justify the Rust `last_level_fused_norm` (fused last-inverse-NTT level
  combined with L2 norm accumulation, using `BIG_Q_FUSED_NORM = Q << 23`
  and `LAZY_OFFSET_GS = 256·Q` instead of `Q`).

  The whole-pipeline equality (`last_level_fused_norm` Rust function ≡
  unfused composition) is **not** proved here. That would require a Lean
  model of the N-element loop. It is operationally checked by the
  `fused_norm_step_matches_spec` proptest in `src/ntt.rs` and, when
  ignored host tests are run, the PQClean differential.
-/

import Falcon512.Defs
import Falcon512.NTT
import Mathlib.Data.ZMod.Basic
import Mathlib.Tactic.Zify
import Mathlib.Tactic.Ring
import Mathlib.Tactic.Linarith

namespace Falcon512.Spec.Norm

open Falcon512.Spec

-- ============================================================================
-- Centered reduction
-- ============================================================================

def centeredAbs (raw : Nat) : Nat :=
  if raw > Q / 2 then Q - raw else raw

theorem centeredAbs_le_half_q (raw : Nat) (h : raw < Q) :
    centeredAbs raw ≤ Q / 2 := by
  unfold centeredAbs
  split <;> omega

-- ============================================================================
-- Per-element norm contribution
-- ============================================================================

def normContrib (s1c_abs s2_abs : Nat) : Nat :=
  s1c_abs * s1c_abs + s2_abs * s2_abs

def maxNormContrib : Nat :=
  (Q / 2) * (Q / 2) + MAX_S2_MAG * MAX_S2_MAG

theorem normContrib_bounded (s1c s2 : Nat)
    (h1 : s1c ≤ Q / 2) (h2 : s2 ≤ MAX_S2_MAG) :
    normContrib s1c s2 ≤ maxNormContrib := by
  unfold normContrib maxNormContrib
  exact Nat.add_le_add (Nat.mul_le_mul h1 h1) (Nat.mul_le_mul h2 h2)

-- ============================================================================
-- Key identity: big_q offset produces same residue as Q offset
-- ============================================================================

/-- For any `c, x` and `k ≥ 1` with `c + k·Q ≥ x`:
    `(c + k·Q - x) % Q = (c + Q - x % Q) % Q`. Both sides `≡ c - x (mod Q)`.

    This is the Nat-level identity that makes `BIG_Q_FUSED_NORM = Q·2²³`
    and `LAZY_OFFSET_GS = 256·Q` interchangeable with `Q` for the fused-norm
    subtraction. The proof routes through `Int` and `Int.add_mul_emod_self`
    because Nat subtraction coercions defeat `omega`/`zify` directly. -/
theorem sub_with_multiple_of_q (c x k : Nat)
    (hge : c + k * Q ≥ x)
    (hcr : c + Q ≥ x % Q) :
    (c + k * Q - x) % Q = (c + Q - x % Q) % Q := by
  suffices h : ((c + k * Q - x : Nat) : Int) % (Q : Nat) =
               ((c + Q - x % Q : Nat) : Int) % (Q : Nat) by exact_mod_cast h
  rw [Nat.cast_sub hge, Nat.cast_sub hcr]
  simp only [Nat.cast_add, Nat.cast_mul]
  have hmod_cast : (↑(x % Q) : Int) = ↑x % ↑Q := by omega
  rw [hmod_cast]
  rw [show (↑c : Int) + ↑k * ↑Q - ↑x = (↑c - ↑x) + ↑k * ↑Q from by ring,
      Int.add_mul_emod_self,
      show (↑c : Int) + ↑Q - ↑x % ↑Q = (↑c - ↑x % ↑Q) + 1 * ↑Q from by ring,
      Int.add_mul_emod_self,
      show (↑c : Int) - ↑x = (↑c - ↑x % ↑Q) + (↑x % ↑Q - ↑x) from by ring]
  have : ↑x % (↑Q : Int) - ↑x = -(↑x / ↑Q) * ↑Q := by
    have := Int.emod_add_ediv (↑x : Int) (↑Q : Int)
    linarith
  rw [this, show (↑c : Int) - ↑x % ↑Q + -(↑x / ↑Q) * ↑Q =
    (↑c - ↑x % ↑Q) + -(↑x / ↑Q) * ↑Q from by ring,
    Int.add_mul_emod_self]

-- ============================================================================
-- Norm computation equivalence in ZMod (algebraic core)
-- ============================================================================

/-- `BIG_Q_FUSED_NORM = Q · 2²³` is algebraically equivalent to `Q` for
    the norm subtraction: both are multiples of `Q` and thus `≡ 0` in
    `ZMod Q`. -/
theorem big_q_equiv_q_zmod : (BIG_Q_FUSED_NORM : ZMod Q) = 0 := by
  unfold BIG_Q_FUSED_NORM
  native_decide

/-- `LAZY_OFFSET_GS = 256·Q` is `≡ 0` in `ZMod Q`. -/
theorem lazy_offset_equiv_zero_zmod : (LAZY_OFFSET_GS : ZMod Q) = 0 := by
  unfold LAZY_OFFSET_GS
  native_decide

-- ============================================================================
-- Norm bound correctness
-- ============================================================================

theorem l2_bound_positive : L2_BOUND > 0 := by unfold L2_BOUND; omega

-- ============================================================================
-- Algebraic primitives the Rust fused-norm optimization relies on
-- ============================================================================

/-- Both Rust offset constants — `BIG_Q_FUSED_NORM = Q · 2²³` (the
    fused-norm subtraction offset) and `LAZY_OFFSET_GS = 256·Q` (the
    lazy GS butterfly offset) — are multiples of `Q` and thus vanish in
    `ZMod Q`. Combined with `sub_with_multiple_of_q` above, this is the
    per-element evidence that the Rust fused norm computes the same
    `centeredAbs² + s2²` sum as the clean (unfused) norm.

    The whole-pipeline equality (`last_level_fused_norm` Rust function ≡
    unfused composition) is **not** proved here — it would require lifting
    these per-element identities through the explicit N-element loop in
    `src/ntt.rs`. That whole-pipeline equality is checked operationally
    by the proptests (`fused_norm_step_matches_spec` in `src/ntt.rs`) and,
    when ignored host tests are run, the differential vs PQClean. -/
theorem fused_norm_offsets_vanish_mod_q :
    Q ∣ BIG_Q_FUSED_NORM ∧ Q ∣ LAZY_OFFSET_GS ∧
    (BIG_Q_FUSED_NORM : ZMod Q) = 0 ∧ (LAZY_OFFSET_GS : ZMod Q) = 0 := by
  refine ⟨?_, ?_, big_q_equiv_q_zmod, lazy_offset_equiv_zero_zmod⟩
  · unfold BIG_Q_FUSED_NORM; exact dvd_mul_right Q _
  · unfold LAZY_OFFSET_GS; exact dvd_mul_left Q 256

end Falcon512.Spec.Norm
