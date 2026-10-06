//! PQClean cross-check: the wire bytes our crate exposes (header byte,
//! signature length cap) are what PQClean actually emits. Confirms that
//! our constants `FALCON_512_SIGNATURE_LEN` / `FALCON_512_PUBKEY_LEN`
//! and the documented header bytes (0x39 / 0x09) agree with the
//! reference rather than only with each other.
//!
//! The in-lib pins for `Q`, `N`, `L2_BOUND`, `NONCE_LEN`, `PUBKEY_HEADER`,
//! `SIG_HEADER`, the wire-length equations, and the 5·Q rejection bound
//! live in `internal-tests/spec_pins.rs` (loaded into the lib via
//! `#[cfg(test)] #[path]` from `src/lib.rs`).

use pqcrypto_falcon::falcon512;
use pqcrypto_traits::sign::{DetachedSignature, PublicKey};
use solana_falcon512::{FALCON_512_PUBKEY_LEN, FALCON_512_SIGNATURE_LEN};

/// Falcon spec §3.11.3:
///   sig = [0x30 | logn] || nonce[40] || gr_encode(s2)
/// For Falcon-512 (logn = 9), header = 0x39, total ≤ 1 + 40 + 625 = 666.
/// PQClean: `crypto_sign/falcon-512/clean/api.h`.
#[test]
fn pqclean_signature_header_and_length() {
    let (_pk, sk) = falcon512::keypair();
    let sig = falcon512::detached_sign(b"audit", &sk);
    let bytes = sig.as_bytes();

    assert!(
        bytes.len() <= FALCON_512_SIGNATURE_LEN,
        "PQClean sig len {} exceeds our buffer {}",
        bytes.len(),
        FALCON_512_SIGNATURE_LEN
    );
    assert_eq!(bytes[0], 0x39, "PQClean signature header");
}

/// Falcon spec §3.11.1:
///   pk = [0x00 | logn] || pack_14bit(h)
/// For Falcon-512 (logn = 9), header = 0x09, total = 1 + 896 = 897.
#[test]
fn pqclean_pubkey_header_and_length() {
    let (pk, _sk) = falcon512::keypair();
    let bytes = pk.as_bytes();

    assert_eq!(bytes.len(), FALCON_512_PUBKEY_LEN);
    assert_eq!(bytes[0], 0x09, "PQClean pubkey header");
}
