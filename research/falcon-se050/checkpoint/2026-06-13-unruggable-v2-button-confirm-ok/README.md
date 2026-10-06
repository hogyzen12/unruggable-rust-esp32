# Unruggable V2.0 Button Confirmation Flow OK

Date: 2026-06-13

## Result

The **Unruggable V2.0** board now uses the physical `USER_BTN` for sensitive
actions in the signer firmware.

This was verified live on hardware after flashing the updated
`dual-element-se050` firmware.

## What Changed

- `src/board.rs`
  - added `user_button` to the board profile
  - mapped V2.0 `USER_BTN -> IO5`
- `src/main.rs`
  - wired the button into runtime init
  - added hold-to-confirm gating for:
    - wallet creation
    - `PQC_INIT`
    - `SIGN`
    - `SIGN_HYBRID`
    - `PQC_SIGN_VECTOR_DIGEST`
    - `WIPE_KEYS`
- `src/display.rs`
  - made the frame border visible
  - added generic hold-progress screens
  - improved the ready screen footer and prompts
- `tools/hybrid-harness/src/main.rs`
  - increased action timeout
  - added operator prompts for physical button holds

## Commands That Passed

Build checks:

```bash
cargo check --target xtensa-esp32s3-espidf
cargo check --target aarch64-apple-darwin --manifest-path tools/hybrid-harness/Cargo.toml
```

Flash:

```bash
./scripts/flash_se050_dev.sh \
  --port /dev/cu.usbmodem1101 \
  --board-profile dual-element-se050 \
  --skip-build
```

Live hardware verification:

```bash
./scripts/run_hybrid_harness.sh \
  --port /dev/cu.usbmodem1101 \
  --pin <PIN> \
  --initial-wipe \
  --sample-tx-b64 aGVsbG8=
```

## Live Evidence

The hardware harness passed with physical button confirmation on-device for:

- `WIPE_KEYS`
- wallet creation during `SET_MODE:PIN:<PIN>`
- `PQC_INIT`
- `PQC_SIGN_VECTOR_DIGEST`
- `SIGN`
- `SIGN_HYBRID`
- final `WIPE_KEYS`

The run completed with:

- `PQC_READY`
- `Vector-compatible Falcon digest signature verified.`
- `SIGN Ed25519 signature verified.`
- `SIGN_HYBRID Ed25519 and FN-DSA signatures verified.`
- final `WIPED`

## Final Device State

The board was left on the normal signer firmware and returned to a **blank
state** after the verification run.

- `AUTH_MODE=UNSET`
- `KEY_STATE=UNINITIALIZED`
- `PQC_STATE=UNINITIALIZED`
