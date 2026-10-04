# V2 firmware: build once, flash by hardware MAC

The 200-device order uses the ESP32 factory eFuse MAC as its only device ID.
Firmware v0.3.1 and later show it in **Menu → Device info**, alongside the firmware version,
without an app or PIN. The station reads the same MAC through espflash and checks
it against `GET_INFO;DEVICE_MAC=...` after flashing. No printed serial, label,
customer registration or factory-written ID is needed. This is a support
identifier, not a proof of authenticity against replacement firmware.

## Package

Load the Xtensa/Rust environment (`source ~/export-esp.sh` on the build Mac), then:

```sh
cargo test --locked --manifest-path tools/firmware-host-tests/Cargo.toml
cargo test --locked --test hardware_legacy_compat
python3 -m unittest discover -s tools/firmware-production -v
python3 tools/firmware-production/package.py --output dist/firmware/v2-candidate --allow-dirty
```

The packager builds the real signer and peripheral diagnostic in release mode
with locked dependencies, ESP-IDF 5.2.3 and espflash 3.3.0. It packages the matching
bootloader, partition table, generated sdkconfig, ELF/app binaries, source archive
and content hashes. Dirty source is marked candidate; build success is not
physical qualification. New changes need a new package directory. Never edit an
existing package or mix its binaries with another bootloader/partition layout.

The generated bootloader's optional SHA-256 image trailer is prepared for
espflash 3.3.0; its original image is retained too. This is integrity metadata,
not secure boot. Keep the pinned 4 MB layout on the 8 MB boards, preserving NVS
at its existing offsets. There is no whole-flash erase, merged erase image,
secure-boot, flash-encryption or debug-disable fuse operation.

## Station setup, once

Use espflash **3.3.0**, Python 3 and **pyserial 3.5**. These are already installed
on the development Mac. Use a reliable data-capable USB-C cable, one board at a
time and the explicitly selected port. Close wallet apps/serial monitors and
leave the iPhone disconnected. Prepare an incoming, passed and failed tray.

```sh
python3 /path/to/package/factory.py verify
python3 /path/to/package/factory.py run --port /dev/cu.usbmodem2101 \
  --records /path/to/prize-200-records --allow-candidate --dry-run
```

The dry run opens no board. The records folder name identifies the lot; an
optional `--lot` can override it. There is no operator or serial field to enter.
First qualify one spare fresh board through the full station workflow and owner
setup/signing, outside the recipient lot. A provisioned qualification wallet
cannot be reset to factory blank. Complete the agreed review, interruption and
assembled power/button/BLE sampling before processing the full order.

## Each recipient board

1. Plug in the next blank board. Run the **same command** without `--dry-run`;
   nothing in it changes from one board to the next.
2. The tool reads the chip's MAC and flashes diagnostic firmware. Inspect the
   display colors/text/border; press and release the button only if correct.
   Press within 30 seconds of the prompt and release within five seconds. This
   is a short click; the tool's 75-second wait is the overall diagnostic timeout.
   ESP32, SE050, Tropic, display, button and overall PCB must all PASS.
3. The tool flashes the signer, checks the exact build, matches its MAC to the
   ROM MAC, checks both chips and unused customer pairing slots, and confirms
   no PIN/key, ten attempts, no pending wipe/legacy Falcon state and **zero phone
   bonds**. Phone-bond checking is now automatic; no per-unit menu check is needed.
4. Wait for `PASS <MAC>` and the saved-record path, check the final SETUP screen,
   unplug and place the board in the passed tray. Put a failed board aside and
   retain its failure record. Do not swap boards while a command is running.
   `PCB PASS` is only the diagnostic stage; signer installation must finish too.

The station never creates a customer PIN or wallet key. Signer startup may
create/validate its separate SE050 root object. Never erase flash, create a test
wallet, or weaken a check to force a recipient board through. Reflashing does
not reset either secure element. No funds are needed for factory checks.

## Minimal automatic records

Each attempt writes a unique `<MAC>-<time>-<suffix>.json` using schema 2:
MAC, UTC time, lot, action, firmware build, package hash, PASS/FAIL and four compact
checks (PCB, blank wallet, no phone bonds, matching identity). Successful factory
records omit boot logs and detailed chip/status dumps. Failed records retain
stage, error and captured diagnostic output for troubleshooting. Failures before
identification use `UNKNOWN` in the filename. No PIN/private key is recorded.

The MAC-based lot report counts the latest valid attempt per board; retesting
one board never adds another unit. A later failure supersedes an earlier pass.
The report produces one searchable CSV with MAC, time, build, result and error.
Keep the package and record folder backed up. No separate packing spreadsheet
or manual customer list is required. A station PASS is not an independent
security audit or completion of the physical sample qualification.

## Existing wallets and support

Use `flash` for a wallet already in use; `run` is the blank-unit diagnostic flow:

```sh
python3 /path/to/package/factory.py flash --port /dev/cu.usbmodem2101 \
  --expect-mac E0:72:A1:E4:E9:D4 --records /path/to/update-records --allow-candidate
python3 /path/to/package/factory.py check --port /dev/cu.usbmodem2101 \
  --records /path/to/update-records --allow-candidate
```

`flash` preserves wallet/NVS state; `check` reads the running image and locks it.
Owner-update/check records retain public status evidence. Both report the factory
MAC. For support, ask the customer to read **Device info** and quote the full MAC
and current firmware version; find the matching manufacturing record. The record
shows what shipped, not firmware that an owner installed later.

The release supports desktop USB and authenticated bonded iPhone BLE, including
older desktop clients. USB bus loss revokes authorization; the CDC reader monitor
also revokes after 500 ms without IN polls. Forced process-death checks passed on
macOS; an OS reader that stays open through an app hang is outside this detection.
Keep its ISR in IRAM/DRAM. Phone removal changes bonds only, not wallet state.

Native PIN protection has ten attempts and permanent lockout with no reset,
recovery, PIN change or seed export. Open terminal reflashing remains supported
and does not recover a locked-out wallet. Falcon/hybrid/Vector are archived and
excluded. Qualification images expose direct chip probes and cannot enter the
production `run` flow, even with `--allow-candidate`.
