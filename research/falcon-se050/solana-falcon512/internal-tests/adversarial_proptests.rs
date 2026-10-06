//! Adversarial proptest coverage for `validate()`, the decompressor, decoder,
//! header-byte rejection, prepared-pubkey range checks, and a sampled
//! refinement against the Lean encodeAll spec. The companion file
//! `kani_proofs.rs` carries the formal-verification harnesses — together
//! they cover both ends: Kani proves bitvector-exact bounds and
//! single-coefficient spec refinement; this file exercises loop-level
//! composition and whole-array decoder behaviour on random inputs.
//!
//! Loaded into the lib via `#[cfg(test)] #[path]` from `src/lib.rs`; the
//! `internal-tests/` directory is excluded from the published tarball by
//! [package.exclude].

use crate::codec::{decode_pubkey_u32, decompress_signature};
use crate::{N, Q};
use proptest::prelude::*;

proptest! {
    /// If decompress succeeds on random input, s2 must be bounded.
    #[test]
    fn decompress_output_bounded(buf in proptest::collection::vec(any::<u8>(), 625)) {
        let mut s2 = [0i16; N];
        if decompress_signature(&buf, &mut s2) {
            for &v in s2.iter() {
                prop_assert!((-2047..=2047).contains(&v),
                    "s2 coefficient {} out of [-2047, 2047]", v);
            }
        }
    }

    /// If decode succeeds on random input, all coefficients < Q.
    #[test]
    fn decode_pubkey_output_bounded(buf in proptest::collection::vec(any::<u8>(), 896)) {
        let mut h = [0u32; N];
        if decode_pubkey_u32(&buf, &mut h) {
            for &v in h.iter() {
                prop_assert!(v < Q,
                    "pubkey coefficient {} >= Q", v);
            }
        }
    }

    /// Single-bit flip in a valid pubkey encoding is always detected.
    #[test]
    fn pubkey_bit_flip_detected(bit_pos in 0usize..7168) {
        let buf_orig = [0u8; 896]; // all-zero coefficients (valid)
        let mut h_orig = [0u32; N];
        assert!(decode_pubkey_u32(&buf_orig, &mut h_orig));

        let mut buf = buf_orig;
        buf[bit_pos / 8] ^= 1 << (bit_pos % 8);

        let mut h_flipped = [0u32; N];
        if decode_pubkey_u32(&buf, &mut h_flipped) {
            prop_assert!(h_orig != h_flipped,
                "bit flip at position {} was not detected", bit_pos);
        }
    }

    /// verify_with_prepared rejects any non-0x39 header.
    #[test]
    fn header_rejection_exhaustive(header in 0u8..=255u8) {
        if header != 0x39 {
            let mut sig = [0u8; crate::FALCON_512_SIGNATURE_LEN];
            sig[0] = header;
            let sig = crate::Falcon512Signature::from_bytes(sig);
            let prepared = crate::Falcon512PreparedPubkey::from_bytes(
                [0u8; crate::FALCON_512_PREPARED_PUBKEY_LEN],
            );
            prop_assert!(!sig.verify_with_prepared(b"test", &prepared),
                "header 0x{:02x} should be rejected", header);
        }
    }

    /// verify rejects any non-0x09 pubkey header.
    #[test]
    fn pk_header_rejection_exhaustive(header in 0u8..=255u8) {
        if header != 0x09 {
            let mut pk_bytes = [0u8; crate::FALCON_512_PUBKEY_LEN];
            pk_bytes[0] = header;
            let pk = crate::Falcon512Pubkey::from_bytes(pk_bytes);
            // Need valid sig header to reach pk check:
            let mut sig_bytes = [0u8; crate::FALCON_512_SIGNATURE_LEN];
            sig_bytes[0] = 0x39;
            let sig = crate::Falcon512Signature::from_bytes(sig_bytes);
            prop_assert!(!sig.verify(b"test", &pk),
                "pk header 0x{:02x} should be rejected", header);
        }
    }

    /// validate() catches random out-of-range coefficients.
    #[test]
    fn prepared_pubkey_validate_catches_bad_coeff(
        pos in 0usize..512usize,
        coeff in 12289u16..=u16::MAX,
    ) {
        let mut bytes = [0u8; crate::FALCON_512_PREPARED_PUBKEY_LEN];
        bytes[2 * pos] = (coeff & 0xFF) as u8;
        bytes[2 * pos + 1] = (coeff >> 8) as u8;
        let pk = crate::Falcon512PreparedPubkey::from_bytes(bytes);
        prop_assert!(!pk.validate(),
            "validate should reject coeff {} at pos {}", coeff, pos);
    }

    /// validate() accepts prepared pubkeys produced by prepare_pubkey()
    /// from any wire pubkey whose 14-bit-packed coefficients all lie in
    /// `[0, Q)`. The proptest builds the wire form by 14-bit-packing
    /// random in-range coefficients, then runs prepare → validate.
    #[test]
    fn prepared_pubkey_from_prepare_is_valid(
        coeffs in proptest::collection::vec(0u32..Q, N..=N),
    ) {
        // 14-bit MSB-first pack into the wire pubkey body (mirrors
        // the helper used in `codec::tests::pubkey_decode_round_trip`).
        let mut pk_bytes = [0u8; crate::FALCON_512_PUBKEY_LEN];
        pk_bytes[0] = 0x09;
        let mut acc: u32 = 0;
        let mut acc_len: u32 = 0;
        let mut idx: usize = 1; // body starts after the 1-byte header
        for &w in coeffs.iter() {
            acc = (acc << 14) | w;
            acc_len += 14;
            while acc_len >= 8 {
                acc_len -= 8;
                pk_bytes[idx] = (acc >> acc_len) as u8;
                idx += 1;
            }
        }
        if acc_len > 0 {
            pk_bytes[idx] = (acc << (8 - acc_len)) as u8;
        }

        let pk = crate::Falcon512Pubkey::from_bytes(pk_bytes);
        let prepared = pk.prepare_pubkey();
        prop_assert!(prepared.validate(),
            "prepared pubkey from prepare_pubkey() should always validate");
    }

    /// try_from_slice rejects out-of-range coefficients.
    #[test]
    fn try_from_slice_rejects_bad_coeff(
        pos in 0usize..512usize,
        coeff in 12289u16..=u16::MAX,
    ) {
        // Build a 1024-byte aligned buffer with one bad coefficient.
        let mut data = vec![0u8; crate::FALCON_512_PREPARED_PUBKEY_LEN];
        data[2 * pos] = (coeff & 0xFF) as u8;
        data[2 * pos + 1] = (coeff >> 8) as u8;
        let result = crate::Falcon512PreparedPubkey::try_from_slice(&data);
        prop_assert!(result.is_err(),
            "try_from_slice should reject coeff {} at pos {}", coeff, pos);
    }
}

// -------------------------------------------------------------------
// Spec ↔ impl refinement: decompress_signature on canonical N=512
// bytes built directly from the Lean `encodeAll` definition.
//
// The Lean side proves that the spec encoder
//     encodeCoeff c = sign :: lsb7Bits(mag % 128) ++
//                     replicate(mag/128, false) ++ [true]
// composes injectively over a coefficient sequence
// (`encodeAll_injective`, `serializeFalcon_injective`).
//
// This proptest is an operational refinement check: we emit those
// exact bits in `encode_coeff_spec_bits`, MSB-pack into the wire
// bytes via `pack_msb_first`, feed to the Rust `decompress_signature`,
// and demand exact recovery for the sampled N=512 sequences. Together
// with the per-coefficient Kani harness
// `decompress_one_coeff_matches_spec`, it gives implementation
// evidence for the Rust<->Lean bridge without claiming a whole-array
// symbolic refinement proof.
// -------------------------------------------------------------------

/// Lean-spec mirror: bit-list of the 8-bit header followed by the
/// unary tail `replicate (mag/128) false ++ [true]`.
fn encode_coeff_spec_bits(sign: bool, mag: u16, out: &mut Vec<bool>) {
    out.push(sign);
    let lo = (mag % 128) as u32;
    for k in 0..7u32 {
        out.push((lo >> (6 - k)) & 1 == 1);
    }
    let high = (mag / 128) as usize;
    for _ in 0..high {
        out.push(false);
    }
    out.push(true);
}

/// MSB-first bit-to-byte packing, mirroring Lean's `packBytes`. Pads
/// the trailing partial byte with zero bits and zero-fills the
/// remainder of `out` (the wire format's trailing zero-byte region).
fn pack_msb_first(bits: &[bool], out: &mut [u8]) {
    for slot in out.iter_mut() {
        *slot = 0;
    }
    for (i, &b) in bits.iter().enumerate() {
        if b {
            out[i / 8] |= 1u8 << (7 - (i % 8));
        }
    }
}

/// Generate one valid coefficient: small magnitudes are common,
/// occasional larger ones exercise the unary tail (mag/128 ≥ 1).
/// Total bit budget across N=512 coefficients must stay under
/// 625*8 = 5000. The bias usually keeps `Σ mag/128` under that
/// budget; oversized generated cases are discarded by `prop_assume!`.
fn coeff_strategy() -> impl Strategy<Value = i16> {
    (0u16..2048u16, any::<bool>(), 0u8..100u8).prop_map(|(raw, sign_raw, bias)| {
        let mag: u16 = if bias < 80 {
            raw % 128 // mag/128 = 0
        } else if bias < 95 {
            raw % 256 // mag/128 ∈ {0, 1}
        } else {
            raw % 512 // mag/128 ∈ {0, 1, 2, 3}
        };
        let sign = sign_raw && mag != 0; // exclude malleable -0
        if sign { -(mag as i16) } else { mag as i16 }
    })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 200,
        .. ProptestConfig::default()
    })]

    /// **Spec-refinement sample.** Build canonical bytes for a random N=512
    /// coefficient sequence by emitting the Lean `encodeAll` bit pattern
    /// and `packBytes` MSB-pack, then `decompress_signature` recovers it
    /// for that sampled case.
    #[test]
    fn decompress_signature_matches_lean_encode_all_spec(
        s2 in proptest::collection::vec(coeff_strategy(), N..=N),
    ) {
        let s2: [i16; N] = s2.try_into().unwrap();

        // Build the canonical bit-stream via the Lean spec.
        let mut bits: Vec<bool> = Vec::with_capacity(N * 12);
        for &v in s2.iter() {
            let mag = v.unsigned_abs();
            let sign = v < 0;
            encode_coeff_spec_bits(sign, mag, &mut bits);
        }

        // Sanity: the spec encoding must fit the wire.
        prop_assume!(bits.len() <= 625 * 8);

        // Pack MSB-first into 625 bytes (zero-pad to wire length).
        let mut wire = [0u8; 625];
        pack_msb_first(&bits, &mut wire);

        // Round-trip through the Rust decoder.
        let mut decoded = [0i16; N];
        prop_assert!(
            decompress_signature(&wire, &mut decoded),
            "decompress rejected canonical spec encoding"
        );
        prop_assert_eq!(decoded, s2,
            "decompress recovered s2 != original (spec-refinement violation)");
    }
}
