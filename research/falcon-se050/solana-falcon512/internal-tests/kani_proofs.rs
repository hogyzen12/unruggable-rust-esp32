//! Kani formal-verification harnesses for Falcon-512 SBF-tuning constants,
//! decompressor refinement against the Lean encodeCoeff spec, and bitvector-
//! exact arithmetic bounds on the production butterfly kernels. Pairs with
//! the adversarial property tests in `adversarial_proptests.rs`: Kani covers
//! the math; proptest covers loop-level composition end-to-end.
//!
//! Loaded into the lib via `#[cfg(kani)] #[path]` from `src/lib.rs`; the
//! `internal-tests/` directory is excluded from the published tarball by
//! [package.exclude].
//!
//! Run: cargo kani --harness <name>

use crate::ntt::{
    T_OFFSET_LAZY_T, ct_butterfly_lazy_t_step, ct_butterfly_step, fused_norm_step,
    fused_step_kernel, gs_butterfly_lazy_step,
};
use crate::{N, Q};

// The algebraic preconditions on `T_OFFSET_LAZY_T`, `LAZY_OFFSET_GS`,
// `T_OFFSET_FUSED`, and `BIG_Q_FUSED_NORM` (multiple of Q, ≥ worst-case
// operand, ≤ u64::MAX − Q) are checked at the constant definitions in
// `ntt.rs` via `const _: () = assert!(...)`. The Kani harnesses below
// therefore only need to verify the *dynamic* obligation: given those
// preconditions, no production kernel exhibits arithmetic overflow or
// out-of-range output for any input in its lazy invariant range. Header
// / range / malleability rules are exercised end-to-end by the proptest
// section and, when ignored host tests are run, by
// `soak_differential_vs_pqclean` (host-tests/tests/soak.rs).

// =========================================================================
// CT butterfly outputs fit u32 under lazy invariant
// =========================================================================

/// Strict CT butterfly: a, b ≤ 10·Q, zeta < Q ⇒ both outputs fit u32.
#[kani::proof]
#[kani::solver(z3)]
fn ct_butterfly_no_overflow() {
    let a: u64 = kani::any();
    let b: u64 = kani::any();
    let zeta: u64 = kani::any();
    kani::assume(a <= 10 * Q as u64);
    kani::assume(b <= 10 * Q as u64);
    kani::assume(zeta < Q as u64);

    let (lo, hi) = ct_butterfly_step(a, b, zeta);
    assert!(lo <= u32::MAX as u64);
    assert!(hi <= u32::MAX as u64);
}

/// Lazy-`t` CT butterfly: a, b ≤ 8·Q, zeta < Q ⇒ both outputs fit u32.
/// Spec equivalence with the strict variant follows from the compile-time
/// `T_OFFSET_LAZY_T % Q == 0` const-assert in `ntt.rs`.
#[kani::proof]
#[kani::solver(z3)]
fn ct_butterfly_lazy_t_no_overflow() {
    let a: u64 = kani::any();
    let b: u64 = kani::any();
    let zeta: u64 = kani::any();
    kani::assume(a <= 8 * Q as u64);
    kani::assume(b <= 8 * Q as u64);
    kani::assume(zeta < Q as u64);

    let (lo, hi) = ct_butterfly_lazy_t_step(a, b, zeta);
    assert!(lo <= u32::MAX as u64);
    assert!(hi <= u32::MAX as u64);
}

// =========================================================================
// Inverse NTT lazy GS bounds
// =========================================================================

/// Lazy GS butterfly: u, v ≤ 256·Q, zeta < Q ⇒ low fits u32, high < Q.
/// `LAZY_OFFSET_GS` is large enough (compile-time const-assert in
/// `ntt.rs`) that `u + LAZY_OFFSET_GS − v` does not underflow.
#[kani::proof]
#[kani::solver(z3)]
fn inv_ntt_lazy_offset_sufficient() {
    let u: u64 = kani::any();
    let v: u64 = kani::any();
    let zeta: u64 = kani::any();
    kani::assume(u <= 256 * Q as u64);
    kani::assume(v <= 256 * Q as u64);
    kani::assume(zeta < Q as u64);

    let (lo, hi) = gs_butterfly_lazy_step(u, v, zeta);
    assert!(lo <= u32::MAX as u64);
    assert!(hi < Q as u64);
}

// =========================================================================
// Fused step (last fwd / pointwise / first inv): outputs fit u32
// =========================================================================

/// Per-pair fused-step kernel: with `prev_*` bounded by the lazy-t output
/// (8·Q + T_OFFSET_LAZY_T) and `h_*`, `z`, `z_inv < Q`, both outputs are
/// in [0, 2·Q) and [0, Q) respectively (fit u32). `T_OFFSET_FUSED` being
/// a multiple of Q and ≥ the worst-case `t` is checked at compile time
/// in `ntt.rs`.
#[kani::proof]
#[kani::solver(z3)]
fn fused_step_t_offset_sufficient() {
    let prev_low: u32 = kani::any();
    let prev_high: u32 = kani::any();
    let h_lo: u16 = kani::any();
    let h_hi: u16 = kani::any();
    let z: u64 = kani::any();
    let z_inv: u64 = kani::any();

    let lazy_t_bound = 8 * Q as u64 + T_OFFSET_LAZY_T;
    kani::assume(prev_low as u64 <= lazy_t_bound);
    kani::assume(prev_high as u64 <= lazy_t_bound);
    kani::assume((h_lo as u64) < Q as u64);
    kani::assume((h_hi as u64) < Q as u64);
    kani::assume(z < Q as u64);
    kani::assume(z_inv < Q as u64);

    let (new_low, new_high) = fused_step_kernel(prev_low, prev_high, h_lo, h_hi, z, z_inv);

    assert!(new_low < 2 * Q as u64);
    assert!(new_high < Q as u64);
}

// =========================================================================
// Fused norm: per-iter contribution stays under the worst-case bound
// =========================================================================

/// For any inputs in the lazy invariant range, one iteration of
/// `fused_norm_step` contributes at most `2·((Q/2)² + 2047²)`.
#[kani::proof]
#[kani::solver(z3)]
fn fused_norm_per_iter_safe() {
    let buf_lo: u32 = kani::any();
    let buf_hi: u32 = kani::any();
    let c_lo: u16 = kani::any();
    let c_hi: u16 = kani::any();
    let s2_lo: i16 = kani::any();
    let s2_hi: i16 = kani::any();
    let zeta: u64 = kani::any();

    let bound = 256 * Q as u64;
    kani::assume(buf_lo as u64 <= bound);
    kani::assume(buf_hi as u64 <= bound);
    kani::assume((c_lo as u64) < Q as u64);
    kani::assume((c_hi as u64) < Q as u64);
    kani::assume(s2_lo >= -2047 && s2_lo <= 2047);
    kani::assume(s2_hi >= -2047 && s2_hi <= 2047);
    kani::assume(zeta < Q as u64);

    let per_iter = fused_norm_step(buf_lo, buf_hi, c_lo, c_hi, s2_lo, s2_hi, zeta);

    let max_per_iter: u64 = 2 * ((Q as u64 / 2) * (Q as u64 / 2) + 2047u64 * 2047);
    assert!(per_iter <= max_per_iter);
}

// The norm-accumulation overflow + meaningfulness bound and the
// lazy-NTT level bound (K+1)·Q ≤ u32::MAX for K ≤ 9 are now compile-
// time `const _: () = assert!(...)` declarations in `lib.rs` and
// `ntt.rs` respectively — they are pure-const facts, not symbolic
// claims, and the const-assert form runs at compile time without
// a Kani harness shell.

// =========================================================================
// Prepared pubkey validation catches out-of-range coefficients
// =========================================================================

/// validate() rejects any prepared pubkey with a coefficient >= Q at
/// any of the N=512 slots.
#[kani::proof]
fn validate_rejects_out_of_range() {
    let coeff: u16 = kani::any();
    kani::assume(coeff as u32 >= Q);

    // Place the bad coefficient at any symbolic position in [0, N).
    // `usize` (not `u8`) is required to symbolically reach all 512 slots.
    let pos: usize = kani::any();
    kani::assume(pos < N);

    let mut bytes = [0u8; crate::FALCON_512_PREPARED_PUBKEY_LEN];
    // Write the bad coefficient as little-endian u16 at position pos.
    bytes[2 * pos] = (coeff & 0xFF) as u8;
    bytes[2 * pos + 1] = (coeff >> 8) as u8;

    let pk = crate::Falcon512PreparedPubkey::from_bytes(bytes);
    assert!(!pk.validate(), "validate should reject coefficient >= Q");
}

/// validate() accepts a prepared pubkey for a symbolic in-range
/// coefficient at an arbitrary position, with all other slots zero.
/// `validate()` inspects each slot independently, so this covers the
/// accepting branch for any chosen slot; it is not a simultaneous
/// symbolic quantification over all 512 coefficients.
#[kani::proof]
fn validate_accepts_valid() {
    let coeff: u16 = kani::any();
    kani::assume((coeff as u32) < Q);

    let pos: usize = kani::any();
    kani::assume(pos < N);

    let mut bytes = [0u8; crate::FALCON_512_PREPARED_PUBKEY_LEN];
    bytes[2 * pos] = (coeff & 0xFF) as u8;
    bytes[2 * pos + 1] = (coeff >> 8) as u8;

    let pk = crate::Falcon512PreparedPubkey::from_bytes(bytes);
    assert!(
        pk.validate(),
        "validate should accept any in-range coefficient"
    );
}

// =========================================================================
// decompress_one_coeff post-conditions
// =========================================================================
// Invokes the real `codec::decompress_one_coeff` helper (extracted from
// `decompress_signature`) with a symbolic 4-byte buffer and symbolic
// bit-stream state. Verifies the per-coefficient post-conditions over
// the modeled local state space:
//   - any returned value is in [-2047, 2047] (magnitude bound, R3+M2);
//   - the `acc_len <= 7` bit-stream invariant is preserved (precondition
//     of the next iteration).
// 4 bytes is sufficient for any single-coefficient call: 1 prefix +
// at most 2 bytes of unary tail before `m >= 2048` triggers rejection,
// plus 1 byte of headroom.

#[kani::proof]
#[kani::unwind(20)]
#[kani::solver(z3)]
fn decompress_one_coeff_invariants() {
    let buf: [u8; 4] = kani::any();
    let mut acc: u64 = kani::any();
    let mut acc_len: u64 = kani::any();
    let mut idx_in: usize = kani::any();
    kani::assume(acc_len <= 7);
    kani::assume(idx_in <= buf.len());

    let idx_in_before = idx_in;

    if let Some(v) =
        crate::codec::decompress_one_coeff(&buf, &mut acc, &mut acc_len, &mut idx_in)
    {
        // Output magnitude bound (Falcon spec §3.10).
        assert!(
            v >= -2047 && v <= 2047,
            "s2 coefficient {} out of [-2047, 2047]",
            v
        );
        // Bit-stream invariant preserved for the next call.
        assert!(acc_len <= 7, "bit-stream invariant violated");
        // Progress: idx_in advanced and stays within the buffer. A
        // single coefficient consumes at most 1 prefix byte + 2 unary
        // bytes = 3 bytes from the byte stream.
        assert!(idx_in >= idx_in_before, "idx_in regressed");
        assert!(idx_in <= idx_in_before + 3, "idx_in over-advanced");
        assert!(idx_in <= buf.len(), "idx_in past buffer end");
    }
}

// =========================================================================
// Codec refinement: decompress_one_coeff matches the Lean encodeCoeff spec
// =========================================================================
//
// Refinement check, symbolic over the full per-coefficient input space:
// for any valid Falcon coefficient `(sign, mag)` with `mag < 2048` and
// `(sign, mag) != (true, 0)` (the malleable -0 form), feeding the
// canonical `encodeCoeff`-style byte-packed encoding into the Rust
// decompressor recovers the original coefficient exactly.
//
// Pairs with the Lean theorems in `formal_verification/Falcon512/
// Canonicality.lean`:
//   - Lean: spec encoding is injective and prefix-free
//     (`encodeCoeff_injective`, `encodeCoeff_prefix_free`,
//      `serializeFalcon_injective`).
//   - Kani (this harness): Rust impl realises that spec on every valid
//     single-coefficient input, with idx_in / acc_len bit-stream
//     invariants intact.
//
// Together these establish the single-coefficient implementation path
// against the Lean encoding model. The two-coefficient lift is
// `decompress_two_coeffs_matches_spec` below (symbolic concatenation).
// Full N=512 Rust decoder behavior is checked operationally by the
// proptest suite (`canonicality_under_bit_flips` and
// `decompress_signature_matches_lean_encode_all_spec`), not by a
// whole-array symbolic proof.

/// Lean-spec mirror: append `encodeCoeff (sign, mag)` to the bit-stream
/// represented by `bytes` (MSB-first packing) starting at `*bit_idx`.
///   `bits = sign :: lsb7Bits(mag % 128) ++ replicate(mag/128, false) ++ [true]`
/// Caller must ensure `bytes` has room for `9 + mag/128` more bits.
fn encode_coeff_into_bytes(sign: bool, mag: u16, bytes: &mut [u8], bit_idx: &mut usize) {
    if sign {
        bytes[*bit_idx / 8] |= 1u8 << (7 - (*bit_idx % 8));
    }
    *bit_idx += 1;
    let lo = (mag % 128) as u32;
    for k in 0..7usize {
        if (lo >> (6 - k)) & 1 == 1 {
            bytes[*bit_idx / 8] |= 1u8 << (7 - (*bit_idx % 8));
        }
        *bit_idx += 1;
    }
    // Unary tail: `mag/128` zeros (already zero-initialised), then a 1.
    *bit_idx += (mag / 128) as usize;
    bytes[*bit_idx / 8] |= 1u8 << (7 - (*bit_idx % 8));
    *bit_idx += 1;
}

#[kani::proof]
#[kani::unwind(20)]
#[kani::solver(z3)]
fn decompress_one_coeff_matches_spec() {
    // Symbolic valid coefficient.
    let sign: bool = kani::any();
    let mag: u16 = kani::any();
    kani::assume(mag < 2048);
    kani::assume(!(sign && mag == 0));

    // Build the canonical bit-stream MSB-packed into 4 bytes (max 24
    // bits for mag = 2047).
    let mut bytes: [u8; 4] = [0; 4];
    let mut bit_idx: usize = 0;
    encode_coeff_into_bytes(sign, mag, &mut bytes, &mut bit_idx);
    let total_bits = bit_idx;

    // Run the Rust decompressor on the canonical encoding.
    let mut acc: u64 = 0;
    let mut acc_len: u64 = 0;
    let mut idx_in: usize = 0;
    let result =
        crate::codec::decompress_one_coeff(&bytes, &mut acc, &mut acc_len, &mut idx_in);

    // Refinement: the recovered i16 equals the spec coefficient.
    let expected: i16 = if sign { -(mag as i16) } else { mag as i16 };
    assert!(
        result == Some(expected),
        "decompress_one_coeff disagrees with encodeCoeff spec"
    );

    // Bit-stream invariants the next iteration relies on.
    let bytes_consumed = (total_bits + 7) / 8;
    assert!(idx_in == bytes_consumed, "idx_in advance mismatch");
    assert!(acc_len <= 7, "acc_len exceeds bit-stream invariant");
}

// =========================================================================
// Bounded-N concatenation: two coefficients
// =========================================================================
//
// Symbolic lift of the per-coefficient refinement to N=2: two valid
// coefficients, each canonically encoded per the Lean spec, concatenated
// MSB-first into a single byte buffer, are recovered exactly by two
// consecutive calls to `decompress_one_coeff`.
//
// This is the symbolic counterpart to `encodeAll_injective` in Lean
// restricted to two-element lists, plus the impl-side claim that the
// Rust decoder respects the prefix-free boundary. Full N=512 is out of
// reach for symbolic verification; the proptest
// `decompress_signature_matches_lean_encode_all_spec` covers that case
// operationally on random inputs.
//
// Buffer size: each coefficient is 9 + mag/128 ≤ 24 bits, so two fit in
// ≤ 48 bits = 6 bytes. We use 7 bytes for headroom.

#[kani::proof]
#[kani::unwind(20)]
#[kani::solver(z3)]
fn decompress_two_coeffs_matches_spec() {
    let s0: bool = kani::any();
    let m0: u16 = kani::any();
    let s1: bool = kani::any();
    let m1: u16 = kani::any();
    kani::assume(m0 < 2048);
    kani::assume(m1 < 2048);
    kani::assume(!(s0 && m0 == 0));
    kani::assume(!(s1 && m1 == 0));

    // Each coefficient takes 9 + mag/128 bits, max 24, so two together
    // are at most 48 bits — fits in 6 bytes with a byte of headroom.
    // Make the buffer-fits precondition explicit for any future bound bump.
    kani::assume((9 + (m0 as usize) / 128 + 9 + (m1 as usize) / 128) <= 7 * 8);

    // Concatenate the two canonical encodings into a 7-byte buffer.
    let mut bytes: [u8; 7] = [0; 7];
    let mut bit_idx: usize = 0;
    encode_coeff_into_bytes(s0, m0, &mut bytes, &mut bit_idx);
    encode_coeff_into_bytes(s1, m1, &mut bytes, &mut bit_idx);
    let total_bits = bit_idx;

    // Two consecutive `decompress_one_coeff` calls share the bit-stream
    // accumulator state — exactly what `decompress_signature` does in
    // its N-coefficient loop.
    let mut acc: u64 = 0;
    let mut acc_len: u64 = 0;
    let mut idx_in: usize = 0;
    let r0 = crate::codec::decompress_one_coeff(&bytes, &mut acc, &mut acc_len, &mut idx_in);
    let r1 = crate::codec::decompress_one_coeff(&bytes, &mut acc, &mut acc_len, &mut idx_in);

    let exp0: i16 = if s0 { -(m0 as i16) } else { m0 as i16 };
    let exp1: i16 = if s1 { -(m1 as i16) } else { m1 as i16 };
    assert!(r0 == Some(exp0), "first coefficient mismatch");
    assert!(
        r1 == Some(exp1),
        "second coefficient mismatch (concatenation broken)"
    );

    // Bit-stream invariants after two coefficients.
    let bytes_consumed = (total_bits + 7) / 8;
    assert!(
        idx_in == bytes_consumed,
        "idx_in advance mismatch after pair"
    );
    assert!(acc_len <= 7, "acc_len exceeds bit-stream invariant");
}
