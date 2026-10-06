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
- The host harnesses accept an explicit Vector Falcon program ID, so they can
  target a local validator or a future custom deployment instead of assuming
  the repo default.

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

## Current Repo State

- The ESP32 Falcon side is ready: `PQC_SIGN_VECTOR_DIGEST` works on hardware
  and verifies against `solana-falcon512`.
- `tools/vector-onchain-harness` is the next-step client for the real on-chain
  flow: it can submit `initialize`, fetch the current nonce, ask the ESP32 to
  sign the resulting Vector digest, and submit either an empty `advance` or
  a real `advance + passthrough(withdraw)` or
  `advance + passthrough(transfer_checked)` transaction.
- The Falcon Vector program is now deployed on mainnet at
  `FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU`.
- Mainnet canaries have already passed for `full-empty` and `full-withdraw`,
  which means the deployed program accepts real ESP32-produced Falcon digest
  signatures on-chain.
- The `vector` repo itself is structured around local validator and local ELF
  tests. Its hardcoded program IDs are protocol constants and local test
  addresses, not proof of public deployment on mainnet or devnet.
- The current firmware still blocks the fully-hybrid outer-transaction path.
  A live no-RPC probe shows the SE050 `SIGN` path rejects dummy Vector
  `initialize` and `advance` messages, so a true `SE050 outer tx signer +
  Falcon Vector auth` flow still needs a firmware-side signing-path change.

## Firmware Shape

- `SIGN:<b64>` stays the normal SE050 Ed25519 signer.
- `SIGN_HYBRID:<b64>` stays the current SE050 + FN-DSA test path.
- `PQC_SIGN_VECTOR_DIGEST:<base64_32_bytes>` is the Vector-specific Falcon
  path: it signs a host-supplied 32-byte digest and returns a wire-format
  Falcon-512 signature.

## First Real Solana Tx To Ship

The first practical Falcon-authorized Solana transaction in this repo is a
Vector lamport withdrawal:

1. Ensure the Vector account exists with `initialize`
2. Top the Vector PDA up above its rent floor
3. Build `advance + passthrough(withdraw)`
4. Ask the ESP32 to sign that exact Vector digest
5. Submit the transaction with a normal outer fee payer

That proves the key property we care about: the ESP32 Falcon key is not just
signing test bytes, it is authorizing a real on-chain state transition and
value movement through the Vector Falcon program.

The next practical Falcon-authorized transaction is a Vector-controlled SPL
token transfer:

1. Ensure the Vector account exists with `initialize`
2. Create a temporary mint and a Vector-owned source ATA
3. Mint test tokens into the Vector-owned source ATA
4. Build `advance + passthrough(transfer_checked)`
5. Ask the ESP32 to sign that exact Vector digest
6. Submit the transaction with a normal outer fee payer

That proves the same Falcon authorization model works for SPL assets, not just
lamports, as long as the Vector PDA is the relevant token authority.

The next localnet flow after SPL transfer is native stake delegation:

1. Ensure the Vector account exists with `initialize`
2. Create a fresh native stake account funded by the outer fee payer
3. Initialize that stake account with both `staker` and `withdrawer` set to the Vector PDA
4. Build `advance + passthrough(delegate_stake)`
5. Ask the ESP32 to sign that exact Vector digest
6. Submit the transaction with a normal outer fee payer

That proves Falcon authorization also works for native Solana stake operations
when the stake authority is the Vector PDA.

## Mainnet Rollout

The clean path from the working localnet harnesses to mainnet is:

1. Deploy `vector_falcon512.so` under your own program ID on devnet
2. Repoint the host harnesses to that devnet RPC and program ID
3. Verify `full-empty`
4. Verify a tiny `full-withdraw`
5. Verify a tiny `full-spl-transfer`
6. Verify `full-stake-delegate`
7. Only then move the same flow to mainnet

The immediate mainnet transaction model should stay:

- a normal outer fee payer or relayer signs the Solana transaction
- the ESP32 Falcon key signs the Vector digest
- the Vector program verifies Falcon on-chain and performs the authorized CPI flow

That remains necessary until the firmware can reliably use the SE050 to sign
Vector-sized outer Solana messages.

## Compatibility Note

The current `SIGN_HYBRID` PQC leg uses `fn-dsa` with `HASH_ID_RAW`. That is
not the same signing transcript the Vector Falcon flow expects.

For Vector compatibility, the firmware must use `HASH_ID_ORIGINAL_FALCON`
for the 32-byte digest signing path.
