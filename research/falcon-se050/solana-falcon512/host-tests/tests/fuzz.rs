use pqcrypto_falcon::falcon512;
use pqcrypto_traits::sign::{DetachedSignature, PublicKey};
use solana_falcon512::{
    FALCON_512_PUBKEY_LEN, FALCON_512_SIGNATURE_LEN, Falcon512Pubkey, Falcon512Signature,
};

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn fill(&mut self, buf: &mut [u8]) {
        let mut i = 0;
        while i < buf.len() {
            let v = self.next().to_le_bytes();
            let n = (buf.len() - i).min(8);
            buf[i..i + n].copy_from_slice(&v[..n]);
            i += n;
        }
    }
    fn range(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn sign(msg: &[u8]) -> ([u8; FALCON_512_PUBKEY_LEN], [u8; FALCON_512_SIGNATURE_LEN]) {
    let (pk, sk) = falcon512::keypair();
    let sig = falcon512::detached_sign(msg, &sk);
    let mut pk_arr = [0u8; FALCON_512_PUBKEY_LEN];
    pk_arr.copy_from_slice(pk.as_bytes());
    let mut sig_arr = [0u8; FALCON_512_SIGNATURE_LEN];
    sig_arr[..sig.as_bytes().len()].copy_from_slice(sig.as_bytes());
    (pk_arr, sig_arr)
}

#[test]
fn fuzz_random_inputs_no_panic() {
    // Mix of random and header-valid inputs:
    //   - random headers exercise the early-reject path,
    //   - forced 0x09/0x39 headers drive past the header check into the
    //     real verify pipeline (NTT decode, hash-to-point, norm).
    // Without the forced-header iterations, ~99% of cases short-circuit
    // at the header byte and the verify path is barely exercised.
    let mut rng = Rng::new(0xDEAD_BEEF_DEAD_BEEF);
    for i in 0..2000 {
        let mut pk_bytes = [0u8; FALCON_512_PUBKEY_LEN];
        let mut sig_bytes = [0u8; FALCON_512_SIGNATURE_LEN];
        let mut msg = [0u8; 64];
        rng.fill(&mut pk_bytes);
        rng.fill(&mut sig_bytes);
        rng.fill(&mut msg);
        if i & 1 == 0 {
            pk_bytes[0] = 0x09;
            sig_bytes[0] = 0x39;
        }
        let pk = Falcon512Pubkey::from(pk_bytes);
        let sig = Falcon512Signature::from(sig_bytes);
        let _ = sig.verify(&msg, &pk);
    }
}

#[test]
fn fuzz_mutated_signature_rejects() {
    let msg = b"fuzz signature mutation";
    let (pk_bytes, sig_bytes) = sign(msg);
    let pubkey = Falcon512Pubkey::from(pk_bytes);
    assert!(Falcon512Signature::from(sig_bytes).verify(msg, &pubkey));

    let mut rng = Rng::new(0xCAFE_BABE_CAFE_BABE);
    for iter in 0..500 {
        let mut mutated = sig_bytes;
        let pos = rng.range(FALCON_512_SIGNATURE_LEN as u64) as usize;
        let bit = rng.range(8) as u8;
        mutated[pos] ^= 1 << bit;
        if mutated == sig_bytes {
            continue;
        }
        let bad = Falcon512Signature::from(mutated);
        assert!(
            !bad.verify(msg, &pubkey),
            "iter {iter}: sig mutation at byte {pos} bit {bit} accepted"
        );
    }
}

#[test]
fn fuzz_mutated_pubkey_rejects() {
    let msg = b"fuzz pubkey mutation";
    let (pk_bytes, sig_bytes) = sign(msg);

    let mut rng = Rng::new(0x1234_5678_9ABC_DEF0);
    for iter in 0..500 {
        let mut mutated_pk = pk_bytes;
        let pos = rng.range(FALCON_512_PUBKEY_LEN as u64) as usize;
        let bit = rng.range(8) as u8;
        mutated_pk[pos] ^= 1 << bit;
        if mutated_pk == pk_bytes {
            continue;
        }
        let pubkey = Falcon512Pubkey::from(mutated_pk);
        let signature = Falcon512Signature::from(sig_bytes);
        assert!(
            !signature.verify(msg, &pubkey),
            "iter {iter}: pubkey mutation at byte {pos} bit {bit} accepted"
        );
    }
}

#[test]
fn fuzz_mutated_message_rejects() {
    let original: &[u8] = b"original message long enough to mutate plenty of bits";
    let (pk_bytes, sig_bytes) = sign(original);
    let pubkey = Falcon512Pubkey::from(pk_bytes);
    let signature = Falcon512Signature::from(sig_bytes);

    let mut rng = Rng::new(0xBADF_00DE_BADF_00DE);
    for iter in 0..500 {
        let mut mutated = original.to_vec();
        let pos = rng.range(original.len() as u64) as usize;
        let bit = rng.range(8) as u8;
        mutated[pos] ^= 1 << bit;
        assert!(
            !signature.verify(&mutated, &pubkey),
            "iter {iter}: msg mutation at byte {pos} bit {bit} accepted"
        );
    }
}

#[test]
fn fuzz_random_message_with_valid_keypair() {
    // Valid pubkey/sig pair, but feed completely random messages — must reject.
    let (pk_bytes, sig_bytes) = sign(b"the real message");
    let pubkey = Falcon512Pubkey::from(pk_bytes);
    let signature = Falcon512Signature::from(sig_bytes);

    let mut rng = Rng::new(0xFACE_FEED_FACE_FEED);
    for iter in 0..500 {
        let mut buf = [0u8; 32];
        rng.fill(&mut buf);
        assert!(
            !signature.verify(&buf, &pubkey),
            "iter {iter}: random msg accepted with valid keypair/sig"
        );
    }
}
