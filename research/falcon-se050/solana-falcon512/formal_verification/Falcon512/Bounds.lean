/-
  Falcon512.Bounds — Arithmetic safety lemmas under the lazy-reduction
  invariants.

  Each theorem here takes the lazy-NTT level invariant `r ≤ (K+1)·Q`
  (or its specialisation `b ≤ 8·Q` for the lazy-t butterfly) as a
  *hypothesis* and proves that, under that invariant, the arithmetic
  in `src/ntt.rs` (CT product, CT sum, lazy-t product, fused-step T
  offset) does not overflow its storage type.

  This file does NOT prove that the lazy invariants themselves hold
  level-by-level over the actual NTT loop. That structural fact — i.e.,
  "after K applications of `ct_butterfly_lazy_t_step`, each coefficient
  is in fact bounded by (K+1)·Q" — is checked operationally by the
  proptests in `src/ntt.rs::tests::proptest!` (each kernel matches a
  non-wrapping spec on random inputs in its declared range) and, when
  ignored host tests are run, the PQClean differential. The lemmas here
  are the "if the invariant holds, the arithmetic is safe" half of the
  picture.
-/

import Falcon512.Defs

namespace Falcon512.Spec.Bounds

open Falcon512.Spec

-- ============================================================================
-- Theorem: CT butterfly product fits u64
-- ============================================================================

/-- Given the lazy-CT invariant `r ≤ (K+1)·Q` for `K ≤ 9`, the product
    `r * zeta` (with `zeta < Q`) fits in u64. The level-bound hypothesis
    is taken as given; it is checked operationally by the kernel-vs-
    spec proptests in `src/ntt.rs`. -/
theorem ct_product_fits_u64 (r zeta : Nat) (K : Nat)
    (hr : r ≤ (K + 1) * Q)
    (hz : zeta < Q)
    (hK : K ≤ 9) :
    r * zeta < 2^64 := by
  have hKQ : (K + 1) * Q ≤ 10 * Q := by
    apply Nat.mul_le_mul_right; omega
  have hrb : r ≤ 10 * Q := Nat.le_trans hr hKQ
  have hzb : zeta ≤ Q - 1 := by omega
  calc r * zeta
      ≤ (10 * Q) * (Q - 1) := Nat.mul_le_mul hrb hzb
    _ < 2^64 := by unfold Q; omega

/-- Given the lazy-CT invariant `a ≤ (K+1)·Q` for `K ≤ 9`, the sum `a + t`
    (with `t < Q`) fits in u32. As above, the invariant itself is
    checked operationally. -/
theorem ct_sum_fits_u32 (a t : Nat) (K : Nat)
    (ha : a ≤ (K + 1) * Q)
    (ht : t < Q)
    (hK : K ≤ 9) :
    a + t < 2^32 := by
  have hKQ : (K + 1) * Q ≤ 10 * Q := Nat.mul_le_mul_right Q (by omega)
  have hab : a ≤ 10 * Q := Nat.le_trans ha hKQ
  calc a + t ≤ 10 * Q + (Q - 1) := by omega
    _ < 2^32 := by unfold Q; omega

/-- The subtraction a + Q - t is non-negative when a ≥ 0 and t < Q. -/
theorem ct_diff_nonneg (a t : Nat) (ht : t < Q) :
    a + Q ≥ t := by omega

-- ============================================================================
-- Theorem: Lazy-t butterfly fits u32
-- ============================================================================

/-- In the lazy-t butterfly (last forward level), `t = b * zeta` (unreduced)
    fits in u32 when `b ≤ 8·Q` and `zeta < Q`.
    Numerically: `8·Q · (Q-1) = 8·12289·12288 = 1,208,258,256 < 2³² ≈ 4.29·10⁹`. -/
theorem lazy_t_fits_u32 (b zeta : Nat)
    (hb : b ≤ 8 * Q)
    (hz : zeta < Q) :
    b * zeta < 2^32 := by
  calc b * zeta
      ≤ 8 * Q * (Q - 1) := Nat.mul_le_mul hb (by omega)
    _ < 2^32 := by unfold Q; omega

/-- `T_OFFSET_LAZY_T = 8·Q²` keeps the lazy-t subtraction non-negative for
    any `b·zeta` with `b ≤ 8·Q` and `zeta < Q`. The lazy-NTT invariant
    `a ≤ 8·Q` holds in the calling context but is not load-bearing here —
    `a + T_OFFSET_LAZY_T ≥ T_OFFSET_LAZY_T ≥ b·zeta` regardless of `a`. -/
theorem lazy_t_offset_sufficient (a b zeta : Nat)
    (hb : b ≤ 8 * Q)
    (hz : zeta < Q) :
    a + T_OFFSET_LAZY_T ≥ b * zeta := by
  unfold T_OFFSET_LAZY_T
  calc b * zeta
      ≤ 8 * Q * (Q - 1) := Nat.mul_le_mul hb (by omega)
    _ ≤ 8 * Q * Q := Nat.mul_le_mul_left _ (by omega)
    _ ≤ a + 8 * Q * Q := Nat.le_add_left _ _

-- ============================================================================
-- Theorem: Lazy reduction bounds across all NTT levels
-- ============================================================================

/-- The bound `(K+1)·Q` itself fits in u32 for any `K ≤ 9`. This is the
    arithmetic side of the lazy-NTT level invariant; the *structural*
    claim that coefficients actually stay under that bound across K
    levels is what the proptests check. -/
theorem lazy_bound_fits_u32 (K : Nat) (hK : K ≤ 9) :
    (K + 1) * Q < 2^32 := by
  have : (K + 1) * Q ≤ 10 * Q := Nat.mul_le_mul_right Q (by omega)
  calc (K + 1) * Q ≤ 10 * Q := this
    _ < 2^32 := by unfold Q; omega

-- ============================================================================
-- Theorem: Fused step T_OFFSET is sufficient
-- ============================================================================

/-- `T_OFFSET_FUSED = Q · 2³¹` in the fused step exceeds the worst-case
    `t = prev_high · zeta`, where `prev_high ≤ 8·Q + 8·Q²` (the max output
    of the lazy-t butterfly stored as u32). -/
theorem fused_t_offset_sufficient (prev_high zeta : Nat)
    (hp : prev_high ≤ 8 * Q + 8 * Q * Q)  -- structural bound from lazy-t
    (hz : zeta < Q) :
    T_OFFSET_FUSED ≥ prev_high * zeta := by
  unfold T_OFFSET_FUSED
  have hzb : zeta ≤ Q - 1 := by omega
  calc prev_high * zeta
      ≤ (8 * Q + 8 * Q * Q) * (Q - 1) := Nat.mul_le_mul hp hzb
    _ ≤ Q * 2^31 := by unfold Q; omega

-- ============================================================================
-- Theorem: Inverse NTT lazy offset sufficient
-- ============================================================================

/-- `LAZY_OFFSET_GS = 256·Q` in the GS butterfly prevents underflow.
    After the fused step, low halves are `≤ 2·Q`. After 7 lazy GS levels
    where each level doubles the max, values reach `2⁷ · 2·Q = 256·Q`. -/
theorem inv_ntt_lazy_offset_sufficient :
    LAZY_OFFSET_GS ≥ 2^7 * (2 * Q) := by
  unfold LAZY_OFFSET_GS Q; omega

/-- The high half of the lazy GS butterfly fits u64:
    `(max_input + LAZY_OFFSET_GS) * (Q-1) < 2⁶⁴`. -/
theorem inv_ntt_lazy_gs_fits_u64 :
    (LAZY_OFFSET_GS + LAZY_OFFSET_GS) * (Q - 1) < 2^64 := by
  unfold LAZY_OFFSET_GS Q; omega

-- ============================================================================
-- Theorem: big_q in fused norm is sufficient
-- ============================================================================

/-- `BIG_Q_FUSED_NORM = Q · 2²³` exceeds the maximum unreduced `new_lo (≤ 512·Q)`. -/
theorem big_q_exceeds_new_lo :
    BIG_Q_FUSED_NORM > 512 * Q := by
  unfold BIG_Q_FUSED_NORM Q; omega

/-- `BIG_Q_FUSED_NORM` exceeds the maximum unreduced `new_hi (≤ 512·Q·(Q-1))`. -/
theorem big_q_exceeds_new_hi :
    BIG_Q_FUSED_NORM > 512 * Q * (Q - 1) := by
  unfold BIG_Q_FUSED_NORM Q; omega

-- ============================================================================
-- Theorem: L2 norm accumulation fits u64
-- ============================================================================

/-- Worst-case L2 norm over all 512 elements:
    512 * ((Q/2)^2 + MAX_S2_MAG^2) < 2^64. -/
theorem norm_total_fits_u64 :
    N * ((Q / 2) * (Q / 2) + MAX_S2_MAG * MAX_S2_MAG) < 2^64 := by
  unfold N Q MAX_S2_MAG; omega

/-- The worst-case total norm exceeds L2_BOUND, confirming the bound
    is a meaningful discriminator (not trivially always-pass). -/
theorem norm_bound_is_meaningful :
    N * ((Q / 2) * (Q / 2) + MAX_S2_MAG * MAX_S2_MAG) > L2_BOUND := by
  unfold N Q MAX_S2_MAG L2_BOUND; omega

end Falcon512.Spec.Bounds
