# 2026-06-07 Vector Localnet E2E

This checkpoint captures the first successful end-to-end local Vector/Falcon
transaction flow using:

- the live ESP32 device for `PQC_SIGN_VECTOR_DIGEST`
- the local `vector/programs/falcon512` SBF program
- a local `solana-test-validator`
- a local fee payer for the outer Solana transaction

## Result

The full local flow succeeded:

1. Start local validator with the Falcon Vector program loaded
2. Run the host on-chain harness against the real board
3. Submit `initialize`
4. Fetch the on-chain nonce
5. Ask the ESP32 to sign the real Vector digest
6. Submit `advance`

## Commands

Build the local Vector Falcon program:

```bash
./scripts/build_vector_falcon_program.sh
```

Start the local validator:

```bash
./scripts/start_vector_falcon_local_validator.sh \
  --mint-keypair /Users/hogyzen12/.config/solana/worKFoQQH5KzuBnmS3jKKYsJuUi5toCoEp7n4mwRtwa.json
```

Run the end-to-end local harness:

```bash
./scripts/run_vector_localnet_onchain_harness.sh \
  --port /dev/cu.usbmodem1101 \
  --pin <PIN> \
  --commitment confirmed \
  --fee-payer-file /Users/hogyzen12/.config/solana/worKFoQQH5KzuBnmS3jKKYsJuUi5toCoEp7n4mwRtwa.json \
  --mode full-empty
```

## Runtime Values

- Vector program id:
  `HdkE3dPYgCRZJgLv64mbFmojyCprUim8VRXzK2wR6Qgm`
- Device SE050 pubkey:
  `f2T3PEs5GXL8syLNhjFTiYmvnHswDtQ5X5mYPoBAsWz`
- Device Vector identity:
  `ca3c84e7353326d9efbc668003a14399fae39bb9f83a3b932eed4c779b214db8`
- Device Vector PDA:
  `auocbN27ioAvhuT6S5Sqr1xN5T1eVnptGZQXkEBbV9Q`

## Successful Transactions

`initialize`:

- signature:
  `54Lz4JPdrCFw4tR8GQyBoyapi3tavgzXmsTGDHt6uDAiJrmo2ZgRVqkPp9gtadx7Y2c57VVsZbHAdXm6jNVDuz8`
- nonce:
  `fd11b4b5acb5471c12d3e7e26f69c1af448fd8977cf6a9ef528c7fe463198324`
- lamports:
  `8477280`

`advance-empty`:

- signature:
  `274vvHxiiYmF5AhrnXMc6WonFa8UrqX7adHDYMX1k92B1ZNXTxbesW7zQ1kt73pnfp9qsaYLUgFeaoCqbPXApeat`
- digest:
  `8a67c6ed9c239d001ec0b2cdda4b07360effc8ba5738791b761fb3e63abc3b12`

The next nonce observed on-chain matched the digest expected by the host
harness.

## Meaning

This is the first confirmed proof point that the repo can:

- deploy the Falcon Vector program locally
- derive the correct Vector digest from the live account nonce
- ask the real ESP32 to produce a compatible Falcon signature
- submit a Solana transaction whose Vector instruction verifies that signature

What still does **not** work:

- public-cluster use, because the repo program id is not deployed on the
  target public RPCs tested so far
- the full SE050-outer-signer route for Vector-sized Solana messages, because
  the current SE050 `SIGN` path still rejects those messages
