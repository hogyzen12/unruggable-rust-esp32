# 2026-06-07 Vector Localnet SPL Transfer OK

Checkpoint for the first real Falcon-authorized SPL token transfer on localnet
using the ESP32 device.

## What Passed

- Local validator loaded `vector_falcon512.so`
- ESP32 `PQC_SIGN_VECTOR_DIGEST` signed the real Vector digest on hardware
- `tools/vector-onchain-harness` ran `--mode full-spl-transfer`
- The harness created a temporary mint and associated token accounts
- The source ATA was owned by the Vector PDA
- The final transaction submitted `advance + passthrough(transfer_checked)`
- The on-chain Vector nonce advanced to the signed digest
- The source ATA and destination ATA balances changed as expected

## Successful Command

```bash
./scripts/run_vector_localnet_onchain_harness.sh \
  --port /dev/cu.usbmodem1101 \
  --pin <PIN> \
  --commitment confirmed \
  --fee-payer-file /Users/hogyzen12/.config/solana/worKFoQQH5KzuBnmS3jKKYsJuUi5toCoEp7n4mwRtwa.json \
  --mode full-spl-transfer
```

## Observed Result

- Mint setup tx: `528UGucVT9ntmSpFasqhFsajge7Wfanr383yXRQfjbHDce9WMS22moRRhPCBn4s3x1jy1HVcrFhL6tiyEUGXWycw`
- Mint-to tx: `5dJE4Bj7UQh9cVW8GF1NcF6eQSBWUqNfG7T4nBiaJyEj4CLXpJLDUGAsA1EJ9PEEYPnqxH6wTMKB5NDv84SRcuDc`
- SPL transfer tx: `3dwAgxhzjxs9axt7MXayVHiRamVBBhijLgMhwtLuqhevEuhcXXFxNeepC4N2tWf9h59vcYpc47BpipTh7LYeag9j`
- Advance digest hex: `739566f593b50d4da985915edb052b39a308a5ac2b0efe21038f4c1b2b3dada8`
- Source ATA balance: `20000 -> 10000`
- Destination ATA balance: `0 -> 10000`

## Important Implementation Note

The passthrough builder must clear `is_signer` on sub-instruction accounts.
Vector promotes PDA signing during CPI with `invoke_signed`; the outer
transaction must not require a real signature from the Vector PDA.
