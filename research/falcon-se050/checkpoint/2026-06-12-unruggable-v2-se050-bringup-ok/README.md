# Unruggable V2.0 SE050 Bring-Up OK

Date: 2026-06-12

## Result

The newer **Unruggable V2.0** ESP32-S3 board is now brought up successfully with
the signer firmware's `dual-element-se050` board profile.

- The SE050 is present and responding on the real V2.0 bus.
- The TFT pin map is correct.
- The signer firmware boots and reports `BOARD=dual-element-se050`.
- The live hybrid harness passed through:
  - blank-state wipe
  - PIN setup
  - SE050 wallet creation
  - Falcon/PQC init
  - SE050 binding verification
  - `PQC_SIGN_VECTOR_DIGEST`
  - `SIGN`
  - `SIGN_HYBRID`
  - final wipe back to blank
- V2.0 now also supports real physical button confirmation on `USER_BTN=IO5`
  for wallet creation, Falcon init, sign, hybrid sign, Vector digest sign, and
  wipe.

The board was left on the **normal signer firmware** and returned to a **blank
state** after verification.

## V2.0 Pin Map

From `Unruggable V2.0/PDF/SCH_Unruggable V2.0.pdf`:

- SE050
  - `SE_ENA -> IO15`
  - `SE_SCL -> IO21`
  - `SE_SDA -> IO26`
- TFT
  - `TFT_CS -> IO42`
  - `TFT_SCL -> IO41`
  - `TFT_SDA -> IO40`
  - `TFT_DC -> IO39`
  - `TFT_RES -> IO38`
  - `TFT_BL -> IO37`
- Tropic footprint (not populated on this board)
  - `TROPIC_CSN -> IO47`
  - `TROPIC_SCK -> IO33`
  - `TROPIC_MISO -> IO34`
  - `TROPIC_MOSI -> IO48`
- User button
  - `USER_BTN -> IO5`

## Firmware Surface

Relevant files:

- `src/board.rs`
- `src/main.rs`
- `src/display.rs`
- `src/bin/peripheral_probe.rs`
- `scripts/flash_se050_dev.sh`
- `docs/dual-element-board-review.md`

`dual-element-se050` now has built-in V2.0 defaults. Explicit pin overrides are
still supported for later PCB spins.

## Commands That Passed

Build checks:

```bash
cargo check --target xtensa-esp32s3-espidf --bin esp32-solana-signer
SIGNER_BOARD_PROFILE=dual-element-se050 cargo check --target xtensa-esp32s3-espidf --bin esp32-solana-signer
SIGNER_BOARD_PROFILE=dual-element-se050 cargo check --target xtensa-esp32s3-espidf --bin peripheral_probe
```

Probe build:

```bash
SIGNER_BOARD_PROFILE=dual-element-se050 cargo +esp build --bin peripheral_probe
```

Flash signer firmware:

```bash
./scripts/flash_se050_dev.sh \
  --port /dev/cu.usbmodem1101 \
  --board-profile dual-element-se050
```

Hybrid/Falcon verification:

```bash
./scripts/run_hybrid_harness.sh \
  --port /dev/cu.usbmodem1101 \
  --pin <PIN> \
  --initial-wipe
```

Hybrid/Falcon verification including a live `SIGN` + `SIGN_HYBRID` proof:

```bash
./scripts/run_hybrid_harness.sh \
  --port /dev/cu.usbmodem1101 \
  --pin <PIN> \
  --initial-wipe \
  --sample-tx-b64 aGVsbG8=
```

## Live Evidence

Probe output included:

- `Board profile: dual-element-se050`
- `I2C(se050): SDA=IO26 SCL=IO21 EN=IO15`
- `[I2C] Found 0x48 (SE050 expected)`
- `[SE050] ATR looks valid -> SE050 responding`
- `[TFT] SPI bus/device init OK on configured pins`

Hybrid harness output included:

- `GET_INFO ... BOARD=dual-element-se050`
- `SE050_STATUS -> SE050_BLANK`
- `SET_MODE:PIN:<PIN> -> MODE_SET:PIN`
- `GET_PUBKEY -> PUBKEY:3QAwiY5thY9XWDTr7NNWdjMFSu79BN65BZipTMAtPZJ5`
- `PQC_INIT -> PQC_READY`
- `PQC_SIGN_VECTOR_DIGEST ... -> Vector-compatible Falcon digest signature verified.`
- `SIGN:aGVsbG8= -> SIGNATURE:...`
- `SIGN_HYBRID:aGVsbG8= -> SIGNATURE:... + PQC_SIGNATURE:...`
- `SIGN_HYBRID Ed25519 and FN-DSA signatures verified.`
- final `WIPE_KEYS -> WIPED`

## Next Step

If this unit is being handed to someone else:

- it is already flashed with the correct V2.0 firmware
- it is currently blank, so the recipient can set their own PIN and initialize
  their own SE050 + Falcon state

If more polish is needed:

- optionally repeat the `SIGN_HYBRID` sample-payload smoke test before shipping
- optionally add Tropic bring-up once the `TR01` part is actually populated
