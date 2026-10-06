/-
  Falcon512.Refinement — Per-element algebraic facts that justify each
  of the six Rust verify-pipeline optimizations.

  This file proves, for each optimization, the per-element ZMod-Q
  identity that makes the optimization sound. The optimizations and
  the corresponding lemma:

    1. Lazy CT butterfly (skip `% Q` on the sum half)
       — `lazy_ct_preserves_mod`
    2. Lazy-`t` butterfly (skip `% Q` on `t` at the last forward level)
       — `lazy_t_preserves_mod`
    3. Fused last-fwd + pointwise-mul + first-inv
       — `Falcon512.Spec.Fused.fused_equiv_zmod`
    4. Fused norm with `BIG_Q_FUSED_NORM = Q << 23` offset
       — `big_q_offset_same_mod`
    5. Unreduced hash-to-point output (`c[i]` stored up to `5·Q − 1`)
       — `unreduced_c_same_mod`
    6. Lazy GS offset `LAZY_OFFSET_GS = 256·Q`
       — `lazy_offset_same_mod`

  Whole-array composition (the Rust functions `ntt`, `inv_ntt`,
  `last_level_fused_norm`, `fused_step_kernel`) is checked
  operationally — by the kernel-vs-spec proptests in `src/ntt.rs` and,
  when ignored host tests are run, the PQClean differential — not in
  Lean. That division is intentional per the project-level scope: Lean
  checks the math of each optimization; the Rust loops are checked by
  composition tests on real inputs.
-/

import Falcon512.Defs
import Falcon512.NTT
import Falcon512.Norm
import Mathlib.Data.ZMod.Basic
import Mathlib.Tactic.Ring
import Mathlib.Tactic.Linarith

namespace Falcon512.Refinement

open Falcon512.Spec

-- ============================================================================
-- Optimization 1: Lazy CT butterfly
-- ============================================================================
-- Clean CT: t = (b * z) % Q; lo = (a + t) % Q; hi = (a + Q - t) % Q
-- Lazy CT:  t = (b * z) % Q; lo = a + t;       hi = a + Q - t
-- (skip % Q on lo and hi)
--
-- The next level multiplies by zeta and reduces: (lo * zeta') % Q.
-- Key identity: (x * y) % Q = ((x % Q) * y) % Q.

/-- Skipping % Q on the CT butterfly output doesn't affect the result
    after the next multiplication + reduction. -/
theorem lazy_ct_preserves_mod (a t zeta : Nat) :
    ((a + t) * zeta) % Q = (((a + t) % Q) * zeta) % Q := by
  conv_lhs => rw [Nat.mul_mod]
  conv_rhs => rw [Nat.mul_mod, Nat.mod_mod]

-- ============================================================================
-- Optimization 2: Lazy-t butterfly (last forward level)
-- ============================================================================
-- Clean: t = (b * z) % Q
-- Lazy-t: t = b * z  (no % Q on t itself)
--
-- The next consumer multiplies by h_pk and reduces.
-- Key: (h * (a + b*z)) % Q = (h * ((a + (b*z)%Q) % Q)) % Q

/-- Skipping % Q on t in the last-level butterfly doesn't affect
    the final result after pointwise multiplication + reduction. -/
theorem lazy_t_preserves_mod (h a b z : Nat) :
    (h * (a + b * z)) % Q = (h * (a + (b * z) % Q)) % Q := by
  -- Both (a + b*z) and (a + (b*z)%Q) are ≡ a + b*z (mod Q).
  -- So h * either ≡ h * (a + b*z) (mod Q).
  conv_lhs => rw [Nat.mul_mod]
  conv_rhs => rw [Nat.mul_mod]
  congr 1
  -- Need: (a + b * z) % Q = (a + (b * z) % Q) % Q
  exact (Nat.add_mod a (b * z) Q).symm ▸
    (Nat.add_mod a ((b * z) % Q) Q).symm ▸ by rw [Nat.mod_mod]

-- ============================================================================
-- Optimization 3: Fused last-fwd + mul + first-inv
-- ============================================================================
-- Clean: three separate passes over the array
-- Fused: one pass computing all three in registers
--
-- Algebraic equivalence is proved in `Falcon512.Spec.Fused.fused_equiv_zmod`
-- (this file imports `Falcon512.NTT`, which transitively brings in `Defs`;
-- the Fused proof is one ring-step over ZMod Q):
--   pLow + pHigh                = h0*(a + b*z) + h1*(a - b*z)
--   (pLow - pHigh) * z_inv      = (h0*(a+b*z) - h1*(a-b*z)) * z_inv

-- ============================================================================
-- Optimization 4: Fused last-inv + norm
-- ============================================================================
-- Clean: complete inverse NTT, then loop over coefficients to compute norm
-- Fused: last GS butterfly in registers, norm accumulated inline
--
-- The last GS butterfly computes:
--   out[j]     = buf[j] + buf[j+N/2]              (unreduced)
--   out[j+N/2] = (buf[j] + offset - buf[j+N/2]) * zeta  (unreduced)
--
-- The norm then computes: raw = (c[j] + big_q - out[j]) % Q
-- Since big_q is a multiple of Q, this equals (c[j] - out[j]) % Q,
-- and since out[j] ≡ out_reduced[j] (mod Q), the norm is the same.

/-- The `c + BIG_Q_FUSED_NORM - out` subtraction gives the same mod-Q
    result as `c + Q - (out % Q)`, because `BIG_Q_FUSED_NORM ≡ 0 (mod Q)`. -/
theorem big_q_offset_same_mod (c out : Nat)
    (hc : c < 5 * Q) (hout : out < 512 * Q) :
    (c + BIG_Q_FUSED_NORM - out) % Q = (c + Q - out % Q) % Q := by
  have hQ_pos : Q > 0 := by unfold Q; omega
  unfold BIG_Q_FUSED_NORM
  rw [mul_comm Q (2^23)]
  apply Falcon512.Spec.Norm.sub_with_multiple_of_q c out (2^23)
  · unfold Q at *; omega
  · have := Nat.mod_lt out hQ_pos; omega

-- ============================================================================
-- Optimization 5: Unreduced hash-to-point output
-- ============================================================================
-- Clean: c[i] = w % Q, where w < 5*Q
-- Rust:  c[i] = w (unreduced, w < 5*Q, fits u16)
--
-- Downstream: (c[i] + big_q - buf[i]) % Q.
-- Key: w % Q ≡ w (mod Q), so (w + big_q - x) % Q = (w%Q + big_q - x) % Q.

/-- Storing the unreduced w instead of w % Q doesn't affect the norm
    computation, because % Q at the end absorbs the un-reduction. The
    `w < 5*Q` rejection bound from hash-to-point is the production
    setting in which this is invoked, but the algebraic identity itself
    holds for any `w` — `hw` is not needed in the proof and is omitted. -/
theorem unreduced_c_same_mod (w x big_q : Nat)
    (hbq : Q ∣ big_q) (hge : w + big_q ≥ x)
    (hbq_pos : big_q ≥ Q) :
    (w + big_q - x) % Q = (w % Q + big_q - x % Q) % Q := by
  have hQ_pos : Q > 0 := by unfold Q; omega
  obtain ⟨k, rfl⟩ := hbq
  -- After destructuring: goal uses Q * k (not k * Q)
  -- hge : w + Q * k ≥ x, hbq_pos : Q * k ≥ Q
  have hk : k ≥ 1 := by
    by_contra h; push_neg at h; interval_cases k; omega
  have hxmod := Nat.mod_lt x hQ_pos
  have hwmod := Nat.mod_lt w hQ_pos
  have hge2 : w % Q + Q * k ≥ x % Q := by
    have : Q * k ≥ Q := Nat.le_mul_of_pos_right Q (by omega)
    omega
  suffices h : ((w + Q * k - x : Nat) : Int) % ↑Q =
               ((w % Q + Q * k - x % Q : Nat) : Int) % ↑Q by exact_mod_cast h
  rw [Nat.cast_sub hge, Nat.cast_sub hge2]
  simp only [Nat.cast_add, Nat.cast_mul]
  have hmod_w : (↑(w % Q) : Int) = ↑w % ↑Q := by omega
  have hmod_x : (↑(x % Q) : Int) = ↑x % ↑Q := by omega
  rw [hmod_w, hmod_x]
  rw [show (↑w : Int) + ↑Q * ↑k - ↑x = (↑w - ↑x) + ↑k * ↑Q from by ring,
      Int.add_mul_emod_self,
      show (↑w : Int) % ↑Q + ↑Q * ↑k - ↑x % ↑Q = (↑w % ↑Q - ↑x % ↑Q) + ↑k * ↑Q from by ring,
      Int.add_mul_emod_self]
  -- Now need: (↑w - ↑x) % ↑Q = (↑w % ↑Q - ↑x % ↑Q) % ↑Q
  rw [show (↑w : Int) - ↑x = (↑w % ↑Q - ↑x % ↑Q) + (↑w - ↑w % ↑Q - (↑x - ↑x % ↑Q)) from by ring]
  have hw_div : ↑w - ↑w % (↑Q : Int) = (↑w / ↑Q) * ↑Q := by
    have := Int.emod_add_ediv (↑w : Int) (↑Q : Int); linarith
  have hx_div : ↑x - ↑x % (↑Q : Int) = (↑x / ↑Q) * ↑Q := by
    have := Int.emod_add_ediv (↑x : Int) (↑Q : Int); linarith
  rw [show (↑w : Int) % ↑Q - ↑x % ↑Q + ((↑w : Int) - ↑w % ↑Q - (↑x - ↑x % ↑Q)) =
    (↑w % ↑Q - ↑x % ↑Q) + ((↑w - ↑w % ↑Q) - (↑x - ↑x % ↑Q)) from by ring,
    hw_div, hx_div,
    show (↑w : Int) % ↑Q - ↑x % ↑Q + (↑w / ↑Q * ↑Q - ↑x / ↑Q * ↑Q) =
      (↑w % ↑Q - ↑x % ↑Q) + (↑w / ↑Q - ↑x / ↑Q) * ↑Q from by ring,
    Int.add_mul_emod_self]

-- ============================================================================
-- Optimization 6: lazy_offset = 256*Q in inverse GS
-- ============================================================================
-- Clean GS: hi = (u + Q - v) * zeta % Q
-- Lazy GS:  hi = (u + 256*Q - v) * zeta % Q
--
-- Since 256*Q ≡ Q ≡ 0 (mod Q), both are (u - v) * zeta mod Q.

/-- `LAZY_OFFSET_GS = 256·Q` and `Q` are both `≡ 0 (mod Q)`, so the GS
    butterfly subtraction gives the same result with either offset. -/
theorem lazy_offset_same_mod (u v zeta : Nat)
    (hu : u < LAZY_OFFSET_GS) (hv : v < LAZY_OFFSET_GS) :
    ((u + LAZY_OFFSET_GS - v) * zeta) % Q = ((u + Q - v % Q) * zeta) % Q := by
  have hQ_pos : Q > 0 := by unfold Q; omega
  unfold LAZY_OFFSET_GS at *
  -- Factor out the * zeta: (a * zeta) % Q = (b * zeta) % Q if a % Q = b % Q
  have key : (u + 256 * Q - v) % Q = (u + Q - v % Q) % Q := by
    apply Falcon512.Spec.Norm.sub_with_multiple_of_q u v 256
    · unfold Q at *; omega
    · have := Nat.mod_lt v hQ_pos; omega
  conv_lhs => rw [Nat.mul_mod, key, ← Nat.mul_mod]

-- ============================================================================
-- Composing the per-optimization lemmas
-- ============================================================================
-- Each Rust optimization above is proven mod-Q-preserving at the butterfly
-- or single-coefficient level. The intended pipeline argument is that a
-- sequential composition of these steps over the polynomial array preserves
-- the clean mod-Q values, and the final L2-norm decision depends only on
-- those values. We do not state or prove that array-level composition
-- theorem here; it would require lifting the per-element lemmas through the
-- explicit N-element loops in `ntt.rs`/`codec.rs`. Rust-side proptests,
-- soak tests, and differential tests provide operational evidence for that
-- loop-level composition.

end Falcon512.Refinement
