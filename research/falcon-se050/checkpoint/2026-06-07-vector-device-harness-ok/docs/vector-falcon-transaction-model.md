# Vector / Falcon Solana Model

## What "Falcon in a Solana tx" means

Solana does not accept Falcon signatures in the native transaction
`signatures[]` array. A "Solana tx with Falcon in it" means:

1. The submitted transaction is still a normal Solana transaction.
2. Falcon proof bytes live inside a custom instruction payload.
3. An on-chain program recomputes the canonical digest and verifies the
   Falcon signature during execution.

The `vector` repo is an example of this pattern. Its Falcon program stores
the Falcon public key identity on-chain, recomputes the
`advance_vector_digest` from the instructions sysvar, and verifies the
wire-format Falcon-512 signature with `solana-falcon512`.

## How This Maps To This Hardware

- The SE050 remains Ed25519-only.
- The ESP32 performs Falcon-512 signing in software.
- The host app builds the transaction, computes the 32-byte Vector digest,
  asks the ESP32 to sign that digest, inserts the Falcon signature into the
  Vector `advance` instruction, and submits the final Solana transaction.

`tools/vector-device-harness` is the current host-side bridge for that flow.
It talks to the live device, verifies the SE050 binding, derives the Vector
identity and PDA from the device Falcon key, and prints the `initialize`
instruction or `advance` signing inputs needed for the next step.

## Important Nonce Detail

Vector does not start from a fixed all-zero or all-`0xff` nonce on-chain.
During `initialize`, the program derives the initial nonce from:

- the Falcon identity seed
- the latest slot-hashes sysvar entry at execution time

That means the practical first-use flow is:

1. Submit `initialize`
2. Fetch the created Vector account
3. Read the current nonce from the account header
4. Build the first `advance` digest from that nonce
5. Ask the ESP32 to sign the digest with `PQC_SIGN_VECTOR_DIGEST`
6. Submit the `advance` transaction

## Where Both Signers Work Together

- The SE050 can still sign the outer Solana transaction when an Ed25519
  signature is needed for the fee payer or another normal Solana signer.
- The ESP32 Falcon key signs the Vector authorization digest for the same
  transaction intent.
- The existing SE050 binding signature can be used to link the ESP32 Falcon
  key to the hardware wallet identity.

## Firmware Shape

- `SIGN:<b64>` stays the normal SE050 Ed25519 signer.
- `SIGN_HYBRID:<b64>` stays the current SE050 + FN-DSA test path.
- `PQC_SIGN_VECTOR_DIGEST:<base64_32_bytes>` is the Vector-specific Falcon
  path: it signs a host-supplied 32-byte digest and returns a wire-format
  Falcon-512 signature.

## Compatibility Note

The current `SIGN_HYBRID` PQC leg uses `fn-dsa` with `HASH_ID_RAW`. That is
not the same signing transcript the Vector Falcon flow expects.

For Vector compatibility, the firmware must use `HASH_ID_ORIGINAL_FALCON`
for the 32-byte digest signing path.
