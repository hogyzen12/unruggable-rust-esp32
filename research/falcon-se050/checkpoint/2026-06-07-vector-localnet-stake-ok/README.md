# 2026-06-07 Vector Localnet Native Stake Delegation

This checkpoint records the first successful end-to-end localnet native stake
delegation authorized by the ESP32 Falcon signer through the Vector program.

## What changed

- Added `full-stake-delegate` mode to `tools/vector-onchain-harness`
- Added CLI/env support for:
  - `--stake-lamports` / `VECTOR_STAKE_LAMPORTS`
  - `--stake-vote-account` / `VECTOR_STAKE_VOTE_ACCOUNT`
- Added on-chain verification that:
  - the Vector nonce advances to the signed digest
  - the created stake account is owned by the native stake program
  - the stake account authorities remain the Vector PDA
  - the delegated vote account matches the requested validator

## Verified command

```bash
./scripts/run_vector_localnet_onchain_harness.sh \
  --port /dev/cu.usbmodem1101 \
  --pin <PIN> \
  --commitment confirmed \
  --fee-payer-file /Users/hogyzen12/.config/solana/worKFoQQH5KzuBnmS3jKKYsJuUi5toCoEp7n4mwRtwa.json \
  --mode full-stake-delegate
```

## Result

- Initialize tx: `5kAs3mwtFKTTphYhugvxhAS3snsc949KRPYR1wPcFiyGxam5LnQoeHa9qsZnzkPJsdCs2auLzvuCFWMeuN6YMY27`
- Stake setup tx: `2s1rnt32x92cWhoSKJfcxc7ZKqZpa3moxfopR8EuZ8a3paKBgnvzVm4GkMMFYxz91V6xDJ8FFnR8u1hyxSTDVfxS`
- Stake delegate tx: `2HLJYMFmwsF3VTWVtBoq1gG5AANVqKs7grwTGiJd62Tv8Q837AL5Yf2NawwmTnV7dGtwmpW5Jfn3xxPtB4vksPxa`
- Vector digest hex: `bea2a7d70f60016ecc4db7fe125c8e8430c67b67c0c7d794d46a00c253ed4819`
- Stake account: `8mGKruV5Sp4a2yW7EJyw2UJ76o1BM8s6LiE3mAVVoANK`
- Vote account: `9qTVaa5tEFvpLYw8xh5wLxyk37GWMVMxJrEuvoXhiQo`
- Delegated stake: `1_001_000_000` lamports

## Notes

- The outer fee payer stayed local; the ESP32 only signed the Vector digest.
- This confirms the Falcon flow now covers three concrete Solana actions on localnet:
  - SOL withdraw
  - SPL token transfer
  - native stake delegation
