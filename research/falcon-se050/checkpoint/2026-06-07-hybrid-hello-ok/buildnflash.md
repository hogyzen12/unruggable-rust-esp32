cargo +esp build

This build is the ESP32-S3 + SE050 showpiece firmware:

- PIN-only setup
- no button confirmations
- 2FA/OTP setup disabled
- fresh or wiped units are blank until first PIN setup
- `SET_MODE:PIN:<6digits>` creates the SE050 wallet and finalizes auth
- `WIPE_KEYS`, `FACTORY_WIPE`, or `RESET_DEVICE` return the device to blank

The flashable debug image is:

`target/xtensa-esp32s3-espidf/debug/esp32-solana-signer`

Recommended scripts:

- Fresh flash + smoke test + final wipe back to blank:

`./scripts/flash_and_smoke_se050.sh --port /dev/cu.usbmodemXXXX`

- Fresh flash + hybrid cryptographic verification harness:

`./scripts/flash_and_hybrid_harness.sh --port /dev/cu.usbmodemXXXX --sample-tx-b64 <base64_payload>`

- Flash only:

`./scripts/flash_se050_dev.sh --port /dev/cu.usbmodemXXXX --erase`

- Reset an already-flashed unit back to blank:

`python3 scripts/reset_se050_blank.py --port /dev/cu.usbmodemXXXX`

Manual flash command:

`espflash flash --chip esp32s3 target/xtensa-esp32s3-espidf/debug/esp32-solana-signer --port /dev/cu.usbmodemXXXX`

If you want a totally clean flash + NVS state first:

`espflash erase-flash --port /dev/cu.usbmodemXXXX`

Then flash again:

`espflash flash --chip esp32s3 target/xtensa-esp32s3-espidf/debug/esp32-solana-signer --port /dev/cu.usbmodemXXXX`

Expected blank-state protocol after boot or after `WIPE_KEYS`:

- `GET_INFO` includes `AUTH_MODE=UNSET` and `KEY_STATE=UNINITIALIZED`
- `GET_PUBKEY` returns `ERROR:WALLET_NOT_INITIALIZED`
- `SE050_STATUS` returns `SE050_BLANK`

Expected first-time setup flow:

1. `SET_MODE:PIN:<PIN>`
2. `GET_PUBKEY`
3. `UNLOCK:PIN:<PIN>`
4. `SIGN:<base64_payload>`

Useful local smoke test:

`python3 test_firmware.py`

Useful local hybrid harness:

`./scripts/run_hybrid_harness.sh --port /dev/cu.usbmodemXXXX --pin <PIN> --sample-tx-b64 <base64_payload>`
