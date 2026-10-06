//! Pure-Rust Falcon-512 signature **verification**, optimised for Solana SBF.
//!
//! Implements [FN-DSA / Falcon] signature verification (compressed-format
//! signatures only, header byte `0x39`). The crate is `no_std`, allocation
//! free, and all heavy work — pubkey decoding, NTT, SHAKE-256, signature
//! decompression — is hand-written. On Solana SBF a single verify costs
//! roughly **173k–183k compute units** with a prepared pubkey (the spread
//! is per-signature variance in `hash_to_point`'s SHAKE-256 rejection
//! sampling) or ~270k with a raw wire pubkey via
//! [`Falcon512Pubkey::prepare_pubkey`] / [`Falcon512Pubkey::try_prepare_pubkey`].
//!
//! [FN-DSA / Falcon]: https://falcon-sign.info
//!
//! # Quick start
//!
//! ```ignore
//! use solana_falcon512::{Falcon512Pubkey, Falcon512Signature};
//!
//! let pubkey = Falcon512Pubkey::try_from(&pk_bytes[..])?;
//! let signature = Falcon512Signature::try_from(&sig_bytes[..])?;
//! let ok = signature.verify(message, &pubkey);
//! ```
//!
//! For Solana programs with a hard-coded pubkey, prefer the prepared-pubkey
//! path — `prepare_pubkey()` is a `const fn`, so the NTT-form pubkey can be
//! embedded as a `const` and the per-call work is reduced by ~99k CUs:
//!
//! ```ignore
//! use solana_falcon512::{Falcon512Pubkey, Falcon512PreparedPubkey};
//!
//! const PREPARED: Falcon512PreparedPubkey =
//!     Falcon512Pubkey::from_bytes(*include_bytes!("../keys/falcon.pk"))
//!         .prepare_pubkey();
//!
//! let ok = signature.verify_with_prepared(message, &PREPARED);
//! ```
//!
//! # Compatibility
//!
//! - **Falcon-512 only.** Falcon-1024 is not supported.
//! - **Compressed-format signatures only** (header `0x39`). Padded (`0x49`)
//!   and CT-format signatures are rejected.
//! - **Verify only.** Key generation and signing are out of scope; produce
//!   keys and signatures with PQClean / `pqcrypto-falcon` or a hardware key.
//!
//! # Security notes
//!
//! - This crate is **not audited**. Use at your own risk for protecting
//!   anything of value.
//! - Verification operates on **public data only** (signature, pubkey,
//!   message). It is deliberately **not** constant-time — it short-circuits
//!   on header / length / decompression failures and on the running L2 norm
//!   exceeding the bound. That's safe for verify, since none of those leak
//!   secret information.
//! - The implementation has been cross-checked against:
//!   - the NIST FIPS 202 SHAKE-256 KATs (empty input, `"abc"`, multi-block);
//!   - 1,000,000 PQClean-generated signatures (zero failures);
//!   - 10,000 random-input fuzz iterations and 500-iter mutation tests for
//!     each of `(signature, pubkey, message)` (zero false accepts).

#![cfg_attr(not(test), no_std)]

use solana_program_error::ProgramError;

mod codec;
mod keccak;
mod ntt;

#[cfg(kani)]
#[path = "../internal-tests/kani_proofs.rs"]
mod kani_proofs;

#[cfg(test)]
#[path = "../internal-tests/adversarial_proptests.rs"]
mod adversarial_proptests;

#[cfg(test)]
#[path = "../internal-tests/spec_pins.rs"]
mod spec_pins;

/// Wire-encoded Falcon-512 pubkey length, including the 1-byte header.
pub const FALCON_512_PUBKEY_LEN: usize = 897;

/// Falcon-512 compressed-signature buffer length: 1 header byte + 40-byte
/// nonce + up to 625 bytes of Golomb-Rice-encoded `s2`. Signatures whose
/// encoded portion is shorter must zero-pad the trailing bytes.
pub const FALCON_512_SIGNATURE_LEN: usize = 666;

/// Serialised length of a [`Falcon512PreparedPubkey`]: 512 little-endian `u16`
/// coefficients = 1024 bytes. Each coefficient is `h_pk_NTT[i] · N_INV mod Q`
/// which is `< Q < 2^14`, so u16 is enough — halves the on-chain rent vs
/// storing as u32, and SBF `ldxh` costs the same as `ldxw` so there's no
/// CU penalty.
pub const FALCON_512_PREPARED_PUBKEY_LEN: usize = N * 2;

pub(crate) const N: usize = 512;
pub(crate) const Q: u32 = 12289;

const NONCE_LEN: usize = 40;
const L2_BOUND: u64 = 34_034_726;
const PUBKEY_HEADER: u8 = 0x09;
const SIG_HEADER: u8 = 0x39;

// Compile-time guards on the wire-encoding length constants:
// - Sig: 1 header + 40 nonce + 625 Golomb-Rice = 666 bytes.
// - Pubkey: 1 header + ceil(N * 14 / 8) = 1 + 896 = 897 bytes.
const _: () = assert!(FALCON_512_SIGNATURE_LEN == 1 + NONCE_LEN + 625);
const _: () = assert!(FALCON_512_PUBKEY_LEN == 1 + (N * 14) / 8);

// Norm-accumulation overflow safety + meaningfulness:
// - Worst-case `‖s1‖² + ‖s2‖²` per coefficient is `(Q/2)² + 2047²`; over N
//   slots the total fits u64.
// - L2_BOUND must be reachable (i.e., < the worst-case total) so the
//   verifier's `<= L2_BOUND` test isn't trivially true.
const _: () = assert!(((Q as u64 / 2) * (Q as u64 / 2) + 2047u64 * 2047) * (N as u64) < u64::MAX);
const _: () = assert!(L2_BOUND < ((Q as u64 / 2) * (Q as u64 / 2) + 2047u64 * 2047) * (N as u64));

/// Wire-encoded Falcon-512 public key (header byte `0x09` + 14-bit-packed
/// polynomial `h ∈ Z_q[x] / (x^512 + 1)`).
///
/// `#[repr(transparent)]` so a `&[u8; FALCON_512_PUBKEY_LEN]` can be
/// re-borrowed as a `&Falcon512Pubkey` without a copy via
/// [`Falcon512Pubkey::from_ref`].
#[derive(Clone, Eq, PartialEq)]
#[repr(transparent)]
pub struct Falcon512Pubkey([u8; FALCON_512_PUBKEY_LEN]);

impl TryFrom<&[u8]> for Falcon512Pubkey {
    type Error = ProgramError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let bytes: [u8; FALCON_512_PUBKEY_LEN] = value
            .try_into()
            .map_err(|_| ProgramError::InvalidArgument)?;
        Ok(bytes.into())
    }
}

impl From<[u8; FALCON_512_PUBKEY_LEN]> for Falcon512Pubkey {
    fn from(value: [u8; FALCON_512_PUBKEY_LEN]) -> Self {
        Self(value)
    }
}

impl Falcon512Pubkey {
    /// Borrow a `&[u8; FALCON_512_PUBKEY_LEN]` as a `&Falcon512Pubkey` with
    /// no copy. Useful when the pubkey bytes already live somewhere (e.g.
    /// a Solana account) and you want to avoid a 897-byte memcpy.
    pub const fn from_ref(bytes: &[u8; FALCON_512_PUBKEY_LEN]) -> &Self {
        // SAFETY: `Falcon512Pubkey` is `#[repr(transparent)]` over
        // `[u8; FALCON_512_PUBKEY_LEN]`. Both have the same layout,
        // alignment (1), and validity invariants, so the cast is sound.
        unsafe { &*(bytes as *const [u8; FALCON_512_PUBKEY_LEN] as *const Self) }
    }

    /// Borrow an arbitrary-length `&[u8]` as a `&Falcon512Pubkey`, returning
    /// `Err(InvalidArgument)` if the slice isn't exactly 897 bytes. Combines
    /// length check + [`from_ref`](Self::from_ref) into one safe call —
    /// pure references throughout, zero copies.
    pub fn try_from_slice(bytes: &[u8]) -> Result<&Self, ProgramError> {
        let array: &[u8; FALCON_512_PUBKEY_LEN] = bytes
            .try_into()
            .map_err(|_| ProgramError::InvalidArgument)?;
        Ok(Self::from_ref(array))
    }

    /// Wrap a 897-byte buffer as a pubkey without validation. The contents
    /// are validated lazily at verify time (`verify`) or eagerly when
    /// preparing the NTT form (`prepare_pubkey`).
    pub const fn from_bytes(value: [u8; FALCON_512_PUBKEY_LEN]) -> Self {
        Self(value)
    }

    /// Borrow the raw 897-byte wire encoding.
    pub const fn as_bytes(&self) -> &[u8; FALCON_512_PUBKEY_LEN] {
        &self.0
    }

    /// Decode the pubkey polynomial and run a forward NTT, returning a form
    /// that lets [`Falcon512Signature::verify_with_prepared`] skip the per-call
    /// decode and forward NTT (saves ~99k CUs on Solana SBF).
    ///
    /// `const fn`, so consumer programs can embed the prepared pubkey as a
    /// `const` and pay zero runtime setup cost.
    ///
    /// # Panics
    ///
    /// Panics if the pubkey is malformed (wrong header, an out-of-range
    /// coefficient, or non-zero trailing bits). When invoked in const
    /// context this becomes a compile-time error — exactly what you want
    /// if the pubkey is baked into the binary. For untrusted runtime
    /// pubkeys, prefer [`Self::try_prepare_pubkey`] which returns
    /// `Result<_, ProgramError>` instead of panicking, or
    /// [`Falcon512Signature::verify`] which never panics.
    pub const fn prepare_pubkey(&self) -> Falcon512PreparedPubkey {
        let bytes = &self.0;
        assert!(bytes[0] == PUBKEY_HEADER, "invalid pubkey header");

        let mut h = [0u32; N];
        let mut acc: u32 = 0;
        let mut acc_len: u32 = 0;
        let mut idx_in: usize = 1;
        let mut idx_out: usize = 0;
        while idx_out < N {
            acc = (acc << 8) | bytes[idx_in] as u32;
            idx_in += 1;
            acc_len += 8;
            if acc_len >= 14 {
                acc_len -= 14;
                let w = (acc >> acc_len) & 0x3FFF;
                assert!(w < Q, "invalid pubkey coefficient");
                h[idx_out] = w;
                idx_out += 1;
            }
        }
        assert!(
            (acc & ((1u32 << acc_len) - 1)) == 0,
            "non-zero trailing bits in pubkey",
        );

        ntt::ntt(&mut h);
        // Pre-multiply each NTT coefficient by N_INV. This folds the `1/N`
        // scaling that the inverse NTT would otherwise need at runtime
        // directly into the prepared pubkey — at compile time, free. After
        // this, the runtime path is `forward NTT(s2) * h_pk_NTT_scaled`
        // followed by an unscaled inverse NTT; the math works out since
        // inv_NTT(NTT(a) * NTT(b)) = (a*b) * N and we've already divided by
        // N inside h_pk. The result fits in u16 (each value < Q < 2^14),
        // so we narrow on the way out to halve the on-chain footprint.
        let n_inv = ntt::N_INV as u64;
        let q = Q as u64;
        let mut packed = [0u16; N];
        let mut k = 0;
        while k < N {
            packed[k] = (h[k] as u64 * n_inv % q) as u16;
            k += 1;
        }
        Falcon512PreparedPubkey(packed)
    }

    /// Runtime variant of [`Self::prepare_pubkey`] that surfaces malformed
    /// pubkeys as `Err(ProgramError::InvalidArgument)` instead of panicking.
    ///
    /// Use this when the pubkey comes from an untrusted source at runtime
    /// (a Solana account, an instruction argument, off-chain data, etc.).
    /// The work performed is identical to `prepare_pubkey` — same decode
    /// + forward NTT + `N_INV` fold — only the error path differs.
    ///
    /// `prepare_pubkey` (panicking, `const`) is preferred when the pubkey is
    /// known at compile time, since the work runs at build time and any
    /// failure becomes a compile error.
    pub fn try_prepare_pubkey(&self) -> Result<Falcon512PreparedPubkey, ProgramError> {
        let bytes = &self.0;
        if bytes[0] != PUBKEY_HEADER {
            return Err(ProgramError::InvalidArgument);
        }

        let mut h = [0u32; N];
        if !codec::decode_pubkey_u32(&bytes[1..], &mut h) {
            return Err(ProgramError::InvalidArgument);
        }

        ntt::ntt(&mut h);
        let n_inv = ntt::N_INV as u64;
        let q = Q as u64;
        let mut packed = [0u16; N];
        for (i, &slot) in h.iter().enumerate() {
            packed[i] = (slot as u64 * n_inv % q) as u16;
        }
        Ok(Falcon512PreparedPubkey(packed))
    }
}

impl TryFrom<Falcon512Pubkey> for Falcon512PreparedPubkey {
    type Error = ProgramError;

    /// Decode + forward-NTT the wire pubkey into the runtime "prepared" form,
    /// surfacing malformed input as `Err(InvalidArgument)`. Same work as
    /// [`Falcon512Pubkey::try_prepare_pubkey`].
    fn try_from(value: Falcon512Pubkey) -> Result<Self, Self::Error> {
        value.try_prepare_pubkey()
    }
}

impl TryFrom<&Falcon512Pubkey> for Falcon512PreparedPubkey {
    type Error = ProgramError;

    /// Borrowed-input version of [`TryFrom<Falcon512Pubkey>`]: useful when
    /// the wire pubkey already lives in account data and you don't want to
    /// move/copy it just to prepare.
    fn try_from(value: &Falcon512Pubkey) -> Result<Self, Self::Error> {
        value.try_prepare_pubkey()
    }
}

/// Pubkey polynomial decoded and pre-transformed into NTT (frequency) form,
/// ready to be multiplied with a signature's NTT polynomial during verify.
/// The `N_INV` scaling that the inverse NTT normally needs at the end is
/// also pre-folded in. Stored as `u16` since every coefficient is `< Q < 2^14`.
///
/// Construct from a [`Falcon512Pubkey`] via [`Falcon512Pubkey::prepare_pubkey`]
/// (which can run in `const` context), or from a 1024-byte serialised buffer
/// via [`Falcon512PreparedPubkey::from_bytes`] / [`as_bytes`](Self::as_bytes).
/// Storing the serialised form on-chain costs 1024 bytes of account data but
/// lets repeated verifies skip the ~99k-CU NTT prep on every call.
///
/// `#[repr(transparent)]` so a `&[u16; N]` can be re-borrowed as a
/// `&Falcon512PreparedPubkey` without a copy via
/// [`Falcon512PreparedPubkey::from_ref`].
#[derive(Clone, Eq, PartialEq)]
#[repr(transparent)]
pub struct Falcon512PreparedPubkey([u16; N]);

impl Falcon512PreparedPubkey {
    /// Borrow a `&[u8; FALCON_512_PREPARED_PUBKEY_LEN]` as a
    /// `&Falcon512PreparedPubkey` with no copy. Useful when the prepared
    /// pubkey lives in a Solana account: skips a 1024-byte memcpy that
    /// `from_bytes(*account_bytes)` would otherwise perform.
    ///
    /// # Safety
    ///
    /// `bytes` must be aligned to at least 2 bytes. The Solana program ABI
    /// guarantees account data is 8-byte aligned, so reading directly from
    /// `&account.data[offset..offset + LEN]` (with `offset` 2-byte aligned)
    /// satisfies this. If you slice into a misaligned position, prefer
    /// [`Falcon512PreparedPubkey::from_bytes`] which copies into an aligned
    /// stack buffer.
    pub const unsafe fn from_ref(bytes: &[u8; FALCON_512_PREPARED_PUBKEY_LEN]) -> &Self {
        // SAFETY: caller guarantees 2-byte alignment.
        // `Falcon512PreparedPubkey` is `#[repr(transparent)]` over
        // `[u16; N]`, which has the same size as `[u8; LEN]` (= N*2).
        unsafe { &*(bytes as *const [u8; FALCON_512_PREPARED_PUBKEY_LEN] as *const Self) }
    }

    /// Borrow an arbitrary-length `&[u8]` as a `&Falcon512PreparedPubkey`,
    /// checking length, 2-byte alignment, and coefficient range (`< Q`).
    /// Returns `Err(InvalidArgument)` if any check fails.
    ///
    /// This is the recommended entry point for loading a prepared pubkey
    /// from untrusted account data — it validates that every NTT
    /// coefficient is in `[0, Q)`, preventing wrong verification results
    /// from corrupted or attacker-controlled account bytes.
    ///
    /// Solana account data is 8-byte aligned per the program ABI, so reading
    /// from `&account.data[..1024]` always passes the alignment check (and
    /// any 2-byte-aligned offset into it does too). Use this in PDA loaders
    /// to skip the 1024-byte memcpy that
    /// [`from_bytes`](Self::from_bytes) would emit.
    pub fn try_from_slice(bytes: &[u8]) -> Result<&Self, ProgramError> {
        let array: &[u8; FALCON_512_PREPARED_PUBKEY_LEN] = bytes
            .try_into()
            .map_err(|_| ProgramError::InvalidArgument)?;
        // Alignment check — `from_ref` requires 2-byte alignment for the
        // u16 reinterpret.
        if !(array.as_ptr() as usize).is_multiple_of(core::mem::align_of::<u16>()) {
            return Err(ProgramError::InvalidArgument);
        }
        // SAFETY: length matches (`try_into` succeeded) and alignment
        // verified above.
        let pk = unsafe { Self::from_ref(array) };
        // Coefficient range check — every NTT coefficient must be < Q.
        // Without this, an attacker who controls the account data could
        // write out-of-range coefficients that cause verify_with_prepared
        // to produce wrong results.
        if !pk.validate() {
            return Err(ProgramError::InvalidArgument);
        }
        Ok(pk)
    }

    /// Reconstruct from a 1024-byte buffer produced by [`as_bytes`](Self::as_bytes).
    /// Coefficients are read as little-endian `u16`s. No validation is
    /// performed — the bytes are assumed to come from a trusted source
    /// (typically a Solana account previously written by your own program).
    ///
    /// If the source is untrusted (e.g. account data that another program
    /// or user could have written), call [`validate`](Self::validate)
    /// after construction to verify all coefficients are in range.
    pub const fn from_bytes(bytes: [u8; FALCON_512_PREPARED_PUBKEY_LEN]) -> Self {
        let mut h = [0u16; N];
        let mut i = 0;
        while i < N {
            h[i] = u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]);
            i += 1;
        }
        Self(h)
    }

    /// Check that every NTT coefficient is `< Q`. Returns `true` if all
    /// coefficients are in range, `false` if any coefficient is `>= Q`.
    ///
    /// A prepared pubkey produced by [`Falcon512Pubkey::prepare_pubkey`] is
    /// always valid. This method is needed only when loading a prepared
    /// pubkey from an untrusted source (e.g. a Solana account that another
    /// program or user could have written). Using an invalid prepared
    /// pubkey with [`Falcon512Signature::verify_with_prepared`] is not
    /// memory-unsafe, but can produce wrong verification results —
    /// accepting invalid signatures or rejecting valid ones.
    pub const fn validate(&self) -> bool {
        let mut i = 0;
        while i < N {
            if self.0[i] as u32 >= Q {
                return false;
            }
            i += 1;
        }
        true
    }

    /// Borrow the underlying `[u16; N]` storage as a `&[u8; LEN]` byte view
    /// suitable for writing to a Solana account. Zero-copy on little-endian
    /// targets (the only kind Rust supports for Solana SBF and all common
    /// hosts), since `[u16; N]` is laid out as little-endian u16 words in
    /// memory and that matches what `from_bytes` reads back via
    /// `u16::from_le_bytes`. Round-trips via [`from_bytes`](Self::from_bytes)
    /// or [`from_ref`](Self::from_ref).
    #[cfg(target_endian = "little")]
    pub const fn as_bytes(&self) -> &[u8; FALCON_512_PREPARED_PUBKEY_LEN] {
        // SAFETY: `Falcon512PreparedPubkey` is `#[repr(transparent)]` over
        // `[u16; N]`. `[u16; N]` and `[u8; 2*N]` have the same size; the
        // u16 storage has stricter alignment (2 bytes) than u8, so casting
        // *down* from u16 to u8 reference is sound. On little-endian
        // (compile-time-checked via `cfg(target_endian = "little")`) the
        // raw byte order matches `to_le_bytes` element-wise.
        unsafe { &*(self as *const Self as *const [u8; FALCON_512_PREPARED_PUBKEY_LEN]) }
    }

    /// Serialise to an owned 1024-byte buffer. Always available (works on
    /// big-endian hosts too) at the cost of an element-by-element byte-swap
    /// loop. Round-trips via [`from_bytes`](Self::from_bytes).
    #[cfg(not(target_endian = "little"))]
    pub fn as_bytes(&self) -> [u8; FALCON_512_PREPARED_PUBKEY_LEN] {
        let mut out = [0u8; FALCON_512_PREPARED_PUBKEY_LEN];
        for (i, &coeff) in self.0.iter().enumerate() {
            out[2 * i..2 * i + 2].copy_from_slice(&coeff.to_le_bytes());
        }
        out
    }
}

impl TryFrom<&[u8]> for Falcon512PreparedPubkey {
    type Error = ProgramError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let bytes: [u8; FALCON_512_PREPARED_PUBKEY_LEN] = value
            .try_into()
            .map_err(|_| ProgramError::InvalidArgument)?;
        Ok(Self::from_bytes(bytes))
    }
}

/// Wire-encoded compressed Falcon-512 signature (header `0x39` + 40-byte
/// nonce + Golomb-Rice-encoded `s2`, zero-padded to 666 bytes).
///
/// `#[repr(transparent)]` so a `&[u8; FALCON_512_SIGNATURE_LEN]` can be
/// re-borrowed as a `&Falcon512Signature` without a copy via
/// [`Falcon512Signature::from_ref`] — useful in Solana entrypoints to skip
/// the 666-byte memcpy that `Falcon512Signature::from(*sig_bytes)` would
/// otherwise emit (~200 CU saved).
#[derive(Clone, Eq, PartialEq)]
#[repr(transparent)]
pub struct Falcon512Signature([u8; FALCON_512_SIGNATURE_LEN]);

impl From<[u8; FALCON_512_SIGNATURE_LEN]> for Falcon512Signature {
    fn from(value: [u8; FALCON_512_SIGNATURE_LEN]) -> Self {
        Self(value)
    }
}

impl TryFrom<&[u8]> for Falcon512Signature {
    type Error = ProgramError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let bytes: [u8; FALCON_512_SIGNATURE_LEN] = value
            .try_into()
            .map_err(|_| ProgramError::InvalidArgument)?;
        Ok(bytes.into())
    }
}

impl Falcon512Signature {
    /// Wrap a 666-byte buffer as a signature without validation.
    pub const fn from_bytes(value: [u8; FALCON_512_SIGNATURE_LEN]) -> Self {
        Self(value)
    }

    /// Borrow a `&[u8; FALCON_512_SIGNATURE_LEN]` as a `&Falcon512Signature`
    /// with no copy. Equivalent to `Falcon512Signature::from(*bytes)` but
    /// without materialising the 666-byte struct on the caller's stack — for
    /// Solana entrypoints where the bytes already live in the runtime-provided
    /// input buffer, this skips a memcpy (~200 CU).
    pub const fn from_ref(bytes: &[u8; FALCON_512_SIGNATURE_LEN]) -> &Self {
        // SAFETY: `Falcon512Signature` is `#[repr(transparent)]` over
        // `[u8; FALCON_512_SIGNATURE_LEN]`, so a `&[u8; N]` and a
        // `&Falcon512Signature` have identical layout, alignment, and
        // validity invariants.
        unsafe { &*(bytes as *const [u8; FALCON_512_SIGNATURE_LEN] as *const Self) }
    }

    /// Borrow an arbitrary-length `&[u8]` as a `&Falcon512Signature`,
    /// returning `Err(InvalidArgument)` if the slice isn't exactly 666
    /// bytes. Combines length check + [`from_ref`](Self::from_ref) into one
    /// safe call — pure references throughout, zero copies.
    pub fn try_from_slice(bytes: &[u8]) -> Result<&Self, ProgramError> {
        let array: &[u8; FALCON_512_SIGNATURE_LEN] = bytes
            .try_into()
            .map_err(|_| ProgramError::InvalidArgument)?;
        Ok(Self::from_ref(array))
    }

    /// Borrow the raw 666-byte wire encoding.
    pub const fn as_bytes(&self) -> &[u8; FALCON_512_SIGNATURE_LEN] {
        &self.0
    }

    /// Verify against a prepared pubkey. Use this when the pubkey is a
    /// `const` (e.g. baked into a Solana program) to avoid the pubkey decode
    /// + forward NTT on every call.
    ///
    /// Returns `false` on any failure mode — wrong header byte, malformed
    /// signature compression, L2-norm bound exceeded, etc. — and never
    /// panics. Distinguishing the failure reason is intentionally unsupported
    /// since outside of debugging it usually doesn't matter.
    #[inline(never)]
    pub fn verify_with_prepared(&self, message: &[u8], prepared: &Falcon512PreparedPubkey) -> bool {
        let sig = &self.0;
        if sig[0] != SIG_HEADER {
            return false;
        }

        let nonce = &sig[1..1 + NONCE_LEN];
        let comp = &sig[1 + NONCE_LEN..];

        // Stack buffers are uninit rather than zero-initialised: both
        // `decompress_signature` and `hash_to_point` write every slot before
        // it's read, so the 1024-byte memset for each (~250 CU each on SBF)
        // is pure overhead. SAFETY justifications inline.
        use core::mem::MaybeUninit;
        let mut s2_buf = [MaybeUninit::<i16>::uninit(); N];
        // SAFETY: `decompress_signature` only writes to its `s2` argument
        // (via `for u in s2.iter_mut(); *u = ...`) and never reads from it.
        // After it returns `true`, every slot has been written, so we can
        // treat the buffer as initialised.
        let s2_ref: &mut [i16; N] = unsafe { &mut *(s2_buf.as_mut_ptr() as *mut [i16; N]) };
        if !codec::decompress_signature(comp, s2_ref) {
            return false;
        }

        let mut c_buf = [MaybeUninit::<u16>::uninit(); N];
        // SAFETY: `hash_to_point` writes every slot via the running
        // `c_p..c_end` pointer (no reads from `c`), and always returns with
        // `c_p == c_end` so all N slots are initialised on return.
        let c_ref: &mut [u16; N] = unsafe { &mut *(c_buf.as_mut_ptr() as *mut [u16; N]) };
        codec::hash_to_point(nonce, message, c_ref);

        norm_check_with_prepared(&prepared.0, s2_ref, c_ref)
    }

    /// Verify against a raw pubkey. Decodes and runs the forward NTT on every
    /// call — for hot paths with a static pubkey, prefer
    /// [`verify_with_prepared`](Self::verify_with_prepared).
    ///
    /// Returns `false` on any failure mode (wrong header, malformed pubkey
    /// or signature, norm bound exceeded). Never panics.
    #[inline(never)]
    pub fn verify(&self, message: &[u8], pubkey: &Falcon512Pubkey) -> bool {
        let sig = &self.0;
        let pk = &pubkey.0;

        if sig[0] != SIG_HEADER || pk[0] != PUBKEY_HEADER {
            return false;
        }

        let nonce = &sig[1..1 + NONCE_LEN];
        let comp = &sig[1 + NONCE_LEN..];

        let mut s2 = [0i16; N];
        if !codec::decompress_signature(comp, &mut s2) {
            return false;
        }

        let mut c = [0u16; N];
        codec::hash_to_point(nonce, message, &mut c);

        check_norm(&pk[1..], &s2, &c)
    }
}

#[inline(never)]
fn check_norm(pk_data: &[u8], s2: &[i16; N], c: &[u16; N]) -> bool {
    let mut h_ntt = [0u32; N];
    if !codec::decode_pubkey_u32(pk_data, &mut h_ntt) {
        return false;
    }
    ntt::ntt(&mut h_ntt);
    // Match the prepared-pubkey path: pre-fold N_INV into h_pk_NTT, narrow
    // to u16 (each value < Q < 2^14). See `prepare_pubkey` for the identity
    // that makes this correct.
    let n_inv = ntt::N_INV as u64;
    let q = Q as u64;
    let mut packed = [0u16; N];
    for (i, &slot) in h_ntt.iter().enumerate() {
        packed[i] = (slot as u64 * n_inv % q) as u16;
    }
    norm_check_with_prepared(&packed, s2, c)
}

#[inline(never)]
fn norm_check_with_prepared(h_pk_ntt: &[u16; N], s2: &[i16; N], c: &[u16; N]) -> bool {
    // Single working buffer that flows through three roles in sequence:
    //   1. NTT-main-levels output of s2     (after `ntt_main_levels_from_signed`)
    //   2. pointwise-mul + first-inv-level  (after `fused_last_fwd_mul_first_inv`)
    //   3. inv-NTT *up to but not including* the last level
    //      (after `inv_ntt_main_levels`)
    // The last inv-NTT level is then folded into the L2-norm accumulation
    // via `last_level_fused_norm`, so `buf`'s 512 final values never get
    // written-then-re-read (~2.5k CU saved over the unfused split).
    use core::mem::MaybeUninit;
    let mut buf_uninit = [MaybeUninit::<u32>::uninit(); N];
    // SAFETY: `ntt_main_levels_from_signed` writes every slot of `r`
    // before any subsequent reader sees it (the level-1 sgn_ct_bf
    // butterflies cover all N positions: r[j] and r[j + N/2] for
    // j ∈ [0, N/2)). The 2 KB memset-to-zero this avoids is pure overhead
    // since `buf` is never read before being fully overwritten.
    let buf: &mut [u32; N] = unsafe { &mut *(buf_uninit.as_mut_ptr() as *mut [u32; N]) };
    ntt::ntt_main_levels_from_signed(buf, s2);
    ntt::fused_last_fwd_mul_first_inv(buf, h_pk_ntt);
    ntt::inv_ntt_main_levels(buf);
    ntt::last_level_fused_norm(buf, c, s2, L2_BOUND)
}

#[cfg(test)]
#[path = "../internal-tests/miri_unsafe_paths.rs"]
mod miri_unsafe_paths;
