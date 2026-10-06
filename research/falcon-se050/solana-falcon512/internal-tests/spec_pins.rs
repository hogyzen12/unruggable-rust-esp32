//! Spec-constant pins. Each verifier constant is asserted against an
//! independently-derivable spec value, with a citation. Lives inside the
//! lib so it has direct visibility into the private constants — failures
//! here mean a constant was edited away from its spec value without
//! updating this file.
//!
//! Sources:
//!   - the FN-DSA / Falcon submission spec (Falcon round-3 docs, §3.x),
//!   - FIPS 206 (NIST standardisation of FN-DSA, current draft),
//!   - the PQClean reference (`crypto_sign/falcon-512/clean/`).
//!
//! PQClean cross-checks of the actual wire bytes (header byte, length
//! cap) live separately in `host-tests/tests/pqclean_wire_format.rs`,
//! since they require the `pqcrypto-falcon` dev-dep.

use crate::ntt::{BIG_Q_FUSED_NORM, LAZY_OFFSET_GS, T_OFFSET_FUSED, T_OFFSET_LAZY_T};
use crate::{
    FALCON_512_PUBKEY_LEN, FALCON_512_SIGNATURE_LEN, L2_BOUND, N, NONCE_LEN, PUBKEY_HEADER, Q,
    SIG_HEADER,
};

/// Polynomial degree N = 512 — the "512" in Falcon-512.
/// Ring is Z_q[x] / (x^512 + 1).
#[test]
fn polynomial_degree_n_eq_512() {
    assert_eq!(N, 512);
    assert_eq!(N, 1 << 9, "N = 2^logn = 2^9 for Falcon-512");
}

/// Field modulus Q = 12289. Falcon spec §3.1: Q ≡ 1 (mod 2N) so the
/// negacyclic NTT exists. Specifically Q = 12·1024 + 1 for N = 512.
#[test]
fn modulus_q_eq_12289() {
    assert_eq!(Q, 12289);
    assert_eq!(Q as usize, 12 * 1024 + 1, "Q = 12·2^10 + 1 per Falcon §3.1");
    assert_eq!(
        (Q as usize) % (2 * N),
        1,
        "Q ≡ 1 (mod 2N) — required for negacyclic NTT"
    );
}

/// L2_BOUND = ⌊β²⌋, the L2-norm rejection bound. Falcon spec §3.9 / §4.2:
///   ‖s1‖² + ‖s2‖² ≤ ⌊β²⌋,  β² = (1.1)²·σ²·2N for σ tuned to Falcon-512.
/// PQClean publishes the rounded constant as `l2bound[9] = 34034726`
/// (`crypto_sign/falcon-512/clean/vrfy.c`).
#[test]
fn l2_bound_matches_pqclean() {
    assert_eq!(L2_BOUND, 34_034_726);
}

/// Salt length = 40 bytes (320 bits). Falcon spec §3.11.3.
#[test]
fn nonce_len_eq_40() {
    assert_eq!(NONCE_LEN, 40);
}

/// Pubkey wire-format header. Falcon spec §3.11.1:
///   pk = [0x00 | logn] || pack_14bit(h)
/// For Falcon-512 (logn = 9), header = 0x00 | 9 = 0x09.
#[test]
#[allow(clippy::identity_op)] // `0x00 | logn` mirrors spec §3.11.1 derivation
fn pubkey_header_byte() {
    assert_eq!(PUBKEY_HEADER, 0x09);
    assert_eq!(PUBKEY_HEADER, 0x00 | 0x09_u8);
}

/// Compressed signature wire-format header. Falcon spec §3.11.3:
///   sig = [0x30 | logn] || nonce[40] || gr_encode(s2)
/// For Falcon-512 (logn = 9), header = 0x30 | 9 = 0x39.
#[test]
fn sig_header_byte() {
    assert_eq!(SIG_HEADER, 0x39);
    assert_eq!(SIG_HEADER, 0x30 | 0x09_u8);
}

/// Wire-format lengths: pk = 1 + ⌈N·14/8⌉ = 897, sig = 1 + nonce + 625 = 666.
/// Already enforced at compile time via `const _: () = assert!(...)` in
/// lib.rs; this test re-states the equation so it surfaces in `cargo test`.
#[test]
fn wire_format_lengths() {
    assert_eq!(FALCON_512_PUBKEY_LEN, 1 + (N * 14) / 8);
    assert_eq!(FALCON_512_PUBKEY_LEN, 897);
    assert_eq!(FALCON_512_SIGNATURE_LEN, 1 + NONCE_LEN + 625);
    assert_eq!(FALCON_512_SIGNATURE_LEN, 666);
}

/// HashToPoint rejection bound is 5·Q. Falcon spec §3.7: 16-bit candidates
/// accepted iff w < k·Q for the largest k with k·Q ≤ 2^16.
/// Q = 12289 ⇒ 5·Q = 61445 ≤ 65536 < 6·Q = 73734. The literal `5 * Q`
/// in `codec::hash_to_point` is pinned end-to-end by the PQClean
/// differential (any other k diverges immediately).
#[test]
#[allow(clippy::assertions_on_constants)] // tautological at compile time by design
fn hash_to_point_rejection_bound_is_5q() {
    assert!(5 * Q <= 1 << 16, "5·Q must fit in u16 candidates");
    assert!(6 * Q > 1 << 16, "6·Q would exceed 2^16, so 5·Q is the max");
}

// -----------------------------------------------------------------------
// Lazy-NTT optimisation offsets
//
// Each constant below has a matching Lean `def` in
// `formal_verification/Falcon512/Defs.lean`. The Lean refinement proofs
// (`Falcon512.Spec.Refinement.*`, `Falcon512.Spec.Bounds.*`) reason
// about the *named* constants, so any literal-value drift between the
// Rust source here and the Lean side surfaces by name when these tests
// fail or when the Lean `def`s are no longer reduced to the same value.
// -----------------------------------------------------------------------

/// Lazy-`t` CT butterfly offset. Matches `Falcon512.Spec.T_OFFSET_LAZY_T`. -/
#[test]
fn t_offset_lazy_t_eq_8_q_squared() {
    assert_eq!(T_OFFSET_LAZY_T, 8 * (Q as u64) * (Q as u64));
}

/// Lazy GS butterfly offset. Matches `Falcon512.Spec.LAZY_OFFSET_GS`. -/
#[test]
fn lazy_offset_gs_eq_256_q() {
    assert_eq!(LAZY_OFFSET_GS, 256 * (Q as u64));
}

/// Fused last-fwd / pointwise / first-inv `T` offset. Matches
/// `Falcon512.Spec.T_OFFSET_FUSED`. -/
#[test]
fn t_offset_fused_eq_q_2_pow_31() {
    assert_eq!(T_OFFSET_FUSED, (Q as u64) * (1u64 << 31));
}

/// Fused-norm subtraction offset. Matches `Falcon512.Spec.BIG_Q_FUSED_NORM`. -/
#[test]
fn big_q_fused_norm_eq_q_2_pow_23() {
    assert_eq!(BIG_Q_FUSED_NORM, (Q as u64) << 23);
}
