# solana-falcon512

Pure-Rust **Falcon-512 signature verification**, optimised for Solana SBF programs.

- `no_std`, allocation-free, zero non-essential dependencies.
- Compressed-format signatures (header byte `0x39`) only.
- **~173–183k compute units per verify** on Solana SBF with a prepared pubkey
  (rejection sampling is not constant-time — see [Benchmarks](#benchmarks)).
- Zero-copy borrow APIs (`from_ref`, `try_from_slice`) so signatures and prepared pubkeys can be verified directly from runtime input / account data with no memcpy.
- Prepared pubkey storage: **1024 bytes** (one u16 per NTT coefficient, since each value is `< Q < 2^14`).
- Cross-checked against NIST SHAKE-256 KATs and 1,000,000 PQClean-generated signatures with zero failures.

## Usage

### Runtime pubkey

```rust
use solana_falcon512::{Falcon512Pubkey, Falcon512Signature};

let pubkey = Falcon512Pubkey::try_from(&pk_bytes[..])?;
let signature = Falcon512Signature::try_from(&sig_bytes[..])?;
let ok = signature.verify(message, &pubkey);
```

### Compile-time prepared pubkey (recommended for Solana programs)

If your program embeds a fixed pubkey, prepare it at compile time. `prepare_pubkey()` is a `const fn` — the decoded NTT-form pubkey is baked into the binary, saving ~99k CUs on every verify:

```rust
use solana_falcon512::{Falcon512Pubkey, Falcon512PreparedPubkey, Falcon512Signature};

const PREPARED: Falcon512PreparedPubkey =
    Falcon512Pubkey::from_bytes(*include_bytes!("../keys/falcon.pk"))
        .prepare_pubkey();

let ok = signature.verify_with_prepared(message, &PREPARED);
```

A malformed pubkey will fail at compile time rather than panic at runtime.

### Runtime prepared pubkey (multi-tenant programs)

If your program verifies against a different pubkey per account/user — i.e.
the pubkey isn't known at compile time — store the **prepared** form on-chain
instead of the raw 897-byte wire encoding. Each verify then loads the NTT-form
pubkey directly and skips the ~99k-CU decode + forward NTT.

The trade-off is 1024 bytes of account data instead of 897 bytes (about
127 bytes extra rent per account). Anything that gets verified multiple times
recoups that cost easily in saved compute.

For runtime preparation, `prepare_pubkey()`'s `const` form would `panic!` on a
malformed wire pubkey — fine at compile time (becomes a build error), but
unhelpful at runtime where you want to surface the error and reject the
instruction. Use `try_prepare_pubkey()` (or the equivalent
`TryFrom<Falcon512Pubkey>` / `TryFrom<&Falcon512Pubkey>` impls) for the
runtime path — it returns `Result<Falcon512PreparedPubkey, ProgramError>`
with `Err(InvalidArgument)` on header / coefficient / trailing-bit issues.

```rust
use solana_falcon512::{
    Falcon512PreparedPubkey, Falcon512Pubkey, Falcon512Signature,
    FALCON_512_PREPARED_PUBKEY_LEN,
};

// On registration: prepare once, write the 1024-byte form into the account.
// Either of these forms works:
let pk = Falcon512Pubkey::try_from(&pk_wire_bytes[..])?;
let prepared: Falcon512PreparedPubkey = (&pk).try_into()?;        // TryFrom
// or, equivalently:
let prepared = pk.try_prepare_pubkey()?;                            // method
account_data.copy_from_slice(prepared.as_bytes());

// On verify: borrow the prepared pubkey directly out of account data — no
// copy, no allocation. `try_from_slice` validates length + 2-byte alignment
// (Solana account data is 8-byte aligned by ABI, so this always passes).
let prepared = Falcon512PreparedPubkey::try_from_slice(&account_data[..])?;
let signature = Falcon512Signature::try_from_slice(sig_bytes)?;
let ok = signature.verify_with_prepared(message, prepared);
```

### Zero-copy borrow APIs

All three byte-array wrappers are `#[repr(transparent)]` and expose
zero-copy borrow constructors:

```rust
// From a fixed-size array reference (no length check, no copy):
let sig: &Falcon512Signature = Falcon512Signature::from_ref(&sig_bytes_array);
let pk:  &Falcon512Pubkey    = Falcon512Pubkey::from_ref(&pk_bytes_array);
let prepared: &Falcon512PreparedPubkey =
    unsafe { Falcon512PreparedPubkey::from_ref(&pp_bytes_array) }; // 2-byte aligned

// From a slice (length checked + alignment checked, still no copy):
let sig      = Falcon512Signature::try_from_slice(&sig_slice)?;
let pk       = Falcon512Pubkey::try_from_slice(&pk_slice)?;
let prepared = Falcon512PreparedPubkey::try_from_slice(&pp_slice)?;
```

The Solana entrypoint pattern below avoids the 666-byte signature memcpy and
the 1024-byte prepared-pubkey memcpy that owned-array constructors would
emit — the verify operates directly on the runtime-provided buffers:

```rust
let signature = Falcon512Signature::from_ref(sig_bytes); // borrowed from input
signature.verify_with_prepared(message, &PREPARED_PUBKEY)
```

## Compatibility

| Variant                                | Supported |
| -------------------------------------- | --------- |
| Falcon-512 compressed (`0x39`)         | ✅        |
| Falcon-512 padded (`0x49`)             | ❌        |
| Falcon-512 CT format                   | ❌        |
| Falcon-1024                            | ❌        |
| Sign / keygen                          | ❌ (verify only — generate keys with PQClean / `pqcrypto-falcon`) |

### Prepared pubkey: a blockchain-specific optimisation

A standard Falcon verifier sees each pubkey once and discards it — there's
no point caching intermediate state. On a blockchain, the same pubkey is
verified against many signatures over its lifetime and you pay rent for
storage, so the economics flip: it pays to bake the deterministic-from-`h`
work into account data and skip it on every verify.

Three transformations baked into `Falcon512PreparedPubkey`:

1. **Decode + forward NTT** of `h`, eliminating the per-verify 14-bit
   unpacking and 9-level NTT (~95k CUs saved per verify).
2. **Pre-fold `N⁻¹ mod q`** into each coefficient. The inverse NTT
   normally ends with a `× (1/N)` scalar pass; pre-folding lets the
   runtime skip it. `inv_NTT(prepared · NTT(s2)) = (h · s2 · N) · N⁻¹ =
   h · s2`.
3. **Narrow `u32 → u16`**. NTT coefficients are `< q < 2^14`, so u16 is
   enough — halves on-chain rent (1024 bytes vs 2048) at zero CU cost
   (SBF `ldxh` costs the same as `ldxw`).

The wire format (1024-byte little-endian `[u16; 512]`) is **specific to
this crate**: other Falcon implementations' NTT-form pubkeys store
`NTT(h)` without `N⁻¹` and as u32, so they won't round-trip. That's
intentional. The 897-byte standard wire pubkey is still the
interoperability boundary — accept it via [`Falcon512Pubkey::try_from`],
prepare once with [`Falcon512Pubkey::try_prepare_pubkey`] (or the `const`
panicking [`Falcon512Pubkey::prepare_pubkey`] for compile-time keys), and
store the result. The 127-byte rent overhead (1024 vs 897) amortises over
many verifications throughout the lifetime of the pubkey.

## Benchmarks

Measured via Mollusk SVM, default optimised build (`lto = "fat"`,
`opt-level = 3`, `codegen-units = 1`), with the entrypoint borrowing the
signature in place via `Falcon512Signature::from_ref`:

| Path                                 | CUs            |
| ------------------------------------ | -------------- |
| `verify_with_prepared` (success)     | ~173k–183k     |
| `verify_with_prepared` (rejection)   | ~173k–183k     |
| `verify` (raw pubkey)                | ~270k          |

The CU range for `verify_with_prepared` reflects per-signature variance
in `hash_to_point`: each Falcon signature embeds a fresh random nonce, and
SHAKE-256 rejection sampling in `hash_to_point` consumes a variable number
of permutations depending on how many `< 5q` candidates land in each
absorbed block (typically 8–10 permutations, ~95% of the variance). The
algorithm work is identical; only the keccak count differs. A safe
compute-unit budget for the prepared path is
`set_compute_unit_limit(195_000)`.

Notable points along the optimisation curve (start of the journey vs. now,
`verify_with_prepared`):

| Stage                                                                        | CUs      |
| ---------------------------------------------------------------------------- | -------- |
| Naive port                                                                   | ~283k    |
| u64-throughout NTT butterflies                                               | 265k     |
| `assert_unchecked` for SBF bounds-check elision                              | 248k     |
| Lazy-reduction (drop intermediate `% q` where the next mul·zeta·% q absorbs) | 222k     |
| Fuse inv-NTT last level with L2-norm loop                                    | 213k     |
| Pre-fold `N_INV` into prepared pubkey                                        | 209k     |
| Lazy-`t` at last forward NTT level                                           | 196k     |
| Zero-copy `from_ref` for signature in entrypoint                             | 196k     |
| chi-row + chi-row-iota Keccak layout                                         | 195k     |
| Bertoni 6-lane lane-complementing                                            | 187k     |
| In-place chi-row + 10 cell-saves (no `B[25]` scratch)                        | 183k     |
| `#[inline(always)]` on hot helpers (`ntt_levels_after_first`, etc.)          | **183k** |

## Project layout

- `src/` — verify library (`no_std`, rlib only). Zero non-essential deps.
- `host-tests/` — separate crate hosting the integration tests
  (e2e / fuzz / soak) so the main crate doesn't pull in `pqcrypto-falcon`,
  `rayon`, etc. as dev-deps.
- `program/` — minimal Solana program demonstrating in-program verification
  with a baked-in prepared pubkey.
- `program/tests/` — Mollusk SBF tests against the actual `.so`.

## Testing

```sh
cargo test --workspace                              # lib unit + e2e + fuzz
cargo test --workspace --release -- --ignored       # 100k-iter soak (~40s),
                                                    # plus PQClean differential and
                                                    # 10M-iter random-rejection soak
(cd program && cargo test-sbf)                      # SBF tests via Mollusk
```

The e2e tests include a `prepared_pubkey_roundtrip_matches_direct_verify`
case that prepares a pubkey at runtime, serialises via `as_bytes`,
deserialises via `from_bytes`, and confirms `verify_with_prepared` agrees
with direct `verify` for valid signatures and rejects for tampered ones.

### Regenerating the example keypair

The Solana program in `program/` embeds a Falcon-512 keypair at
`program/tests/fixtures/falcon.{pk,sk}` for its compile-time prepared pubkey
and Mollusk tests. To regenerate:

```rust
use pqcrypto_falcon::falcon512;
use pqcrypto_traits::sign::{PublicKey, SecretKey};

let (pk, sk) = falcon512::keypair();
std::fs::write("program/tests/fixtures/falcon.pk", pk.as_bytes()).unwrap();
std::fs::write("program/tests/fixtures/falcon.sk", sk.as_bytes()).unwrap();
```

## Security

**This crate is not audited.** Use at your own risk for protecting anything of value.

Verification operates exclusively on public data (signature, pubkey, message), so the implementation is deliberately not constant-time — it short-circuits on header / length / decompression failures and on the running L2 norm exceeding the bound. None of those leak secret information.

For the underlying cryptography see [Falcon][falcon] and [NIST FN-DSA / FIPS 206][fndsa].

[falcon]: https://falcon-sign.info
[fndsa]: https://csrc.nist.gov/pubs/fips/206/ipd

### Common footguns

#### 1. Wire-format validity does not constitute a valid pubkey

`Falcon512Pubkey::prepare_pubkey` only validates the wire format (header byte, 14-bit packed coefficients within `[0, q)`, no trailing bits). Cryptographically confirming that a pubkey `h` came from real key generation — i.e. that `h = g · f⁻¹ mod q` for some short `(f, g)` trapdoor — is impossible without the trapdoor itself. Thus, a user can submit any 897-byte buffer that parses, even one with no corresponding secret key. If this is important for your use case, consider verifying that a key is legitimate with a signature challenge.

#### 2. Signatures are non-deterministic and non-unique

Falcon signing samples a fresh 40-byte nonce per signature, so `sign(sk, msg)` returns a *different* 666-byte signature every time. The same `(msg, pk)` has many valid signatures. As such, just as is the case with other signature schemes, it is important not to ever dedup by signature value.

#### 3. No built-in domain separation

`hash_to_point` digests `SHAKE256(nonce ‖ message)` with no extra prefix. If the same Falcon keypair is used across two protocols (e.g. authentication and payments), a signature produced for protocol A may be replayable in protocol B if their message formats happen to overlap. Consider using domain separation in your message to avoid this issue.

#### 4. Compressed format only

This crate only accepts the standard compressed signature (header `0x39`) format of Falcon512. Padded (`0x49`) and CT-format signatures are deemed invalid.

#### 5. Solana transaction size

A raw Falcon-512 verification touches ~666 bytes of signature + ~897 bytes of pubkey, exceeding Solana's **1232-byte legacy-transaction limit**. Consider storing a prepared pubkey in a PDA (1024 bytes — fits within the typical PDA size budget) to get around this limitation: only the 666-byte signature then needs to come in via the instruction.

#### 6. Security level

Falcon-512 targets NIST PQC level 1: ~128-bit classical / ~117-bit post-quantum security which is suitable for short-to-medium-lifetime authorisation. Consider periodically rotating keys.

## Warranty

**None.** This software is provided strictly "AS IS", without warranty of any kind, express or implied — including but not limited to warranties of merchantability, fitness for a particular purpose, correctness, security, or non-infringement. The authors and contributors accept no liability for any loss, damage, or unintended consequence arising from its use.

In plain English: if you deploy this and lose funds, that is **entirely your problem.** Audit it, test it against your threat model, and budget for the possibility that it's wrong.

## License

MIT