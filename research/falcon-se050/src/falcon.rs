use anyhow::{anyhow, ensure, Result};
use fn_dsa_comm::{
    sign_key_size, signature_size, vrfy_key_size, DOMAIN_NONE, FN_DSA_LOGN_512,
    HASH_ID_ORIGINAL_FALCON, HASH_ID_RAW,
};
use fn_dsa_kgen::{KeyPairGenerator, KeyPairGenerator512};
use fn_dsa_sign::{SigningKey, SigningKey512};
use fn_dsa_vrfy::{VerifyingKey, VerifyingKey512};
use rand_core::OsRng;
use sha2::{Digest, Sha256};

pub const PQC_LOGN: u32 = FN_DSA_LOGN_512;
const SIGNING_KEY_LEN: usize = sign_key_size(PQC_LOGN);
const VERIFYING_KEY_LEN: usize = vrfy_key_size(PQC_LOGN);
const SIGNATURE_LEN: usize = signature_size(PQC_LOGN);
const BINDING_TAG: &[u8] = b"unruggable-bind-v2";
const HYBRID_TX_TAG: &[u8] = b"unruggable-hybrid-solana-tx-v1";
const WRAP_KEY_TAG: &[u8] = b"unruggable-pqc-wrap-v2";

pub struct KeyMaterial {
    pub signing_key: Vec<u8>,
    pub verifying_key: Vec<u8>,
}

pub fn signing_key_len() -> usize {
    SIGNING_KEY_LEN
}

pub fn verifying_key_len() -> usize {
    VERIFYING_KEY_LEN
}

pub fn generate_keypair() -> Result<KeyMaterial> {
    let mut generator = KeyPairGenerator512::default();
    let mut signing_key = [0u8; SIGNING_KEY_LEN];
    let mut verifying_key = [0u8; VERIFYING_KEY_LEN];

    generator.keygen(PQC_LOGN, &mut OsRng, &mut signing_key, &mut verifying_key);

    ensure!(
        signing_key.len() == signing_key_len(),
        "unexpected FN-DSA signing key length"
    );
    ensure!(
        verifying_key.len() == verifying_key_len(),
        "unexpected FN-DSA verifying key length"
    );

    Ok(KeyMaterial {
        signing_key: signing_key.to_vec(),
        verifying_key: verifying_key.to_vec(),
    })
}

pub fn sign_message(signing_key_bytes: &[u8], message: &[u8]) -> Result<Vec<u8>> {
    let mut signing_key = SigningKey512::decode(signing_key_bytes)
        .ok_or_else(|| anyhow!("failed to decode FN-DSA signing key"))?;
    let mut signature = [0u8; SIGNATURE_LEN];

    signing_key.sign(
        &mut OsRng,
        &DOMAIN_NONE,
        &HASH_ID_RAW,
        message,
        &mut signature,
    );
    Ok(signature.to_vec())
}

pub fn verify_message(verifying_key_bytes: &[u8], message: &[u8], signature: &[u8]) -> Result<()> {
    let verifying_key = VerifyingKey512::decode(verifying_key_bytes)
        .ok_or_else(|| anyhow!("failed to decode FN-DSA verifying key"))?;
    ensure!(
        verifying_key.verify(signature, &DOMAIN_NONE, &HASH_ID_RAW, message),
        "FN-DSA verification failed"
    );
    Ok(())
}

pub fn sign_message_original_falcon(signing_key_bytes: &[u8], message: &[u8]) -> Result<Vec<u8>> {
    let mut signing_key = SigningKey512::decode(signing_key_bytes)
        .ok_or_else(|| anyhow!("failed to decode FN-DSA signing key"))?;
    let mut signature = [0u8; SIGNATURE_LEN];

    signing_key.sign(
        &mut OsRng,
        &DOMAIN_NONE,
        &HASH_ID_ORIGINAL_FALCON,
        message,
        &mut signature,
    );
    Ok(signature.to_vec())
}

pub fn vector_identity(falcon_pubkey: &[u8]) -> [u8; 32] {
    Sha256::digest(falcon_pubkey).into()
}

pub fn build_binding_message(se050_pubkey: &[u8; 32], falcon_pubkey: &[u8]) -> Vec<u8> {
    let pubkey_hash = vector_identity(falcon_pubkey);
    let mut message =
        Vec::with_capacity(BINDING_TAG.len() + 1 + se050_pubkey.len() + 4 + pubkey_hash.len());
    message.extend_from_slice(BINDING_TAG);
    message.push(0);
    message.extend_from_slice(se050_pubkey);
    append_len_prefixed(&mut message, &pubkey_hash);
    message
}

pub fn build_hybrid_message(se050_pubkey: &[u8; 32], solana_message: &[u8]) -> Vec<u8> {
    let mut message =
        Vec::with_capacity(HYBRID_TX_TAG.len() + 1 + se050_pubkey.len() + 4 + solana_message.len());
    message.extend_from_slice(HYBRID_TX_TAG);
    message.push(0);
    message.extend_from_slice(se050_pubkey);
    append_len_prefixed(&mut message, solana_message);
    message
}

pub fn build_wrap_key_message(se050_pubkey: &[u8; 32]) -> Vec<u8> {
    let mut message = Vec::with_capacity(WRAP_KEY_TAG.len() + 1 + se050_pubkey.len());
    message.extend_from_slice(WRAP_KEY_TAG);
    message.push(0);
    message.extend_from_slice(se050_pubkey);
    message
}

fn append_len_prefixed(buf: &mut Vec<u8>, data: &[u8]) {
    let len = u32::try_from(data.len()).unwrap_or(u32::MAX);
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(data);
}
