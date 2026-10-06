use anyhow::{ensure, Context, Result};
use fn_dsa_comm::{
    sign_key_size, signature_size, vrfy_key_size, DOMAIN_NONE, FN_DSA_LOGN_512,
    HASH_ID_ORIGINAL_FALCON, HASH_ID_RAW,
};
use fn_dsa_kgen::{KeyPairGenerator, KeyPairGenerator512};
use fn_dsa_sign::{SigningKey, SigningKey512};
use fn_dsa_vrfy::{VerifyingKey, VerifyingKey512};
use rand_core::OsRng;
use sha2::{Digest, Sha256};
use solana_falcon512::{
    Falcon512PreparedPubkey, Falcon512Pubkey, Falcon512Signature, FALCON_512_PUBKEY_LEN,
    FALCON_512_SIGNATURE_LEN,
};

const LOGN: u32 = FN_DSA_LOGN_512;
const SIGNING_KEY_LEN: usize = sign_key_size(LOGN);
const VERIFYING_KEY_LEN: usize = vrfy_key_size(LOGN);
const SIGNATURE_LEN: usize = signature_size(LOGN);

fn main() -> Result<()> {
    println!("============================================================");
    println!("Vector / Falcon Interop Probe");
    println!("============================================================");

    let (signing_key, verifying_key) = generate_keypair()?;
    let identity = vector_identity(&verifying_key);
    let digest = demo_vector_digest(&identity);

    println!("Wire pubkey len: {VERIFYING_KEY_LEN} bytes");
    println!("Wire sig len:    {SIGNATURE_LEN} bytes");
    println!("Vector identity: {}", hex::encode(identity));
    println!("Demo digest:     {}", hex::encode(digest));

    let raw_sig = sign_message(&signing_key, &digest, false)?;
    ensure!(
        verify_fn_dsa(&verifying_key, &digest, &raw_sig, false)?,
        "HASH_ID_RAW self-verification failed"
    );
    let raw_under_solana = verify_solana_falcon(&verifying_key, &digest, &raw_sig)?;
    println!("HASH_ID_RAW -> solana-falcon512 verify: {raw_under_solana}");

    let orig_sig = sign_message(&signing_key, &digest, true)?;
    ensure!(
        verify_fn_dsa(&verifying_key, &digest, &orig_sig, true)?,
        "HASH_ID_ORIGINAL_FALCON self-verification failed"
    );
    let orig_under_solana = verify_solana_falcon(&verifying_key, &digest, &orig_sig)?;
    println!("HASH_ID_ORIGINAL_FALCON -> solana-falcon512 verify: {orig_under_solana}");

    ensure!(
        !raw_under_solana,
        "expected HASH_ID_RAW to fail solana-falcon512 verification"
    );
    ensure!(
        orig_under_solana,
        "expected HASH_ID_ORIGINAL_FALCON to verify under solana-falcon512"
    );

    println!();
    println!("Result: current hybrid mode is not Vector-compatible as-is.");
    println!("Result: original Falcon mode is compatible with solana-falcon512.");
    Ok(())
}

fn generate_keypair() -> Result<(Vec<u8>, Vec<u8>)> {
    let mut generator = KeyPairGenerator512::default();
    let mut signing_key = [0u8; SIGNING_KEY_LEN];
    let mut verifying_key = [0u8; VERIFYING_KEY_LEN];

    generator.keygen(LOGN, &mut OsRng, &mut signing_key, &mut verifying_key);

    Ok((signing_key.to_vec(), verifying_key.to_vec()))
}

fn sign_message(
    signing_key_bytes: &[u8],
    message: &[u8],
    original_falcon: bool,
) -> Result<Vec<u8>> {
    let mut signing_key = SigningKey512::decode(signing_key_bytes)
        .ok_or_else(|| anyhow::anyhow!("failed to decode signing key"))?;
    let mut signature = [0u8; SIGNATURE_LEN];
    let hash_id = if original_falcon {
        &HASH_ID_ORIGINAL_FALCON
    } else {
        &HASH_ID_RAW
    };

    signing_key.sign(&mut OsRng, &DOMAIN_NONE, hash_id, message, &mut signature);
    Ok(signature.to_vec())
}

fn verify_fn_dsa(
    verifying_key_bytes: &[u8],
    message: &[u8],
    signature: &[u8],
    original_falcon: bool,
) -> Result<bool> {
    let verifying_key = VerifyingKey512::decode(verifying_key_bytes)
        .ok_or_else(|| anyhow::anyhow!("failed to decode verifying key"))?;
    let hash_id = if original_falcon {
        &HASH_ID_ORIGINAL_FALCON
    } else {
        &HASH_ID_RAW
    };
    Ok(verifying_key.verify(signature, &DOMAIN_NONE, hash_id, message))
}

fn verify_solana_falcon(
    verifying_key_bytes: &[u8],
    digest: &[u8],
    signature: &[u8],
) -> Result<bool> {
    ensure!(
        verifying_key_bytes.len() == FALCON_512_PUBKEY_LEN,
        "unexpected Falcon pubkey length: {}",
        verifying_key_bytes.len()
    );
    ensure!(
        signature.len() == FALCON_512_SIGNATURE_LEN,
        "unexpected Falcon signature length: {}",
        signature.len()
    );

    let pubkey = Falcon512Pubkey::try_from_slice(verifying_key_bytes)
        .context("solana-falcon512 rejected the wire pubkey")?;
    let prepared: Falcon512PreparedPubkey = pubkey
        .try_prepare_pubkey()
        .context("solana-falcon512 failed to prepare the wire pubkey")?;
    let signature = Falcon512Signature::try_from_slice(signature)
        .context("solana-falcon512 rejected the wire signature")?;

    Ok(signature.verify_with_prepared(digest, &prepared))
}

fn vector_identity(verifying_key_bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(verifying_key_bytes).into()
}

fn demo_vector_digest(identity: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"vector-demo-v1");
    hasher.update(identity);
    hasher.finalize().into()
}
