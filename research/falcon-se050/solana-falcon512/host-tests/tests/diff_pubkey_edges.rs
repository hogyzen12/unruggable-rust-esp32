use pqcrypto_falcon::falcon512;
use pqcrypto_traits::sign::{DetachedSignature, PublicKey};
use solana_falcon512::{
    FALCON_512_PUBKEY_LEN, FALCON_512_SIGNATURE_LEN, Falcon512Pubkey, Falcon512Signature,
};

const Q: u32 = 12289;
const N: usize = 512;

// Generate a valid (pk, padded-sig, raw-sig-bytes) tuple via PQClean. The
// raw bytes are kept so each test can construct its own
// `falcon512::DetachedSignature` for the differential side.
fn gen_pair(
    msg: &[u8],
) -> (
    [u8; FALCON_512_PUBKEY_LEN],
    [u8; FALCON_512_SIGNATURE_LEN],
    Vec<u8>,
) {
    let (pk, sk) = falcon512::keypair();
    let sig = falcon512::detached_sign(msg, &sk);
    let mut pk_arr = [0u8; FALCON_512_PUBKEY_LEN];
    pk_arr.copy_from_slice(pk.as_bytes());
    let mut sig_arr = [0u8; FALCON_512_SIGNATURE_LEN];
    sig_arr[..sig.as_bytes().len()].copy_from_slice(sig.as_bytes());
    (pk_arr, sig_arr, sig.as_bytes().to_vec())
}

fn sf_verify(pk: &[u8; FALCON_512_PUBKEY_LEN], sig: &Falcon512Signature, msg: &[u8]) -> bool {
    sig.verify(msg, Falcon512Pubkey::from_ref(pk))
}

fn pq_verify(pk_bytes: &[u8], sig: &falcon512::DetachedSignature, msg: &[u8]) -> bool {
    falcon512::PublicKey::from_bytes(pk_bytes)
        .ok()
        .map(|pk| falcon512::verify_detached_signature(sig, msg, &pk).is_ok())
        .unwrap_or(false)
}

// Decode 897-byte wire pubkey into 512 14-bit coefficients (no validity check).
fn decode_coeffs(pk: &[u8; FALCON_512_PUBKEY_LEN]) -> [u32; N] {
    let mut h = [0u32; N];
    let mut acc = 0u32;
    let mut acc_len = 0u32;
    let mut idx_in = 1usize;
    let mut idx_out = 0usize;
    while idx_out < N {
        acc = (acc << 8) | pk[idx_in] as u32;
        idx_in += 1;
        acc_len += 8;
        if acc_len >= 14 {
            acc_len -= 14;
            h[idx_out] = (acc >> acc_len) & 0x3FFF;
            idx_out += 1;
        }
    }
    h
}

// Encode 512 14-bit values into 897-byte wire pubkey. Accepts values up to
// 16383 so we can synthesise pubkeys with out-of-range coefficients.
fn encode_coeffs(coeffs: &[u32; N]) -> [u8; FALCON_512_PUBKEY_LEN] {
    let mut out = [0u8; FALCON_512_PUBKEY_LEN];
    out[0] = 0x09;
    let mut acc = 0u32;
    let mut acc_len = 0u32;
    let mut idx_out = 1usize;
    for &c in coeffs.iter() {
        acc = (acc << 14) | (c & 0x3FFF);
        acc_len += 14;
        while acc_len >= 8 {
            acc_len -= 8;
            out[idx_out] = ((acc >> acc_len) & 0xFF) as u8;
            idx_out += 1;
        }
    }
    out
}

#[test]
fn rejects_pubkey_with_invalid_header_byte() {
    let msg = b"invalid header byte test";
    let (pk_orig, sig_bytes, pq_sig_raw) = gen_pair(msg);
    let sig = Falcon512Signature::from(sig_bytes);
    let pq_sig = falcon512::DetachedSignature::from_bytes(&pq_sig_raw).unwrap();

    // Sanity-check that the unmutated pair verifies in both impls.
    assert!(sf_verify(&pk_orig, &sig, msg));
    assert!(pq_verify(&pk_orig, &pq_sig, msg));

    for header in 0u8..=0xFF {
        let mut pk = pk_orig;
        pk[0] = header;
        let sf = sf_verify(&pk, &sig, msg);
        let pq_ok = pq_verify(&pk, &pq_sig, msg);
        assert_eq!(
            sf, pq_ok,
            "disagree at header={header:#04x}: sf={sf} pq={pq_ok}"
        );
        if header != 0x09 {
            assert!(!sf, "header {header:#04x} unexpectedly accepted");
        }
    }
}

#[test]
fn rejects_pubkey_with_coefficient_eq_q() {
    let msg = b"coeff = q test";
    let (pk_orig, sig_bytes, pq_sig_raw) = gen_pair(msg);
    let sig = Falcon512Signature::from(sig_bytes);
    let pq_sig = falcon512::DetachedSignature::from_bytes(&pq_sig_raw).unwrap();

    let coeffs = decode_coeffs(&pk_orig);
    for &pos in &[0usize, 1, 2, 16, 100, 255, 511] {
        let mut c = coeffs;
        c[pos] = Q;
        let pk = encode_coeffs(&c);
        let sf = sf_verify(&pk, &sig, msg);
        let pq_ok = pq_verify(&pk, &pq_sig, msg);
        assert_eq!(sf, pq_ok, "disagree at pos={pos}: sf={sf} pq={pq_ok}");
        assert!(!sf, "coeff=q at pos {pos} unexpectedly accepted");
    }
}

#[test]
fn rejects_pubkey_with_coefficient_above_q() {
    let msg = b"coeff > q test";
    let (pk_orig, sig_bytes, pq_sig_raw) = gen_pair(msg);
    let sig = Falcon512Signature::from(sig_bytes);
    let pq_sig = falcon512::DetachedSignature::from_bytes(&pq_sig_raw).unwrap();

    let coeffs = decode_coeffs(&pk_orig);
    let bad_vals: &[u32] = &[Q + 1, Q + 2, 13000, 14000, 15000, 16000, 16382, 16383];

    for &pos in &[0usize, 1, 7, 100, 255, 256, 511] {
        for &v in bad_vals {
            let mut c = coeffs;
            c[pos] = v;
            let pk = encode_coeffs(&c);
            let sf = sf_verify(&pk, &sig, msg);
            let pq_ok = pq_verify(&pk, &pq_sig, msg);
            assert_eq!(
                sf, pq_ok,
                "disagree at pos={pos} val={v}: sf={sf} pq={pq_ok}"
            );
            assert!(!sf, "coeff={v} at pos {pos} unexpectedly accepted");
        }
    }
}

#[test]
fn rejects_pubkey_with_many_invalid_coefficients() {
    let msg = b"many bad coefficients test";
    let (pk_orig, sig_bytes, pq_sig_raw) = gen_pair(msg);
    let sig = Falcon512Signature::from(sig_bytes);
    let pq_sig = falcon512::DetachedSignature::from_bytes(&pq_sig_raw).unwrap();

    // First 100 coefficients set to q.
    let coeffs = decode_coeffs(&pk_orig);
    let mut c = coeffs;
    for v in &mut c[..100] {
        *v = Q;
    }
    let pk = encode_coeffs(&c);
    let sf = sf_verify(&pk, &sig, msg);
    let pq_ok = pq_verify(&pk, &pq_sig, msg);
    assert_eq!(sf, pq_ok);
    assert!(!sf);

    // All 512 coefficients = 16383 (max 14-bit pattern, far above q).
    let pk_max = encode_coeffs(&[16383u32; N]);
    let sf2 = sf_verify(&pk_max, &sig, msg);
    let pq_ok2 = pq_verify(&pk_max, &pq_sig, msg);
    assert_eq!(sf2, pq_ok2);
    assert!(!sf2);
}

#[test]
fn agrees_on_all_zero_pubkey() {
    // `h = 0` parses as well-formed (every coefficient is 0, < q) but no
    // real keypair generates it. We only assert that both impls reach the
    // same accept/reject verdict — the behaviour itself (almost certainly
    // reject on the norm check, since `s1 = c` for any signed sig) is not
    // the point.
    let msg = b"all zero pubkey test";
    let (_, sig_bytes, pq_sig_raw) = gen_pair(msg);
    let sig = Falcon512Signature::from(sig_bytes);
    let pq_sig = falcon512::DetachedSignature::from_bytes(&pq_sig_raw).unwrap();

    let pk = encode_coeffs(&[0u32; N]);
    assert_eq!(pk[0], 0x09);
    let sf = sf_verify(&pk, &sig, msg);
    let pq_ok = pq_verify(&pk, &pq_sig, msg);
    assert_eq!(sf, pq_ok);
}

#[test]
fn agrees_on_constant_pubkey() {
    // Pubkey with all coefficients equal to some constant. Degenerate but
    // well-formed; both impls should reach the same verdict.
    let msg = b"constant pubkey test";
    let (_, sig_bytes, pq_sig_raw) = gen_pair(msg);
    let sig = Falcon512Signature::from(sig_bytes);
    let pq_sig = falcon512::DetachedSignature::from_bytes(&pq_sig_raw).unwrap();

    for &c in &[1u32, 2, 1000, 6144, Q - 1] {
        let pk = encode_coeffs(&[c; N]);
        let sf = sf_verify(&pk, &sig, msg);
        let pq_ok = pq_verify(&pk, &pq_sig, msg);
        assert_eq!(sf, pq_ok, "disagree at constant={c}: sf={sf} pq={pq_ok}");
    }
}

#[test]
fn rejects_truncated_pubkey() {
    let msg = b"truncated pubkey test";
    let (pk_orig, _, _) = gen_pair(msg);
    let truncated = &pk_orig[..896];
    let sf_ok = Falcon512Pubkey::try_from_slice(truncated).is_ok();
    let pq_ok = falcon512::PublicKey::from_bytes(truncated).is_ok();
    assert_eq!(sf_ok, pq_ok);
    assert!(!sf_ok);
}

#[test]
fn rejects_extended_pubkey() {
    let msg = b"extended pubkey test";
    let (pk_orig, _, _) = gen_pair(msg);
    let mut extended = [0u8; 898];
    extended[..897].copy_from_slice(&pk_orig);
    extended[897] = 0xAB; // garbage trailing byte
    let sf_ok = Falcon512Pubkey::try_from_slice(&extended).is_ok();
    let pq_ok = falcon512::PublicKey::from_bytes(&extended).is_ok();
    assert_eq!(sf_ok, pq_ok);
    assert!(!sf_ok);
}

#[test]
fn agrees_on_bit_shifted_pubkey_encoding() {
    // Shift every byte after the header by one bit, in either direction.
    // This aliases coefficient boundaries and almost always produces an
    // out-of-range coefficient somewhere; both impls should reject.
    let msg = b"bit shift test";
    let (pk_orig, sig_bytes, pq_sig_raw) = gen_pair(msg);
    let sig = Falcon512Signature::from(sig_bytes);
    let pq_sig = falcon512::DetachedSignature::from_bytes(&pq_sig_raw).unwrap();

    let mut shifted_l = pk_orig;
    let mut carry = 0u8;
    for byte in &mut shifted_l[1..] {
        let next = *byte >> 7;
        *byte = (*byte << 1) | carry;
        carry = next;
    }
    let sf_l = sf_verify(&shifted_l, &sig, msg);
    let pq_l = pq_verify(&shifted_l, &pq_sig, msg);
    assert_eq!(sf_l, pq_l);

    let mut shifted_r = pk_orig;
    let mut borrow = 0u8;
    for byte in &mut shifted_r[1..] {
        let next = *byte & 1;
        *byte = (*byte >> 1) | (borrow << 7);
        borrow = next;
    }
    let sf_r = sf_verify(&shifted_r, &sig, msg);
    let pq_r = pq_verify(&shifted_r, &pq_sig, msg);
    assert_eq!(sf_r, pq_r);
}
