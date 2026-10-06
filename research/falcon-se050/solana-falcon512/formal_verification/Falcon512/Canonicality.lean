/-
  Falcon512.Canonicality — Golomb-Rice signature encoding canonicality.

  This file proves that the abstract Falcon-512 signature byte payload is
  uniquely determined by its coefficient sequence. The headline theorem is
  `serializeFalcon_injective`; everything else is the supporting chain.

  ┌─────────────────────────────────────────────────────────────────────┐
  │ MAIN RESULTS                                                        │
  ├─────────────────────────────────────────────────────────────────────┤
  │ encodeCoeff_injective       — per-coefficient bit-encoding unique   │
  │ encodeCoeff_prefix_free     — encodings are prefix-free             │
  │ encodeAll_injective         — sequence-level injectivity            │
  │ encodeFalcon_injective      — N=512 specialization                  │
  │ append_replicate_false_inj  — zero-pad cancellation                 │
  │ wireBits_injective          — bit-level wire-format injectivity     │
  │ packBytes_injective         — MSB-first byte packing is injective   │
  │ serializeFalcon_injective   — abstract byte-level canonicality      │
  └─────────────────────────────────────────────────────────────────────┘

  The abstract model mirrors the Rust decoder's two padding-rejection rules
  in `decompress_signature` (residual-bits-zero and trailing-zero-bytes) by
  enforcing the canonical zero-pad characterized by
  `append_replicate_false_inj`. The Rust property tests under
  `src/codec.rs::adversarial` remain as implementation cross-checks; this
  file is the machine-checked formalization of the model.
-/

import Mathlib.Data.List.Basic
import Mathlib.Data.Nat.Defs
import Mathlib.Tactic.Linarith
import Mathlib.Tactic.Ring
import Falcon512.Defs

namespace Falcon512.Spec.Canonicality

/-! ## §1. Coefficient type

A signed magnitude with the `(sign = true, mag = 0)` form excluded —
the only such form would be malleable since `+0 = -0`. -/

/-- A valid Falcon-512 signature coefficient: sign bit + magnitude in
    `[0, 2048)`, excluding the malleable `(sign=true, mag=0)` form. -/
structure Coeff where
  sign : Bool
  mag : Nat
  hmag : mag < 2048
  hno_neg_zero : ¬(sign = true ∧ mag = 0)

theorem Coeff.ext_iff (c1 c2 : Coeff) :
    c1 = c2 ↔ c1.sign = c2.sign ∧ c1.mag = c2.mag := by
  constructor
  · intro h; rw [h]; exact ⟨rfl, rfl⟩
  · intro ⟨hs, hm⟩
    cases c1; cases c2
    simp_all

/-! ## §2. Per-coefficient bit encoding

The Golomb-Rice encoding is

  `sign :: 7-LSBs(mag % 128) ++ unary(mag / 128) ++ [true]`

with length `9 + mag/128`. The two main results in this section are
`encodeCoeff_injective` (each `Coeff` produces a unique bit-list) and
`encodeCoeff_prefix_free` (no encoding is a strict prefix of another). -/

-- The internal `lsb7Bits` / `lsb7Decode` round-trip is verified by
-- `native_decide` over `Fin 128`, then lifted to a bound-checked input.

/-- Bit-list of the 7 LSBs of `mag`, MSB first. -/
private def lsb7Bits (mag : Nat) : List Bool :=
  [(mag / 64) % 2 = 1,
   (mag / 32) % 2 = 1,
   (mag / 16) % 2 = 1,
   (mag / 8) % 2 = 1,
   (mag / 4) % 2 = 1,
   (mag / 2) % 2 = 1,
   mag % 2 = 1]

private theorem lsb7Bits_length (mag : Nat) : (lsb7Bits mag).length = 7 := by
  unfold lsb7Bits; rfl

private def lsb7Decode : List Bool → Nat
  | [b6, b5, b4, b3, b2, b1, b0] =>
      (if b6 then 64 else 0) + (if b5 then 32 else 0) +
      (if b4 then 16 else 0) + (if b3 then 8  else 0) +
      (if b2 then 4  else 0) + (if b1 then 2  else 0) +
      (if b0 then 1  else 0)
  | _ => 0

private theorem lsb7Bits_roundtrip (mag : Nat) (h : mag < 128) :
    lsb7Decode (lsb7Bits mag) = mag := by
  have key : ∀ m : Fin 128, lsb7Decode (lsb7Bits m.val) = m.val := by native_decide
  exact key ⟨mag, h⟩

private theorem lsb7Bits_injective {a b : Nat} (ha : a < 128) (hb : b < 128)
    (h : lsb7Bits a = lsb7Bits b) : a = b := by
  rw [← lsb7Bits_roundtrip a ha, h, lsb7Bits_roundtrip b hb]

/-- Encoding of a single coefficient as a bit-list (MSB first):
      [sign] ++ lsb7Bits(mag % 128) ++ replicate(mag/128, false) ++ [true]
    Length = 9 + (mag/128). -/
def encodeCoeff (c : Coeff) : List Bool :=
  c.sign :: lsb7Bits (c.mag % 128) ++
  List.replicate (c.mag / 128) false ++
  [true]

theorem encodeCoeff_length (c : Coeff) :
    (encodeCoeff c).length = 9 + c.mag / 128 := by
  unfold encodeCoeff
  simp [List.length_cons, List.length_append, lsb7Bits_length,
        List.length_replicate]
  omega

theorem encodeCoeff_ne_nil (c : Coeff) : encodeCoeff c ≠ [] := by
  intro h
  have : (encodeCoeff c).length = 0 := by rw [h]; rfl
  rw [encodeCoeff_length] at this
  omega

/-- The encoding starts with the sign bit. -/
private theorem encodeCoeff_head (c : Coeff) :
    ∃ rest, encodeCoeff c = c.sign :: rest :=
  ⟨_, rfl⟩

/-- After the sign, the next 7 bits are `lsb7Bits (mag % 128)`, then the
    unary tail. -/
private theorem encodeCoeff_tail_eq (c : Coeff) :
    (encodeCoeff c).tail =
      lsb7Bits (c.mag % 128) ++ List.replicate (c.mag / 128) false ++ [true] :=
  rfl

/-- Dropping the 8-bit fixed prefix (sign + 7 LSBs) exposes the unary tail
    `replicate (mag/128) false ++ [true]`. -/
private theorem encodeCoeff_drop8 (c : Coeff) :
    (encodeCoeff c).drop 8 =
      List.replicate (c.mag / 128) false ++ [true] := by
  show (c.sign :: (lsb7Bits (c.mag % 128) ++
        List.replicate (c.mag / 128) false ++ [true])).drop (1 + 7) = _
  rw [List.drop_succ_cons, List.append_assoc,
      List.drop_left' (lsb7Bits_length _)]

/-- **Encoding injectivity.** Two valid coefficients with the same
    bit-encoding are equal. The proof recovers the four pieces of `c`
    from the encoding: lengths give `mag / 128`, the head gives `sign`,
    and the next 7 bits give `mag % 128` via `lsb7Bits_injective`. -/
theorem encodeCoeff_injective (c1 c2 : Coeff)
    (h : encodeCoeff c1 = encodeCoeff c2) : c1 = c2 := by
  -- Length recovers `mag / 128`.
  have hlen : (encodeCoeff c1).length = (encodeCoeff c2).length := by rw [h]
  rw [encodeCoeff_length, encodeCoeff_length] at hlen
  have hdiv : c1.mag / 128 = c2.mag / 128 := by omega
  -- Head recovers `sign`.
  obtain ⟨r1, hr1⟩ := encodeCoeff_head c1
  obtain ⟨r2, hr2⟩ := encodeCoeff_head c2
  rw [hr1, hr2] at h
  have hsign : c1.sign = c2.sign := List.head_eq_of_cons_eq h
  -- Bits 1..7 (the `lsb7Bits` window) recover `mag % 128`.
  have htail : (encodeCoeff c1).tail = (encodeCoeff c2).tail := by
    rw [hr1, hr2]; exact List.tail_eq_of_cons_eq h
  rw [encodeCoeff_tail_eq, encodeCoeff_tail_eq] at htail
  have hlsb_match : lsb7Bits (c1.mag % 128) = lsb7Bits (c2.mag % 128) := by
    have h7 := congrArg (List.take 7) htail
    rw [List.append_assoc, List.append_assoc,
        List.take_left' (lsb7Bits_length _),
        List.take_left' (lsb7Bits_length _)] at h7
    exact h7
  have hmod : c1.mag % 128 = c2.mag % 128 :=
    lsb7Bits_injective (Nat.mod_lt _ (by omega)) (Nat.mod_lt _ (by omega)) hlsb_match
  -- Reconstruct `mag` from `mag / 128` and `mag % 128`.
  have hmag_eq : c1.mag = c2.mag := by
    have e1 := Nat.div_add_mod c1.mag 128
    have e2 := Nat.div_add_mod c2.mag 128
    omega
  exact (Coeff.ext_iff c1 c2).mpr ⟨hsign, hmag_eq⟩

/-- **Unary-suffix concatenation injectivity.** The pattern
    `replicate k false ++ true :: rest` decodes uniquely: the leading
    `true` marks position `k`, and the suffix is `rest`. This is the
    "split at first `true`" fact used to lift `encodeCoeff_drop8` to
    a prefix-free statement. -/
private theorem unary_concat_inj :
    ∀ {k1 k2 : Nat} {rest1 rest2 : List Bool},
      List.replicate k1 false ++ true :: rest1 =
        List.replicate k2 false ++ true :: rest2 →
      k1 = k2 ∧ rest1 = rest2
  | 0, 0, _, _, h => by simp at h; exact ⟨rfl, h⟩
  | 0, _ + 1, _, _, h => by simp [List.replicate_succ] at h
  | _ + 1, 0, _, _, h => by simp [List.replicate_succ] at h
  | _ + 1, _ + 1, _, _, h => by
      simp [List.replicate_succ] at h
      have ih := unary_concat_inj h
      exact ⟨by omega, ih.2⟩

/-- **Prefix-free property.** Encodings of two coefficients with arbitrary
    suffixes that concatenate to the same bit-list must agree element-wise.

    This is the load-bearing fact for sequence-level canonicality: a decoder
    can unambiguously locate the boundary between successive coefficient
    encodings — no `encodeCoeff` is a proper prefix of another. -/
theorem encodeCoeff_prefix_free (c1 c2 : Coeff) (tail1 tail2 : List Bool)
    (h : encodeCoeff c1 ++ tail1 = encodeCoeff c2 ++ tail2) :
    c1 = c2 ∧ tail1 = tail2 := by
  -- Strip the 8-bit fixed prefix (sign + 7 LSBs) on both sides; what
  -- remains is `unary(mag/128) ++ [true] ++ tail` on each side, and
  -- `unary_concat_inj` matches the unary terminators.
  have h8a : 8 ≤ (encodeCoeff c1).length := by rw [encodeCoeff_length]; omega
  have h8b : 8 ≤ (encodeCoeff c2).length := by rw [encodeCoeff_length]; omega
  have hd : (encodeCoeff c1).drop 8 ++ tail1 =
            (encodeCoeff c2).drop 8 ++ tail2 := by
    have hh := congrArg (List.drop 8) h
    rwa [List.drop_append_of_le_length h8a,
         List.drop_append_of_le_length h8b] at hh
  rw [encodeCoeff_drop8, encodeCoeff_drop8,
      List.append_assoc, List.append_assoc] at hd
  obtain ⟨hkeq, htail⟩ := unary_concat_inj hd
  -- Equal `mag/128` ⇒ equal encoding lengths ⇒ encodings themselves agree.
  have hlen : (encodeCoeff c1).length = (encodeCoeff c2).length := by
    rw [encodeCoeff_length, encodeCoeff_length, hkeq]
  refine ⟨encodeCoeff_injective c1 c2 ?_, htail⟩
  have hh := congrArg (List.take (encodeCoeff c1).length) h
  rwa [List.take_left, hlen, List.take_left] at hh

/-- Pure-prefix form: if one encoding is a prefix of another, the underlying
    coefficients agree and the prefix is exact. -/
theorem encodeCoeff_no_proper_prefix (c1 c2 : Coeff) (extra : List Bool)
    (h : encodeCoeff c2 = encodeCoeff c1 ++ extra) :
    c1 = c2 ∧ extra = [] :=
  encodeCoeff_prefix_free c1 c2 extra [] (by rw [h, List.append_nil])

/-! ## §3. Concatenated sequence encoding (n-element pipeline)

The bit stream the Rust codec produces for the s2 portion of a Falcon-512
signature is the per-coefficient encoding concatenated in order. The
prefix-free property of §2 lifts to whole-sequence injectivity. -/

/-- Bit-encoding of a coefficient sequence: per-coefficient encodings
    concatenated in order. Mirrors the Rust codec's compression path for
    the full N=512 signature payload. -/
def encodeAll : List Coeff → List Bool
  | []      => []
  | c :: cs => encodeCoeff c ++ encodeAll cs

@[simp] theorem encodeAll_nil : encodeAll [] = [] := rfl

@[simp] theorem encodeAll_cons (c : Coeff) (cs : List Coeff) :
    encodeAll (c :: cs) = encodeCoeff c ++ encodeAll cs := rfl

/-- **Sequence-level canonicality.** Coefficient sequences with equal
    concatenated bit-encodings are equal — the n-element pipeline
    statement. Proof: induction on the first list, with the cons-cons
    case discharged by `encodeCoeff_prefix_free`. -/
theorem encodeAll_injective :
    ∀ (cs1 cs2 : List Coeff), encodeAll cs1 = encodeAll cs2 → cs1 = cs2
  | [], [], _ => rfl
  | [], c2 :: _, h => by
      simp [encodeAll] at h
      exact absurd h.1 (encodeCoeff_ne_nil c2)
  | c1 :: _, [], h => by
      simp [encodeAll] at h
      exact absurd h.1 (encodeCoeff_ne_nil c1)
  | c1 :: cs1, c2 :: cs2, h => by
      change encodeCoeff c1 ++ encodeAll cs1 = encodeCoeff c2 ++ encodeAll cs2 at h
      obtain ⟨hc, htail⟩ := encodeCoeff_prefix_free c1 c2 _ _ h
      rw [hc, encodeAll_injective cs1 cs2 htail]

/-- Length formula: 9 fixed bits per coefficient (sign + 7 LSB + unary
    terminator) plus the unary-tail length `mag / 128` per coefficient. -/
theorem encodeAll_length (cs : List Coeff) :
    (encodeAll cs).length = 9 * cs.length + (cs.map (·.mag / 128)).sum := by
  induction cs with
  | nil => simp [encodeAll]
  | cons _ _ ih =>
      simp [encodeAll, encodeCoeff_length, ih, List.length_cons]
      ring

-- N=512 specialization ------------------------------------------------------

/-- A Falcon-512 coefficient payload: an N-long list of valid coefficients. -/
def CoeffN := { cs : List Coeff // cs.length = Falcon512.Spec.N }

/-- Bit-encoding of a length-N coefficient sequence. -/
def encodeFalcon (cs : CoeffN) : List Bool := encodeAll cs.val

/-- **Falcon-512 bit-level canonicality.** Distinct length-N coefficient
    sequences produce distinct bit streams. -/
theorem encodeFalcon_injective (cs1 cs2 : CoeffN)
    (h : encodeFalcon cs1 = encodeFalcon cs2) : cs1 = cs2 :=
  Subtype.ext (encodeAll_injective cs1.val cs2.val h)

/-! ## §4. Wire format: zero-pad and byte packing

The Rust decoder is intended to read a fixed-length byte buffer, expand it
MSB-first into a bit stream, and reject any input where (a) leftover bits in
the accumulator after consuming N coefficients are nonzero, or (b) trailing
bytes past the consumed prefix are nonzero. The abstract model captures the
corresponding canonical shape as
`encodeAll cs ++ replicate _ false` for some zero-pad length determined by
the byte boundary.

This section formalizes that view in two steps:

  - `append_replicate_false_inj` (the load-bearing fact): if both bit
    streams end in `true`, the zero-pad and the underlying stream are
    each separately determined.
  - `packBytes` / `unpackBytes`: an MSB-first bytewise pack/unpack pair
    with `unpackBytes_packBytes` as the round-trip; injectivity of
    packing follows. -/

-- §4.1 — Last bit of any non-empty encoding is the unary terminator. -----

private theorem encodeCoeff_getLast?_eq_some_true (c : Coeff) :
    (encodeCoeff c).getLast? = some true := by
  show ((c.sign :: lsb7Bits (c.mag % 128)) ++ List.replicate (c.mag / 128) false
        ++ [true]).getLast? = some true
  exact List.getLast?_concat _

private theorem encodeAll_ne_nil_of_ne_nil (cs : List Coeff) (h : cs ≠ []) :
    encodeAll cs ≠ [] := by
  cases cs with
  | nil => exact absurd rfl h
  | cons c _ =>
    rw [encodeAll_cons]
    exact fun heq => encodeCoeff_ne_nil c (List.append_eq_nil.mp heq).1

private theorem encodeAll_getLast?_eq_some_true (cs : List Coeff) (h : cs ≠ []) :
    (encodeAll cs).getLast? = some true := by
  induction cs with
  | nil => exact absurd rfl h
  | cons c cs ih =>
    by_cases hcs : cs = []
    · subst hcs
      rw [encodeAll_cons, encodeAll_nil, List.append_nil]
      exact encodeCoeff_getLast?_eq_some_true c
    · rw [encodeAll_cons,
          List.getLast?_append_of_ne_nil _ (encodeAll_ne_nil_of_ne_nil cs hcs)]
      exact ih hcs

-- §4.2 — Zero-pad cancellation. -----------------------------------------

/-- **Zero-pad cancellation.** Two bit-streams each followed by a zero-pad
    agree iff the underlying streams agree and the pads are equal length —
    provided both streams end in `true`.

    Proof outline: WLOG `bs1.length ≤ bs2.length`, so `bs1` is a prefix of
    `bs2`. The trailing segment of `bs2` past `bs1.length` lies inside
    `replicate z1 false` (hence is all-zero), but `bs2` ends in `true`, so
    that segment must be empty. Equal lengths plus `List.append_inj` close
    out both equalities. -/
theorem append_replicate_false_inj
    {bs1 bs2 : List Bool} {z1 z2 : Nat}
    (h1 : bs1.getLast? = some true) (h2 : bs2.getLast? = some true)
    (h : bs1 ++ List.replicate z1 false = bs2 ++ List.replicate z2 false) :
    bs1 = bs2 ∧ z1 = z2 := by
  -- WLOG assume `bs1.length ≤ bs2.length`; symmetric case follows by swap.
  suffices hwlog : ∀ (b1 b2 : List Bool) (n1 n2 : Nat),
      b1.getLast? = some true → b2.getLast? = some true →
      b1.length ≤ b2.length →
      b1 ++ List.replicate n1 false = b2 ++ List.replicate n2 false →
      b1 = b2 ∧ n1 = n2 by
    rcases le_total bs1.length bs2.length with hle | hle
    · exact hwlog bs1 bs2 z1 z2 h1 h2 hle h
    · obtain ⟨he, hn⟩ := hwlog bs2 bs1 z2 z1 h2 h1 hle h.symm
      exact ⟨he.symm, hn.symm⟩
  intro b1 b2 n1 n2 _ hb2 hlen heq
  -- `b1` is a prefix of `b2`.
  have hprefix : b1 = b2.take b1.length := by
    have := congrArg (List.take b1.length) heq
    rwa [List.take_left, List.take_append_of_le_length hlen] at this
  have hsplit : b2 = b1 ++ b2.drop b1.length := by
    conv_lhs => rw [← List.take_append_drop b1.length b2]
    rw [← hprefix]
  -- The trailing segment of `b2` is all-`false` (a prefix of `replicate n1 false`).
  have hdrop_all_false : ∀ x ∈ b2.drop b1.length, x = false := by
    have heq' : List.replicate n1 false =
                b2.drop b1.length ++ List.replicate n2 false := by
      have htmp := heq
      rw [hsplit, List.append_assoc] at htmp
      exact List.append_inj_right htmp rfl
    intro x hx
    have hmem : x ∈ b2.drop b1.length ++ List.replicate n2 false :=
      List.mem_append_left _ hx
    rw [← heq'] at hmem
    exact List.eq_of_mem_replicate hmem
  -- That segment must be empty (else `b2`'s last element would be `false`).
  have hdrop_nil : b2.drop b1.length = [] := by
    by_contra hne
    have hlast : b2.getLast? = (b2.drop b1.length).getLast? := by
      conv_lhs => rw [hsplit]
      rw [List.getLast?_append_of_ne_nil _ hne]
    rw [hlast] at hb2
    obtain ⟨head, tail, hd⟩ : ∃ head tail, b2.drop b1.length = head :: tail := by
      rcases hl : b2.drop b1.length with _ | ⟨head, tail⟩
      · exact absurd hl hne
      · exact ⟨head, tail, rfl⟩
    rw [hd] at hb2 hdrop_all_false
    have hlast_in : (head :: tail).getLast (List.cons_ne_nil _ _) ∈ head :: tail :=
      List.getLast_mem _
    have hf : (head :: tail).getLast (List.cons_ne_nil _ _) = false :=
      hdrop_all_false _ hlast_in
    rw [List.getLast?_eq_getLast _ (List.cons_ne_nil _ _), hf] at hb2
    exact (by decide : (true : Bool) ≠ false) (Option.some.inj hb2).symm
  have heq_bs : b1 = b2 := by rw [hsplit, hdrop_nil, List.append_nil]
  refine ⟨heq_bs, ?_⟩
  have := congrArg List.length heq
  simp [List.length_append, List.length_replicate, heq_bs] at this
  exact this

-- §4.3 — Bit-level wire format. -----------------------------------------

/-- The bit stream the Rust decoder logically sees: `encodeAll cs` plus
    enough trailing zero bits to reach a fixed total bit length. The
    trailing zeros stand for both the within-byte residual and the
    expansion of trailing zero bytes. -/
def wireBits (cs : CoeffN) (totalBits : Nat) : List Bool :=
  encodeAll cs.val ++ List.replicate (totalBits - (encodeAll cs.val).length) false

theorem wireBits_length (cs : CoeffN) (totalBits : Nat)
    (hb : (encodeAll cs.val).length ≤ totalBits) :
    (wireBits cs totalBits).length = totalBits := by
  unfold wireBits
  rw [List.length_append, List.length_replicate]
  omega

/-- **Bit-level wire-format injectivity.** Padded bit streams agree iff
    the underlying coefficient sequences agree. The `totalBits` argument
    is unconstrained because Nat subtraction saturates; in practice it
    is `8 * sigBytes` and at least the encoding length. -/
theorem wireBits_injective {cs1 cs2 : CoeffN} {totalBits : Nat}
    (h : wireBits cs1 totalBits = wireBits cs2 totalBits) :
    cs1 = cs2 := by
  have hN : Falcon512.Spec.N ≠ 0 := by unfold Falcon512.Spec.N; omega
  have hne1 : cs1.val ≠ [] := fun heq => hN (heq ▸ cs1.property).symm
  have hne2 : cs2.val ≠ [] := fun heq => hN (heq ▸ cs2.property).symm
  unfold wireBits at h
  obtain ⟨hbits, _⟩ := append_replicate_false_inj
    (encodeAll_getLast?_eq_some_true cs1.val hne1)
    (encodeAll_getLast?_eq_some_true cs2.val hne2) h
  exact encodeFalcon_injective cs1 cs2 hbits

-- §4.4 — MSB-first byte packing. -----------------------------------------
-- A concrete bijection on length-multiple-of-8 bit streams: `packBytes`
-- packs 8-at-a-time into `Nat ∈ [0, 256)`, `unpackBytes` is the explicit
-- left inverse, and `packBytes_injective` follows directly.

/-- One byte's worth of MSB-first bits packed into `Nat ∈ [0, 256)`. -/
private def packByte (b7 b6 b5 b4 b3 b2 b1 b0 : Bool) : Nat :=
  b7.toNat * 128 + b6.toNat * 64 + b5.toNat * 32 + b4.toNat * 16 +
  b3.toNat *   8 + b2.toNat *  4 + b1.toNat *  2 + b0.toNat

/-- Recover 8 MSB-first bits from a byte. -/
private def unpackByte (n : Nat) : List Bool :=
  [(n / 128) % 2 = 1, (n / 64) % 2 = 1, (n / 32) % 2 = 1, (n / 16) % 2 = 1,
   (n /   8) % 2 = 1, (n /  4) % 2 = 1, (n /  2) % 2 = 1,  n       % 2 = 1]

/-- Per-byte round-trip — verified by case split on the 8 Bool inputs
    (256 leaves, each `rfl`). -/
private theorem unpackByte_packByte (b7 b6 b5 b4 b3 b2 b1 b0 : Bool) :
    unpackByte (packByte b7 b6 b5 b4 b3 b2 b1 b0) =
      [b7, b6, b5, b4, b3, b2, b1, b0] := by
  cases b7 <;> cases b6 <;> cases b5 <;> cases b4 <;>
    cases b3 <;> cases b2 <;> cases b1 <;> cases b0 <;> rfl

/-- Pack a bit list into bytes, MSB-first, 8 bits per byte. Total of
    `<8` trailing bits at the end (if any) are dropped — meaningful only
    on length-multiple-of-8 inputs. -/
def packBytes : List Bool → List Nat
  | b7 :: b6 :: b5 :: b4 :: b3 :: b2 :: b1 :: b0 :: rest =>
      packByte b7 b6 b5 b4 b3 b2 b1 b0 :: packBytes rest
  | _ => []

/-- Unpack bytes back into bits. -/
def unpackBytes : List Nat → List Bool
  | [] => []
  | n :: ns => unpackByte n ++ unpackBytes ns

/-- **Pack/unpack round-trip.** On length-multiple-of-8 bit streams,
    `unpackBytes` recovers the original bits. The short-list cases
    (lengths 1..7) are vacuous because they violate the precondition. -/
theorem unpackBytes_packBytes :
    ∀ (bits : List Bool), bits.length % 8 = 0 →
      unpackBytes (packBytes bits) = bits
  | [], _ => rfl
  | b7 :: b6 :: b5 :: b4 :: b3 :: b2 :: b1 :: b0 :: rest, h => by
      have hrest : rest.length % 8 = 0 := by
        simp only [List.length_cons] at h; omega
      show unpackByte (packByte b7 b6 b5 b4 b3 b2 b1 b0) ++ unpackBytes (packBytes rest)
            = b7 :: b6 :: b5 :: b4 :: b3 :: b2 :: b1 :: b0 :: rest
      rw [unpackByte_packByte, unpackBytes_packBytes rest hrest]; rfl
  | [_], h | [_, _], h | [_, _, _], h | [_, _, _, _], h
  | [_, _, _, _, _], h | [_, _, _, _, _, _], h
  | [_, _, _, _, _, _, _], h => by simp [List.length_cons] at h

/-- **Byte packing is injective on length-multiple-of-8 inputs.** Direct
    corollary of `unpackBytes_packBytes`. -/
theorem packBytes_injective {bits1 bits2 : List Bool}
    (h1 : bits1.length % 8 = 0) (h2 : bits2.length % 8 = 0)
    (h : packBytes bits1 = packBytes bits2) : bits1 = bits2 := by
  rw [← unpackBytes_packBytes bits1 h1,
      ← unpackBytes_packBytes bits2 h2, h]

/-! ## §5. Falcon-512 byte-level signature canonicality

The headline result for the abstract serializer: equal serialized byte
payloads imply equal coefficient sequences. Rust-level acceptance is checked
separately by Kani and property tests in `src/`. -/

/-- The byte-level wire format: `wireBits` MSB-packed into bytes.
    `sigBytes` is the fixed signature byte length (e.g. 666 for Falcon-512). -/
def serializeFalcon (cs : CoeffN) (sigBytes : Nat) : List Nat :=
  packBytes (wireBits cs (8 * sigBytes))

/-- **Abstract byte-level canonicality.** Distinct length-N coefficient
    sequences produce distinct byte streams of any sufficient fixed
    length. The two `≤` hypotheses say the encoding fits in the byte
    buffer; in production, `sigBytes` is chosen large enough by the
    Falcon-512 specification. -/
theorem serializeFalcon_injective {cs1 cs2 : CoeffN} {sigBytes : Nat}
    (hb1 : (encodeAll cs1.val).length ≤ 8 * sigBytes)
    (hb2 : (encodeAll cs2.val).length ≤ 8 * sigBytes)
    (h : serializeFalcon cs1 sigBytes = serializeFalcon cs2 sigBytes) :
    cs1 = cs2 := by
  unfold serializeFalcon at h
  have hlen1 : (wireBits cs1 (8 * sigBytes)).length % 8 = 0 := by
    rw [wireBits_length cs1 _ hb1]; omega
  have hlen2 : (wireBits cs2 (8 * sigBytes)).length % 8 = 0 := by
    rw [wireBits_length cs2 _ hb2]; omega
  exact wireBits_injective (packBytes_injective hlen1 hlen2 h)

end Falcon512.Spec.Canonicality
