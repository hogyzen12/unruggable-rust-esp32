## Dev/Test flash (non-secure)

For the short operator/app overview, see `DEV_UNIT_INFO.md`.
For high-throughput multi-board flashing, see `BATCH_FLASHING.md`.

Use this for regular app/host testing on fresh non-fused boards:

```bash
./provision_dev.sh --port /dev/tty.usbserial-0001
```

Behavior after a successful dev flash:
- Post-flash smoke test must pass over UART.
- Fresh blank units should report `KEY_STATE=UNINITIALIZED`.
- `GET_PUBKEY` should return `ERROR:WALLET_NOT_INITIALIZED` until the user completes `SET_MODE:PIN:<6digits>`.
- After `MODE_SET:PIN`, the host should call `GET_PUBKEY` to read the newly created wallet.
- If the user wants a backup, the host should call `UNLOCK:PIN:<6digits>` and then `EXPORT_PRIVKEY`, and the device will require a fresh button press before returning `PRIVKEY:<base58>`.
- Each successful flash appends a trace row to `logs/dev_flash_log.csv` with timestamp, port, hardware MAC, chip info, firmware version, and smoke-test responses.

## Recommended dev-unit operator flow

Set the validated artifact directory once:

```bash
export PROVISION_TARGET_DIR="$PWD/target"
```

### 1. Full end-to-end QA on one unit

This flashes blank dev firmware first, then verifies PIN setup, wallet creation, unlock, private-key export, and signing. It leaves the board in the verified QA state so you can inspect it before wiping.

```bash
python3 -u qa_e2e_dev.py \
  --port /dev/cu.usbserial-0001 \
  --pin 246810 \
  --yes \
  --skip-build
```

During this flow:
- hold the button for about 2 seconds for `SET_MODE:PIN`
- press once for `EXPORT_PRIVKEY`
- press once for `SIGN`

### 2. Reset a QA-passed board back to blank stock

This wipes the flashed QA state and reflashes the verified blank dev image for shipping.

```bash
./reset_dev_unit.sh --port /dev/cu.usbserial-0001
```

### 3. Batch flashing after preflight passes

```bash
./provision_dev.sh --skip-build --skip-efuse-check --port /dev/cu.usbserial-0001 --batch-tag dev-batch-001
```

## Sample QA flow

Use this on sample units to verify the full user flow. By default, if QA passes, the script automatically reflashes the unit blank for production:

```bash
python3 qa_sample_dev.py --port /dev/tty.usbserial-0001 --pin 246810
```

What it checks:
- blank-state smoke check
- PIN setup
- wallet creation and `GET_PUBKEY`
- PIN unlock
- `EXPORT_PRIVKEY` with button confirmation
- `SIGN` with button confirmation
- automatic blank reflash via `./provision_dev.sh` after a pass

If the dev-unit workflow skips eFuse probing, add `--reflash-skip-efuse-check` when using this lower-level script directly.

If you want to keep the QA wallet on the device for debugging, use:

```bash
python3 qa_sample_dev.py --port /dev/tty.usbserial-0001 --pin 246810 --keep-after-pass
```

## Secure provisioning (final dev units)

Use this only once per fresh board when freezing units:

```bash
./provision_secure.sh --port /dev/tty.usbserial-0001
```

## Protocol latency smoke check

```bash
python3 protocol_rtt.py --port /dev/tty.usbserial-0001 --cmd GET_INFO --cmd GET_TIME --cmd GET_PUBKEY
```
