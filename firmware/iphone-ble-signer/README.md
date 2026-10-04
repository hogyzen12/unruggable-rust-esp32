# iPhone BLE Signer Firmware

This directory is the repo-local firmware fork that will evolve alongside the
Pure UX iPhone app.

What lives here:

- the ESP32-S3 signer firmware crate
- the board/display/SE050 source needed to build and flash it
- the minimal board-review note and flash script needed for local device work
- the upstream firmware README copied as `UPSTREAM-README.md` for reference

What is intentionally excluded:

- old checkpoints and build outputs
- local validator state
- archive material
- Solana program repos and other unrelated toolchains from the upstream checkout

Current status:

- version 0.2.0 targets the first 200 open-hardware prize units; Solana signing over desktop USB-C and iPhone BLE, with Falcon archived for later
- six-digit PIN, ten hardware-enforced failed attempts, permanent lockout and no reset; customer setup protects the Tropic wallet against the bootstrap credential used by replacement ESP32 firmware
- the [current security design and qualification](../../docs/firmware-theft-protection-2026-09-28.md) and [factory workflow](../../tools/firmware-production/README.md) are the release references
- real-wallet BLE now requires authenticated Secure Connections pairing; enter the code shown on the wallet into the phone pairing dialog

- this fork starts from the current USB-serial signer baseline
- `esp32-solana-signer` is the real signer image with USB serial plus BLE transport
- `iphone_ble_wallet` is the BLE proof binary for iPhone transport work only
- the app-side transport review is in
  [../../docs/iphone-wallet-transport-review.md](../../docs/iphone-wallet-transport-review.md)
- the BLE proof milestones are in [docs/ble-proof-plan.md](docs/ble-proof-plan.md)
- the demo-oriented flash and transport notes are in
  [../../docs/demo-wallet-firmware-scout.md](../../docs/demo-wallet-firmware-scout.md)

Factory PCB test (V2)
---------------------

The `peripheral_probe` image is a non-destructive, interactive factory test for
fresh V2 PCBs. It checks the ESP32-S3 flash/heap/eFuse identity, performs a
CRC-validated SE050 reset plus a read-only SELECT APDU, probes the Tropic L2
transport, draws an RGBW display/border pattern, and validates a debounced
button press and release. It does not provision keys or alter secure-element
objects.

```bash
./scripts/flash_factory_test.sh --port /dev/cu.usbmodemXXXX
```

Inspect the colour bars, text, border, orientation, and backlight, then press
the board button. The final display and serial line report each component. A
fresh board without the Tropic fitted reports `PCB=PASS_PRE_TROPIC` and
`TROPIC=NOT_FITTED`; after fitting Tropic, a complete result is `PCB=PASS` and
`TROPIC=PASS`. Reset the board to repeat the test. Flash the production image
after recording the result.

Repo-local workflow:

```bash
./tools/flash-iphone-wallet-firmware.sh --port /dev/cu.usbmodemXXXX --board-profile dual-element-se050
```

The flash wrapper now defaults to the real `esp32-solana-signer` bin. To flash
the BLE proof image instead:

```bash
./tools/flash-iphone-wallet-firmware.sh --port /dev/cu.usbmodemXXXX --bin iphone_ble_wallet
```

For the order, use the immutable packaged factory workflow linked above. ESP32
erase/reflash does not erase the secure elements, reset PIN attempts, or make a
provisioned board new again. Older development wallet credentials are not
automatically migrated by this release.

Avoid `--no-button` for demo prep. The real V2 board profile has a default user
button. The real signer refuses confirmation if its button or screen is missing.

The source-sync tool can overwrite this fork. It is not part of routine builds. If you intentionally want to refresh from the old upstream source repo, pass it explicitly:

```bash
./tools/sync-iphone-firmware.sh /absolute/path/to/esp32-solana-signer
```
