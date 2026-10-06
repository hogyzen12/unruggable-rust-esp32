# Firmware SE050 Wrap V2 OK

Date: 2026-06-13

## Goal

Replace the Falcon private-key-at-rest model from:

- PIN-derived software wrapping only

to:

- live SE050-derived wrapping material
- plus the unlocked PIN session key
- Falcon secret only decrypted into ESP32 RAM for signing, then zeroized

## What Changed

- added a versioned PQC wrap scheme:
  - legacy `v1`: PIN-only wrap for backward compatibility
  - current `v2`: `SE050 signature-derived material + PIN session key + salt`
- new Falcon key blobs written by `PQC_INIT` now store wrap version `v2`
- signing paths derive the wrap key from the live SE050 session before
  decrypting the Falcon secret
- existing `v1` blobs still load and decrypt, so older devices are not bricked

## Security Effect

Flash/NVS extraction alone is no longer enough to brute-force the Falcon key
blob for newly initialized devices. An attacker now needs:

- the live SE050-backed device capability
- and the unlocked PIN session

This is still not a final secure-element Falcon signer, because the Falcon
secret must exist in ESP32 RAM during signing, but it is materially better than
PIN-only flash wrapping.

## Validation

```bash
cargo check --target xtensa-esp32s3-espidf
```

## Boundary

- compile-verified in this checkpoint
- not hardware-reflashed/re-probed in this turn
- multisig smart-contract changes remain independent of this firmware update
