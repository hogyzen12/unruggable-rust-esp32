use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::Aes256Gcm;
use anyhow::{anyhow, ensure, Result};
use pbkdf2::pbkdf2_hmac_array;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const ACCESS_BLOB_MAGIC: [u8; 4] = *b"TRPX";
pub const ACCESS_BLOB_VERSION_WRAPPED: u8 = 0x01;
pub const ACCESS_BLOB_VERSION_SE050_PROTECTED: u8 = 0x02;
pub const ACCESS_WRAP_KDF_ITERS: u32 = 10_000;
pub const ACCESS_WRAP_SALT_LEN: usize = 16;
pub const ACCESS_WRAP_NONCE_LEN: usize = 12;
pub const ACCESS_WRAP_TAG_LEN: usize = 16;
pub const ACCESS_BLOB_WRAPPED_LEN: usize =
    4 + 1 + 1 + ACCESS_WRAP_SALT_LEN + ACCESS_WRAP_NONCE_LEN + 32 + 32 + ACCESS_WRAP_TAG_LEN;
pub const ACCESS_BLOB_SE050_PROTECTED_LEN: usize = 4 + 1 + 1 + 32 + 32;

const ACCESS_WRAP_CONTEXT_LABEL: &[u8] = b"TROPIC_ACCESS_WRAP_V1";

#[derive(Clone, Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
pub enum TropicAccessSecret {
    Wrapped {
        wrap_salt: [u8; ACCESS_WRAP_SALT_LEN],
        nonce: [u8; ACCESS_WRAP_NONCE_LEN],
        encrypted_pairing_priv: [u8; 32],
        tag: [u8; ACCESS_WRAP_TAG_LEN],
    },
    Se050Protected {
        pairing_priv: [u8; 32],
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
pub struct TropicAccessBundle {
    pub pairing_key_index: u8,
    pub pairing_pub: [u8; 32],
    pub secret: TropicAccessSecret,
}

impl std::fmt::Debug for TropicAccessSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TropicAccessSecret([REDACTED])")
    }
}

impl TropicAccessBundle {
    pub fn new_wrapped(
        pairing_key_index: u8,
        wrap_salt: [u8; ACCESS_WRAP_SALT_LEN],
        nonce: [u8; ACCESS_WRAP_NONCE_LEN],
        pairing_pub: [u8; 32],
        encrypted_pairing_priv: [u8; 32],
        tag: [u8; ACCESS_WRAP_TAG_LEN],
    ) -> Self {
        Self {
            pairing_key_index,
            pairing_pub,
            secret: TropicAccessSecret::Wrapped {
                wrap_salt,
                nonce,
                encrypted_pairing_priv,
                tag,
            },
        }
    }

    pub fn new_se050_protected(
        pairing_key_index: u8,
        pairing_pub: [u8; 32],
        pairing_priv: [u8; 32],
    ) -> Self {
        Self {
            pairing_key_index,
            pairing_pub,
            secret: TropicAccessSecret::Se050Protected { pairing_priv },
        }
    }

    pub fn format_label(&self) -> &'static str {
        match self.secret {
            TropicAccessSecret::Wrapped { .. } => "wrapped-v1",
            TropicAccessSecret::Se050Protected { .. } => "se050-protected-v2",
        }
    }

}

pub fn build_wrap_context(
    pairing_key_index: u8,
    wrap_salt: &[u8; ACCESS_WRAP_SALT_LEN],
    pairing_pub: &[u8; 32],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        ACCESS_WRAP_CONTEXT_LABEL.len() + 1 + wrap_salt.len() + pairing_pub.len(),
    );
    out.extend_from_slice(ACCESS_WRAP_CONTEXT_LABEL);
    out.push(pairing_key_index);
    out.extend_from_slice(wrap_salt);
    out.extend_from_slice(pairing_pub);
    out
}

pub fn derive_wrap_key(
    pin: &str,
    wrap_salt: &[u8; ACCESS_WRAP_SALT_LEN],
    se050_signature: &[u8; 64],
    pairing_key_index: u8,
    pairing_pub: &[u8; 32],
) -> [u8; 32] {
    let pin_kdf = pbkdf2_hmac_array::<Sha256, 32>(pin.as_bytes(), wrap_salt, ACCESS_WRAP_KDF_ITERS);
    let mut hasher = Sha256::new();
    hasher.update(ACCESS_WRAP_CONTEXT_LABEL);
    hasher.update([pairing_key_index]);
    hasher.update(wrap_salt);
    hasher.update(pairing_pub);
    hasher.update(pin_kdf);
    hasher.update(se050_signature);

    let mut out = [0u8; 32];
    out.copy_from_slice(&hasher.finalize());
    out
}

// Only tests create legacy wrapped blobs; production retains decoding/migration.
#[cfg(test)]
fn encrypt_pairing_private_key(
    pairing_priv: &[u8; 32],
    wrap_key: &[u8; 32],
    nonce_bytes: &[u8; ACCESS_WRAP_NONCE_LEN],
) -> Result<([u8; 32], [u8; ACCESS_WRAP_TAG_LEN])> {
    let cipher = Aes256Gcm::new_from_slice(wrap_key).map_err(|e| anyhow!("{e:?}"))?;
    let nonce = aes_gcm::Nonce::from_slice(nonce_bytes);

    let mut ciphertext = *pairing_priv;
    let tag = cipher
        .encrypt_in_place_detached(nonce, b"", &mut ciphertext)
        .map_err(|e| anyhow!("{e:?}"))?;

    let mut tag_bytes = [0u8; ACCESS_WRAP_TAG_LEN];
    tag_bytes.copy_from_slice(&tag);
    Ok((ciphertext, tag_bytes))
}

pub fn decrypt_pairing_private_key(
    bundle: &TropicAccessBundle,
    wrap_key: &[u8; 32],
) -> Result<[u8; 32]> {
    let (nonce_bytes, encrypted_pairing_priv, tag_bytes) = match &bundle.secret {
        TropicAccessSecret::Wrapped {
            nonce,
            encrypted_pairing_priv,
            tag,
            ..
        } => (nonce, encrypted_pairing_priv, tag),
        TropicAccessSecret::Se050Protected { .. } => {
            return Err(anyhow!("ACCESS_BLOB_NOT_WRAPPED"));
        }
    };

    let cipher = Aes256Gcm::new_from_slice(wrap_key).map_err(|e| anyhow!("{e:?}"))?;
    let nonce = aes_gcm::Nonce::from_slice(nonce_bytes);
    let tag = aes_gcm::Tag::from_slice(tag_bytes);

    let mut pairing_priv = *encrypted_pairing_priv;
    cipher
        .decrypt_in_place_detached(nonce, b"", &mut pairing_priv, tag)
        .map_err(|e| anyhow!("{e:?}"))?;

    Ok(pairing_priv)
}

pub fn serialize_access_bundle(bundle: &TropicAccessBundle) -> Vec<u8> {
    match &bundle.secret {
        TropicAccessSecret::Wrapped {
            wrap_salt,
            nonce,
            encrypted_pairing_priv,
            tag,
        } => {
            let mut out = vec![0u8; ACCESS_BLOB_WRAPPED_LEN];
            out[0..4].copy_from_slice(&ACCESS_BLOB_MAGIC);
            out[4] = ACCESS_BLOB_VERSION_WRAPPED;
            out[5] = bundle.pairing_key_index;
            out[6..22].copy_from_slice(wrap_salt);
            out[22..34].copy_from_slice(nonce);
            out[34..66].copy_from_slice(&bundle.pairing_pub);
            out[66..98].copy_from_slice(encrypted_pairing_priv);
            out[98..114].copy_from_slice(tag);
            out
        }
        TropicAccessSecret::Se050Protected { pairing_priv } => {
            let mut out = vec![0u8; ACCESS_BLOB_SE050_PROTECTED_LEN];
            out[0..4].copy_from_slice(&ACCESS_BLOB_MAGIC);
            out[4] = ACCESS_BLOB_VERSION_SE050_PROTECTED;
            out[5] = bundle.pairing_key_index;
            out[6..38].copy_from_slice(&bundle.pairing_pub);
            out[38..70].copy_from_slice(pairing_priv);
            out
        }
    }
}

pub fn parse_access_bundle(blob: &[u8]) -> Result<TropicAccessBundle> {
    ensure!(blob.len() >= 6, "ACCESS_BLOB_BAD_LEN_{}", blob.len());
    ensure!(
        &blob[0..4] == ACCESS_BLOB_MAGIC.as_slice(),
        "ACCESS_BLOB_BAD_MAGIC"
    );
    ensure!(blob[5] <= 3, "ACCESS_BLOB_BAD_PAIRING_SLOT_{}", blob[5]);

    match blob[4] {
        ACCESS_BLOB_VERSION_WRAPPED => {
            ensure!(
                blob.len() == ACCESS_BLOB_WRAPPED_LEN,
                "ACCESS_BLOB_BAD_LEN_{}",
                blob.len()
            );

            let mut wrap_salt = [0u8; ACCESS_WRAP_SALT_LEN];
            wrap_salt.copy_from_slice(&blob[6..22]);

            let mut nonce = [0u8; ACCESS_WRAP_NONCE_LEN];
            nonce.copy_from_slice(&blob[22..34]);

            let mut pairing_pub = [0u8; 32];
            pairing_pub.copy_from_slice(&blob[34..66]);

            let mut encrypted_pairing_priv = [0u8; 32];
            encrypted_pairing_priv.copy_from_slice(&blob[66..98]);

            let mut tag = [0u8; ACCESS_WRAP_TAG_LEN];
            tag.copy_from_slice(&blob[98..114]);

            Ok(TropicAccessBundle::new_wrapped(
                blob[5],
                wrap_salt,
                nonce,
                pairing_pub,
                encrypted_pairing_priv,
                tag,
            ))
        }
        ACCESS_BLOB_VERSION_SE050_PROTECTED => {
            ensure!(
                blob.len() == ACCESS_BLOB_SE050_PROTECTED_LEN,
                "ACCESS_BLOB_BAD_LEN_{}",
                blob.len()
            );

            let mut pairing_pub = [0u8; 32];
            pairing_pub.copy_from_slice(&blob[6..38]);

            let mut pairing_priv = [0u8; 32];
            pairing_priv.copy_from_slice(&blob[38..70]);

            Ok(TropicAccessBundle::new_se050_protected(
                blob[5],
                pairing_pub,
                pairing_priv,
            ))
        }
        other => Err(anyhow!("ACCESS_BLOB_BAD_VERSION_{other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_signature() -> [u8; 64] {
        let mut sig = [0u8; 64];
        for (idx, byte) in sig.iter_mut().enumerate() {
            *byte = (idx as u8).wrapping_mul(3).wrapping_add(7);
        }
        sig
    }

    #[test]
    fn wrap_context_is_stable() {
        let wrap_salt = [0x11; ACCESS_WRAP_SALT_LEN];
        let pairing_pub = [0x22; 32];
        let ctx = build_wrap_context(2, &wrap_salt, &pairing_pub);
        assert!(ctx.starts_with(ACCESS_WRAP_CONTEXT_LABEL));
        assert_eq!(ctx[ACCESS_WRAP_CONTEXT_LABEL.len()], 2);
        assert_eq!(
            &ctx[ACCESS_WRAP_CONTEXT_LABEL.len() + 1..ACCESS_WRAP_CONTEXT_LABEL.len() + 17],
            &wrap_salt
        );
        assert_eq!(&ctx[ctx.len() - 32..], &pairing_pub);
    }

    #[test]
    fn serialize_round_trip() {
        let bundle = TropicAccessBundle::new_wrapped(
            1,
            [0x10; ACCESS_WRAP_SALT_LEN],
            [0x20; ACCESS_WRAP_NONCE_LEN],
            [0x30; 32],
            [0x40; 32],
            [0x50; ACCESS_WRAP_TAG_LEN],
        );

        let blob = serialize_access_bundle(&bundle);
        let parsed = parse_access_bundle(&blob).expect("parse bundle");
        assert_eq!(parsed, bundle);
    }

    #[test]
    fn serialize_round_trip_se050_protected() {
        let bundle = TropicAccessBundle::new_se050_protected(2, [0x61; 32], [0x71; 32]);

        let blob = serialize_access_bundle(&bundle);
        let parsed = parse_access_bundle(&blob).expect("parse bundle");
        assert_eq!(parsed, bundle);
    }

    #[test]
    fn derive_wrap_key_changes_with_pin() {
        let wrap_salt = [0xAA; ACCESS_WRAP_SALT_LEN];
        let pairing_pub = [0xBB; 32];
        let sig = sample_signature();

        let key_a = derive_wrap_key("123456", &wrap_salt, &sig, 1, &pairing_pub);
        let key_b = derive_wrap_key("654321", &wrap_salt, &sig, 1, &pairing_pub);
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn encrypt_decrypt_round_trip() {
        let pairing_priv = [0x5C; 32];
        let wrap_salt = [0x12; ACCESS_WRAP_SALT_LEN];
        let pairing_pub = [0x34; 32];
        let nonce = [0x56; ACCESS_WRAP_NONCE_LEN];
        let sig = sample_signature();
        let wrap_key = derive_wrap_key("246810", &wrap_salt, &sig, 3, &pairing_pub);
        let (ciphertext, tag) =
            encrypt_pairing_private_key(&pairing_priv, &wrap_key, &nonce).expect("encrypt");

        let bundle =
            TropicAccessBundle::new_wrapped(3, wrap_salt, nonce, pairing_pub, ciphertext, tag);

        let decrypted = decrypt_pairing_private_key(&bundle, &wrap_key).expect("decrypt");
        assert_eq!(decrypted, pairing_priv);
    }

    #[test]
    fn decrypt_rejects_tampered_ciphertext() {
        let pairing_priv = [0x7C; 32];
        let wrap_salt = [0x21; ACCESS_WRAP_SALT_LEN];
        let pairing_pub = [0x43; 32];
        let nonce = [0x65; ACCESS_WRAP_NONCE_LEN];
        let sig = sample_signature();
        let wrap_key = derive_wrap_key("111111", &wrap_salt, &sig, 2, &pairing_pub);
        let (ciphertext, tag) =
            encrypt_pairing_private_key(&pairing_priv, &wrap_key, &nonce).expect("encrypt");

        let mut bundle =
            TropicAccessBundle::new_wrapped(2, wrap_salt, nonce, pairing_pub, ciphertext, tag);
        match &mut bundle.secret {
            TropicAccessSecret::Wrapped {
                encrypted_pairing_priv,
                ..
            } => encrypted_pairing_priv[0] ^= 0xFF,
            TropicAccessSecret::Se050Protected { .. } => panic!("expected wrapped secret"),
        }

        let err = decrypt_pairing_private_key(&bundle, &wrap_key).expect_err("tamper must fail");
        assert!(format!("{err}").contains("Error"));
    }
}
