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

That harness now also checks `PQC_SIGN_VECTOR_DIGEST` by asking the ESP32 to
sign a 32-byte demo digest and verifying the returned signature with
`solana-falcon512`.

Useful local Falcon/Vector wire-compatibility probe:

`cargo +stable run --manifest-path tools/vector-falcon-interop/Cargo.toml`

Useful live-device Vector bridge harness:

`./scripts/run_vector_device_harness.sh --port /dev/cu.usbmodemXXXX --pin <PIN>`

If you want it to also print the `initialize` instruction shape, pass a payer:

`./scripts/run_vector_device_harness.sh --port /dev/cu.usbmodemXXXX --pin <PIN> --payer <base58_fee_payer>`

After you have actually submitted `initialize` and fetched the current Vector nonce
from the created on-chain account, you can ask the device to sign the real Vector
digest and print the matching `advance` instruction shape:

`./scripts/run_vector_device_harness.sh --port /dev/cu.usbmodemXXXX --pin <PIN> --nonce-hex <64_hex_nonce>`

Useful live-device/on-chain Vector RPC harness:

`./scripts/run_vector_onchain_harness.sh --port /dev/cu.usbmodemXXXX --rpc-url <url> --mode full-empty --fee-payer-file ~/.config/solana/id.json`

That harness talks to the device over serial, submits `initialize` if needed,
fetches the on-chain Vector nonce, asks the ESP32 to sign the real 32-byte
Vector digest, and submits an empty `advance`.

Useful no-RPC device-payer probe:

`./scripts/run_vector_onchain_harness.sh --port /dev/cu.usbmodemXXXX --mode probe-device-payer`

That probe currently fails on live hardware for both dummy Vector `initialize`
and dummy Vector `advance` messages, which means the current SE050 `SIGN` path
still rejects Vector-style outer Solana messages. The immediate working route is
therefore `local fee payer + ESP32 Falcon Vector authorization`.
