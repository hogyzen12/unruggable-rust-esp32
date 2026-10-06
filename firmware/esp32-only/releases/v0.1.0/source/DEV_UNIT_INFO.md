# Dev Unit Firmware Info

This file is the short operator and app-integration reference for the current ESP32 dev-wallet firmware.

## What Is Implemented

- ESP32 dev-unit firmware for a Solana signing wallet over UART.
- Wallet key is generated on-device.
- Freshly flashed blank units do not contain a wallet yet.
- New wallet creation happens only when the user completes `SET_MODE:PIN:<6digits>`.
- PIN mode is the intended setup path for this dev batch.
- `NONE` and `OTP` setup are disabled for new units.
- Signing requires both a valid unlocked PIN session and a physical button press.
- Private-key backup export is implemented and requires both a valid unlocked PIN session and a physical button press.
- Silent wallet rekeying is blocked. If stored key material is inconsistent, the firmware fails closed instead of silently creating a new wallet.
- Physical factory wipe is supported by holding the button on boot for about 10 seconds.

## Important Dev-Unit Constraints

- These dev units are flashed normally. We are not burning eFuses in this workflow.
- These dev units are not secure-boot devices and are not physically tamper-resistant.
- This firmware is blind-signing firmware. There is no display-based transaction verification.
- The app must assume the host computer is not fully trusted and must rely on device button confirmation for sensitive actions.

## Device States

- Blank shipping state:
  - `AUTH_MODE=UNSET`
  - `FINALIZED=0`
  - `KEY_STATE=UNINITIALIZED`
  - `GET_PUBKEY -> ERROR:WALLET_NOT_INITIALIZED`
- User-initialized PIN state:
  - `AUTH_MODE=PIN`
  - `FINALIZED=1`
  - `KEY_STATE=READY`
  - `GET_PUBKEY -> PUBKEY:<base58>`
- Fault state:
  - `KEY_STATE=FAULT`
  - identity-dependent commands fail closed

## UART Protocol Basics

- Transport is line-based UART.
- Commands are ASCII and newline-terminated.
- Responses are one line each.
- Success responses are plain payloads.
- Failures are `ERROR:<code>`.

Main commands:

- `GET_INFO`
  - Returns firmware version, auth mode, lock state, retries left, and key state.
- `GET_PUBKEY`
  - Returns `PUBKEY:<base58>` once the wallet exists.
- `SET_MODE:PIN:<6digits>`
  - First-time setup command.
  - Requires a 2-second button hold.
  - Creates the wallet if the device is blank.
- `UNLOCK:PIN:<6digits>`
  - Opens a 7-minute unlocked session.
- `SIGN:<base64_payload>`
  - Requires active PIN unlock and a button press.
  - Returns `SIGNATURE:<base64_signature>`.
- `EXPORT_PRIVKEY`
  - Requires active PIN unlock and a button press.
  - Returns `PRIVKEY:<base58>`.
- `SHUTDOWN`
  - Requires button confirmation.

Useful error responses:

- `ERROR:WALLET_NOT_INITIALIZED`
- `ERROR:MODE_FINAL`
- `ERROR:AUTH_FAILED`
- `ERROR:AUTH_LOCKED`
- `ERROR:LOCKED`
- `ERROR:BUTTON_TIMEOUT`
- `ERROR:KEYSTORE_CORRUPT`

## How The App Should Interface With The Wallet

### First user onboarding

The app should treat a newly flashed device as blank.

Recommended flow:

1. Call `GET_INFO`.
2. If `AUTH_MODE=UNSET` and `KEY_STATE=UNINITIALIZED`, show setup UI.
3. Ask the user for a 6-digit PIN.
4. Send `SET_MODE:PIN:<pin>`.
5. Tell the user to hold the device button for about 2 seconds.
6. After `MODE_SET:PIN`, call `GET_PUBKEY`.
7. Show the new wallet address to the user.

Important:

- Do not assume a pubkey exists before PIN setup.
- Do not preload or inject wallet material from the app side.

### Normal signing flow

1. Call `UNLOCK:PIN:<pin>`.
2. If unlock succeeds, send `SIGN:<base64_payload>`.
3. Tell the user to press the device button.
4. Read `SIGNATURE:<base64_signature>`.

### Private-key backup flow

1. Call `UNLOCK:PIN:<pin>`.
2. Send `EXPORT_PRIVKEY`.
3. Tell the user to press the device button.
4. Read `PRIVKEY:<base58>`.

Notes:

- The returned value is base58-encoded 64-byte Solana keypair material: `secret32 || pubkey32`.
- Backup export is intentionally supported on this dev firmware.

### What the app should not do

- Do not expect OTP setup in this dev batch.
- Do not expect auth mode changes after setup.
- Do not expect a wallet to survive `erase-flash` or factory wipe.
- Do not assume these dev units are hardware-secure against reflashing.

## Flashing And QA

For high-throughput multi-board flashing, use `BATCH_FLASHING.md`.

Set the validated artifact directory once:

```bash
export PROVISION_TARGET_DIR="$PWD/target"
```

### Build release firmware

```bash
cargo +esp build --release --target xtensa-esp32-espidf
```

### Flash blank dev firmware

```bash
./provision_dev.sh --skip-build --skip-efuse-check --port /dev/cu.usbserial-0001 --batch-tag dev-batch-001
```

This should end with:

- `AUTH_MODE=UNSET`
- `KEY_STATE=UNINITIALIZED`
- `GET_PUBKEY -> ERROR:WALLET_NOT_INITIALIZED`

### Full end-to-end QA on one board

```bash
python3 -u qa_e2e_dev.py \
  --port /dev/cu.usbserial-0001 \
  --pin 246810 \
  --yes \
  --skip-build
```

This flow:

- flashes blank firmware
- verifies blank state
- sets the PIN
- verifies wallet creation
- verifies PIN unlock
- verifies private-key export
- verifies signing

After this script passes, the board is left in a verified QA state with a test PIN and wallet on it.

### Reset a QA-passed board back to blank stock

```bash
./reset_dev_unit.sh --port /dev/cu.usbserial-0001
```

This erases the QA state and reflashes blank stock firmware for shipping.

## Manual Bootloader Entry

Some ESP32 boards do not reliably auto-enter download mode.

If `espflash` stalls at `Connecting...`:

1. Hold `BOOT`.
2. Tap `EN`.
3. Keep holding `BOOT` until `Using flash stub` appears.
4. Release `BOOT`.

## Recovery And Reset Behavior

- Factory wipe:
  - hold the button on boot for about 10 seconds
  - wipes wallet and auth state
  - next setup creates a new wallet
- `erase-flash`:
  - wipes NVS
  - destroys existing wallet identity and PIN state

## Suggested Shipping Expectation

Each shipped dev unit should be:

- flashed successfully
- verified to boot and answer UART
- blank, with no pre-generated wallet
- ready for the end user to create a new wallet during PIN setup

## Related Files

- `src/main.rs`
- `buildnflash.md`
- `qa_e2e_dev.py`
- `reset_dev_unit.sh`
- `provision_dev.sh`
- `DEV_DEVICE_FIRMWARE_SPEC.md`
