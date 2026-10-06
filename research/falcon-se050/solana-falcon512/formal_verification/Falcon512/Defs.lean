/-
  Falcon512.Defs — Core definitions for Falcon-512 formal verification.

  Defines the modular arithmetic ring Z_q, polynomial type, NTT basis,
  and the butterfly operations that compose the forward and inverse NTT.
-/

namespace Falcon512.Spec

-- ============================================================================
-- Constants
-- ============================================================================

/-- Falcon-512 modulus. Prime, = 12289 = 12 * 1024 + 1. -/
def Q : Nat := 12289

/-- Polynomial degree. -/
def N : Nat := 512

/-- L2-norm rejection threshold (Falcon spec Table 3.3). -/
def L2_BOUND : Nat := 34034726

/-- Maximum Golomb-Rice magnitude for s2 coefficients. -/
def MAX_S2_MAG : Nat := 2047

-- ----------------------------------------------------------------------------
-- Lazy-NTT optimisation offsets (mirror `src/ntt.rs` constants)
--
-- Each `def` here is the literal expression used in `src/ntt.rs` for the
-- corresponding `pub(crate) const`. The Lean refinement lemmas refer to
-- these definitions so that any value drift between the Rust and Lean
-- sides surfaces by name (rather than as a stale inline literal).
-- The Rust side const-asserts the modular and bound properties at
-- compile time; we reproduce the values verbatim here.
-- ----------------------------------------------------------------------------

/-- Lazy-`t` CT butterfly offset; `src/ntt.rs::T_OFFSET_LAZY_T = 8·Q²`. -/
def T_OFFSET_LAZY_T : Nat := 8 * Q * Q

/-- Lazy GS butterfly offset; `src/ntt.rs::LAZY_OFFSET_GS = 256·Q`. -/
def LAZY_OFFSET_GS : Nat := 256 * Q

/-- Fused last-fwd / pointwise / first-inv `T` offset;
    `src/ntt.rs::T_OFFSET_FUSED = Q·2³¹`. -/
def T_OFFSET_FUSED : Nat := Q * 2^31

/-- Fused-norm subtraction offset; `src/ntt.rs::BIG_Q_FUSED_NORM = Q·2²³`. -/
def BIG_Q_FUSED_NORM : Nat := Q * 2^23

-- ============================================================================
-- Modular arithmetic
-- ============================================================================

/-- Reduce a natural number modulo Q. -/
@[inline] def modQ (a : Nat) : Nat := a % Q

/-- Modular addition in Z_q. -/
@[inline] def addQ (a b : Nat) : Nat := (a + b) % Q

/-- Modular subtraction in Z_q. Requires a, b < Q. -/
@[inline] def subQ (a b : Nat) : Nat := (a + Q - b) % Q

/-- Modular multiplication in Z_q. -/
@[inline] def mulQ (a b : Nat) : Nat := (a * b) % Q

/-- Modular exponentiation by repeated squaring. -/
def powQ (base exp : Nat) : Nat :=
  if exp = 0 then 1
  else if exp % 2 = 0 then
    let half := powQ base (exp / 2)
    mulQ half half
  else
    mulQ (base % Q) (powQ base (exp - 1))
termination_by exp
decreasing_by all_goals omega

-- ============================================================================
-- Basic modular arithmetic properties
-- ============================================================================

theorem q_pos : Q > 0 := by unfold Q; omega

theorem modQ_lt (a : Nat) : modQ a < Q := Nat.mod_lt a q_pos

theorem addQ_lt (a b : Nat) : addQ a b < Q := Nat.mod_lt _ q_pos

theorem subQ_lt (a b : Nat) : subQ a b < Q := Nat.mod_lt _ q_pos

theorem mulQ_lt (a b : Nat) : mulQ a b < Q := Nat.mod_lt _ q_pos

-- ============================================================================
-- CT (Cooley-Tukey) butterfly
-- ============================================================================

/-- Forward NTT butterfly: given coefficients a, b and twiddle factor zeta,
    produces (a + b*zeta mod q, a - b*zeta mod q). -/
structure CTResult where
  lo : Nat
  hi : Nat

/-- CT butterfly with full reduction on t = b * zeta mod q. -/
def ctButterfly (a b zeta : Nat) : CTResult :=
  let t := mulQ b zeta
  { lo := addQ a t
    hi := subQ a t }

/-- CT butterfly with lazy-t (no reduction on t). -/
def ctButterflyLazyT (a b zeta : Nat) : CTResult :=
  let t := b * zeta  -- unreduced!
  { lo := a + t      -- unreduced sum
    hi := a + T_OFFSET_LAZY_T - t }  -- offset keeps non-negative

-- ============================================================================
-- GS (Gentleman-Sande) butterfly
-- ============================================================================

/-- Inverse NTT butterfly: given coefficients a, b and twiddle factor zeta,
    produces ((a + b) mod q, (a - b) * zeta mod q). -/
def gsButterfly (a b zeta : Nat) : CTResult :=
  { lo := addQ a b
    hi := mulQ (subQ a b) zeta }

/-- Lazy GS butterfly: low half unreduced. -/
def gsButterflyLazy (a b zeta : Nat) : CTResult :=
  { lo := a + b          -- unreduced
    hi := ((a + LAZY_OFFSET_GS - b) * zeta) % Q }

-- ============================================================================
-- Fused step
-- ============================================================================

/-- Fused last-forward + pointwise-mul + first-inverse step.
    For a single pair (i) with forward zeta z, inverse zeta z_inv,
    and prepared pubkey coefficients h0, h1. -/
structure FusedResult where
  lo : Nat
  hi : Nat

def fusedStep (a b z z_inv h0 h1 : Nat) : FusedResult :=
  -- Forward CT butterfly (last level)
  let t := b * z
  let sLow := a + t
  let sHigh := a + T_OFFSET_FUSED - t
  -- Pointwise multiply with prepared pubkey
  let pLow := (h0 * sLow) % Q
  let pHigh := (h1 * sHigh) % Q
  -- Inverse GS butterfly (first level)
  { lo := pLow + pHigh            -- unreduced (up to 2*Q)
    hi := ((pLow + Q - pHigh) * z_inv) % Q }

end Falcon512.Spec
