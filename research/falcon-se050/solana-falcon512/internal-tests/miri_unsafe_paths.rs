//! Miri-targeted single-shot tests for unsafe blocks on the verify path.
//! Loaded into the lib via `#[cfg(test)] #[path]` in src/lib.rs; the
//! whole `internal-tests/` directory is kept out of the published tarball
//! by [package.exclude].

use super::*;

// Single-shot tests targeted at Miri to exercise every `unsafe` block
// in the verify path exactly once. Designed for:
//     cargo +nightly miri test --lib miri_unsafe_paths
// The full proptest/differential suite is too slow under Miri's
// interpreter; this module is the minimum that hits each `unsafe`
// site without thousands of iterations.
//
// Coverage:
//   - `Falcon512Pubkey::from_ref` repr-transparent cast
//   - `Falcon512Signature::from_ref` repr-transparent cast
//   - `MaybeUninit` patterns in `verify_with_prepared` and
//     `norm_check_with_prepared`
//   - `decompress_signature`'s `read_unaligned` u64 trailing scan
//   - `hash_to_point`'s raw-pointer walk over the rate lanes and `c[]`
//   - `Falcon512PreparedPubkey::from_ref` alignment-required cast


// Embed a known-good (pubkey, signature, message) triple so we can run
// a real verify path through Miri without needing PQClean.
static PK_BYTES: &[u8; FALCON_512_PUBKEY_LEN] =
    include_bytes!("../program/tests/fixtures/falcon.pk");
static SIG_BYTES: &[u8; FALCON_512_SIGNATURE_LEN] =
    include_bytes!("../program/tests/fixtures/sample_sig.bin");
const MESSAGE: &[u8] = b"deterministic falcon-512 verify benchmark";

#[test]
fn verify_path_exercises_unsafe() {
    // Path 1: `from_ref` (repr-transparent cast) on both buffers.
    let pk: &Falcon512Pubkey = Falcon512Pubkey::from_ref(PK_BYTES);
    let sig: &Falcon512Signature = Falcon512Signature::from_ref(SIG_BYTES);
    // Path 2: raw `verify` — exercises `check_norm` -> NTT decode +
    // forward NTT + N_INV fold + norm.
    assert!(sig.verify(MESSAGE, pk));
}

#[test]
fn verify_with_prepared_path_exercises_unsafe() {
    // Path 3: `prepare_pubkey` (const fn, no unsafe but exercises
    // the `MaybeUninit` work below).
    let pk = Falcon512Pubkey::from(*PK_BYTES);
    let prepared = pk.prepare_pubkey();
    // Path 4: `verify_with_prepared` — exercises `MaybeUninit` in
    // `s2_buf` / `c_buf` and the `MaybeUninit` in
    // `norm_check_with_prepared::buf`.
    let sig: &Falcon512Signature = Falcon512Signature::from_ref(SIG_BYTES);
    assert!(sig.verify_with_prepared(MESSAGE, &prepared));
}

#[test]
fn prepared_pubkey_aligned_borrow_exercises_unsafe() {
    // Path 5: `Falcon512PreparedPubkey::try_from_slice` — exercises
    // `from_ref` (which has an unsafe alignment-required cast).
    let pk = Falcon512Pubkey::from(*PK_BYTES);
    let prepared = pk.prepare_pubkey();
    let bytes = prepared.as_bytes(); // exercises another transparent cast
    let borrowed =
        Falcon512PreparedPubkey::try_from_slice(bytes).expect("prepared bytes must validate");
    // Use it: verify against the borrowed form too.
    let sig: &Falcon512Signature = Falcon512Signature::from_ref(SIG_BYTES);
    assert!(sig.verify_with_prepared(MESSAGE, borrowed));
}
