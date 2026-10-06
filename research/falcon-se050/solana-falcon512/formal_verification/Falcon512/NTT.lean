/-
  Falcon512.NTT — Number Theoretic Transform correctness.

  Proves that the CT and GS butterflies are mutual inverses,
  that the NTT round-trip identity holds at the butterfly level,
  and key algebraic constants (N_INV, PSI).

  Uses Mathlib's ZMod for modular arithmetic ring reasoning.
-/

import Falcon512.Defs
import Mathlib.Data.ZMod.Basic
import Mathlib.RingTheory.RootsOfUnity.Basic

namespace Falcon512.Spec.NTT

open Falcon512.Spec

-- ============================================================================
-- N_INV correctness
-- ============================================================================

def N_INV : Nat := 12265

theorem n_inv_correct : (N_INV * N) % Q = 1 := by native_decide

-- ============================================================================
-- PSI (primitive 2N-th root of unity)
-- ============================================================================

def PSI : Nat := 49

theorem psi_is_root : powQ PSI N = Q - 1 := by native_decide

theorem psi_order_divides : powQ PSI (2 * N) = 1 := by native_decide

-- ============================================================================
-- CT/GS butterfly inverse relationship
-- ============================================================================

/-- A CT butterfly followed by a GS butterfly with the inverse twiddle
    factor recovers the original pair scaled by 2.
    This is proven in ZMod Q where we have ring structure. -/
theorem ct_gs_inverse_zmod (a b z : ZMod Q)
    (hz : z * z⁻¹ = 1) :
    let lo := a + b * z
    let hi := a - b * z
    lo + hi = 2 * a ∧ (lo - hi) * z⁻¹ = 2 * b := by
  constructor
  · ring
  · -- (lo - hi) * z⁻¹ = 2 * b * z * z⁻¹ = 2 * b
    have : (a + b * z - (a - b * z)) * z⁻¹ = 2 * b * (z * z⁻¹) := by ring
    rw [this, hz, mul_one]

-- ============================================================================
-- Full NTT round-trip scaling factor
-- ============================================================================
--
-- `ct_gs_inverse_zmod` above establishes the per-level round-trip (forward CT
-- followed by inverse GS recovers `(2a, 2b)`). The full NTT applies log₂(N)=9
-- levels, accumulating a factor of `2^9 = 512 = N`, cancelled by `N_INV`
-- (pre-folded into the prepared pubkey). The cancellation `N_INV * N ≡ 1
-- (mod Q)` is proved as `n_inv_correct` above.

-- ============================================================================
-- PSI is a primitive 2N-th root of unity in Z_q
-- ============================================================================

/-- PSI has order exactly 2N in Z_q*: `psi^N = -1 (mod Q)` and
    `psi^(2N) = 1 (mod Q)`. These are the two scalar facts that make
    PSI a primitive 2N-th root of unity, the precondition for the
    NTT-as-ring-isomorphism construction (`Z_q[x]/(x^N+1) ↔ Z_q^N` via
    CRT).

    This lemma proves *only* the order facts; the full ring-isomorphism
    claim (that NTT-pointwise-mul corresponds to polynomial mul mod
    x^N+1) is exhaustively checked by the
    `ntt_multiplication_matches_schoolbook` test in `src/ntt.rs`. -/
theorem psi_has_order_2N :
    powQ PSI N = Q - 1 ∧ powQ PSI (2 * N) = 1 :=
  ⟨psi_is_root, psi_order_divides⟩

end Falcon512.Spec.NTT
