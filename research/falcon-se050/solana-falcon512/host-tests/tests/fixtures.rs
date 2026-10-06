//! One-shot fixture generator for deterministic CU benchmarking.
//!
//! Falcon signing is non-deterministic — each `detached_sign` call samples a
//! fresh 40-byte nonce, so signing at test time gives ±12k CU run-to-run
//! noise. We instead bake one fixed signature into the repo so the program's
//! Mollusk tests measure the same CU every run, making regressions visible.
//!
//! Regenerate (e.g. after rotating `falcon.sk`):
//!   cargo test --release -p host-tests --test fixtures -- --ignored --nocapture

use pqcrypto_falcon::falcon512;
use pqcrypto_traits::sign::{DetachedSignature, SecretKey};
use std::fs;
use std::path::PathBuf;

const SIG_LEN: usize = 666;
const MESSAGE: &[u8] = b"deterministic falcon-512 verify benchmark";

#[test]
#[ignore = "regenerates the static signature fixture"]
fn gen_sample_sig() {
    let dir: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("program/tests/fixtures");
    let sk_bytes = fs::read(dir.join("falcon.sk")).unwrap();
    let sk = falcon512::SecretKey::from_bytes(&sk_bytes).unwrap();

    let sig = falcon512::detached_sign(MESSAGE, &sk);
    let mut padded = [0u8; SIG_LEN];
    padded[..sig.as_bytes().len()].copy_from_slice(sig.as_bytes());

    let out = dir.join("sample_sig.bin");
    fs::write(&out, padded).unwrap();
    println!(
        "wrote {} bytes (msg = {:?}) to {}",
        padded.len(),
        std::str::from_utf8(MESSAGE).unwrap(),
        out.display()
    );
}
