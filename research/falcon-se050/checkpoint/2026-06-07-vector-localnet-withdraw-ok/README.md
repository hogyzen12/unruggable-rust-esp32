# 2026-06-07 Vector Localnet Withdraw OK

This checkpoint captures the first successful Falcon-authorized
value-moving Solana transaction on localnet using the live ESP32 device.

## Flow

The transaction path was:

1. `initialize` the Falcon Vector account
2. top up the Vector PDA so it held more than its rent floor
3. ask the ESP32 to sign the real Vector digest for
   `advance + passthrough(withdraw)`
4. submit the final Solana transaction with a normal local fee payer

## Command

```bash
./scripts/run_vector_localnet_onchain_harness.sh \
  --port /dev/cu.usbmodem1101 \
  --pin <PIN> \
  --commitment confirmed \
  --fee-payer-file /Users/hogyzen12/.config/solana/worKFoQQH5KzuBnmS3jKKYsJuUi5toCoEp7n4mwRtwa.json \
  --mode full-withdraw \
  --withdraw-lamports 3000000
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
  `19Df3hBnWWkGPKt73Nv21M5NttvgpxNjDmkxqayiFwofGD5cSMi75ydwNX26if8TBqRVhkL7enaJ7ds1Pv5jfdR`
- nonce:
  `09b60fd5a556d7ff976144988ed7cd08e3266f8d4748425e50c343f2b16c0f53`
- starting lamports:
  `8477280`

top-up:

- signature:
  `4qxNL3QaHmnp9HbFps6DKYQg5Uh5cEkj9T5GHLcCm6FKq8AwGwTpH2eG76f7CvScGKqFbZAvRgkJgMXUP8cq69x4`
- top-up lamports:
  `3000000`
- lamports after top-up:
  `11477280`

`withdraw`:

- signature:
  `37E8LCSqVN58GPEa16gZ7TTotbj8u7G2xBs32H6vZyBQKCGjx37RfPtTcAAMfrXAL5NXpYS6gvX7knoJbC8qDA8p`
- requested lamports:
  `3000000`
- advance digest:
  `3c149d732f5926eaf4d5b188f52d5dc9ed6226676fb28b2176b10b84f2c3bf73`
- final Vector lamports:
  `8477280`

The final nonce observed on-chain matched the signed digest.

## Meaning

This is the first confirmed proof point that the repo can do a real
Falcon-authorized Solana transaction on localnet:

- the ESP32 signs the exact Vector digest
- the on-chain Falcon Vector program accepts that signature
- the Vector account state advances
- lamports move under that authorization

This is the right base to build richer Vector-authorized transaction flows
next, such as SPL authority changes or protocol-specific instruction bundles.
