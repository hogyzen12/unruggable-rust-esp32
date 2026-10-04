# MLH batch — Unruggable firmware v0.3.3

This is the firmware shipped on the 2026 MLH batch. The release preserves the
original v0.3.3 binaries and their matching source; it is an archival release,
not a new firmware update for those devices.

| Identity | Value |
| --- | --- |
| Firmware | `0.3.3` |
| Build reported by the board | `v2-ff212385a4d13c18` |
| Git tag | `firmware-v0.3.3-mlh-2026` |
| Manifest SHA-256 | `10670a2ee95aa947c6ca6ac328fefd7502d621d8ea9b5b7b1fe610063fe1efe9` |
| Board | Unruggable V2, ESP32-S3 with SE050 and Tropic |
| Partition layout | Qualified 4 MB layout, including on the batch's 8 MB chips |

Download the original firmware package and its checksums from the
[MLH GitHub release](https://github.com/hogyzen12/unruggable-rust-esp32/releases/tag/firmware-v0.3.3-mlh-2026).
The maintained MLH source is **`firmware/iphone-ble-signer/`**. The older
`esp32-solana-signer/` directory in the public repository is historical and is
not the source for this batch.

## Capabilities and owner behavior

- Solana Ed25519 signing through Tropic, with SE050-backed six-digit PIN
  protection and physical approval of signing requests.
- Desktop USB-C serial and authenticated, bonded iPhone Bluetooth transport.
- Session locking on disconnect/expiry and USB host/reader loss. The USB
  application-loss behavior was physically qualified on macOS; an application
  hang that leaves the OS reader active is outside that detection.
- On-device PIN unlock, Receive address/QR, Lock, Forget phones and Device info.
  The full factory MAC and firmware version are available in Device info.
- Transaction review derived from the message bytes. Supported Send requests
  use the feature screen, amount/recipient and checklist. The exact recognised
  three-instruction app marker bundle is summarised in one checklist entry.
- Unknown transaction effects are explicitly labelled NOT DECODED, with a
  request hash and warning checklist. Tap through, then hold SIGN for two
  seconds; REJECT and REVIEW AGAIN are available on the final checklist.
- Ten failed PIN attempts permanently lock the wallet. This release has no
  PIN change, reset, recovery or seed export. Forget phones changes Bluetooth
  pairings only. iOS may also need its stale pairing removed in Settings.
- Terminal reflashing remains supported. Reflashing does not reset PIN
  attempts or restore access to a locked-out wallet. ESP32 secure boot and
  flash-encryption fuses were not enabled for this open-hardware batch.

Falcon/PQC and expanded stake/swap/Squads clear signing are outside this release.
Artwork identifying a transaction category does not mean its effects were fully
decoded; the displayed NOT DECODED warning remains authoritative.

## Verify the checked-out source

From the repository root at the release tag:

```sh
python3 releases/mlh-2026/verify.py
```

This verifies the pinned manifest and every one of its 78 source files, including
the original file paths and content-derived build identity. Current MLH release
documentation lives here, outside that frozen source snapshot; some older
READMEs inside the snapshot describe historical firmware.

## Use the original firmware package

Download `unruggable-mlh-firmware-v0.3.3.tar.gz` and its `.sha256` file into the
same directory. On macOS, check and extract it with:

```sh
shasum -a 256 -c unruggable-mlh-firmware-v0.3.3.tar.gz.sha256
tar -xzf unruggable-mlh-firmware-v0.3.3.tar.gz
cd unruggable-mlh-firmware-v0.3.3
shasum -a 256 -c SHA256SUMS
python3 firmware/factory.py verify
```

The archive contains the original manifest, signer and diagnostic ELF/app
binaries, bootloader, partition table, generated SDK configuration, factory tool
and source archive. It does not contain customer manufacturing records.

The manifest retains its original `candidate` and dirty-source provenance.
The subsequent qualification and shipment are recorded by this release;
rewriting the original manifest would invalidate its identity. Consequently
the preserved flashing tool still requires `--allow-candidate`.

### Existing wallet: preserve owner state

Use **espflash 3.3.0**, Python 3 and **pyserial 3.5**, a USB data cable and one
board. Close apps/serial monitors and disconnect the phone from the wallet.
Replace the sample port and MAC below with the actual values from your board:

```sh
python3 firmware/factory.py flash --port /dev/cu.usbmodemXXXX \
  --expect-mac AA:BB:CC:DD:EE:FF --records ../owner-update-records --allow-candidate
python3 firmware/factory.py check --port /dev/cu.usbmodemXXXX \
  --records ../owner-update-records --allow-candidate
```

Use the appropriate COM port on Windows or serial-device path on Linux.
`flash` updates the firmware while preserving NVS and secure-element state.
`check` reads status and locks the wallet. Do not use a whole-flash erase or
merge padded binaries across NVS. Shipped MLH v0.3.3 boards already have this
firmware and need no reflash to use this release.

### New blank boards: factory diagnostics and signer

The factory command is for blank recipient boards. It flashes diagnostics,
requires display inspection and a short button press/release, flashes the
signer, checks the matching MAC/build and blank wallet state, and saves a record.

```sh
python3 firmware/factory.py run --port /dev/cu.usbmodemXXXX \
  --records ../factory-records --allow-candidate
```

Wait for the final `PASS <MAC>` and saved record, not just `PCB PASS`. No customer
PIN, wallet key or phone bond is created at the station. A board used for owner
setup cannot be reset into a blank recipient unit.

## Build and test

The build host needs the Espressif Xtensa Rust toolchain named `esp`, `ldproxy`,
the ESP-IDF build prerequisites and espflash **3.3.0**. Load that toolchain's
environment (for example `source ~/export-esp.sh`). ESP-IDF is pinned to **5.2.3**;
the original manifest records the exact compiler version. Development rebuilds
do not replace the archived binary hashes.

From the repository root:

```sh
python3 releases/mlh-2026/verify.py
cargo test --locked --manifest-path tools/firmware-host-tests/Cargo.toml
python3 -m unittest discover -s tools/firmware-production -v
python3 -m unittest discover -s tools/firmware-lot -v
python3 tools/firmware-production/package.py --output dist/firmware/my-build
```

The packager builds both signer and diagnostics with locked dependencies and
creates a new package directory. It refuses source changes during the build.
Changed source produces a different content-derived build ID and needs its own
qualification; do not relabel a new build with the MLH release tag.

## Qualification and provenance

Recorded v0.3.3 acceptance includes both desktop USB adapters, iPhone Bluetooth
signing/reconnect, local PIN, Receive/lock, marked Send, simplified blind review,
explicit rejection and the new-unit factory procedure. Automated qualification
recorded 80 host tests, two display tests, 25 clear-signing tests, 27 compatibility
tests, ten factory tests and eight lot-report tests.

The spare's normal first setup/cold restart passed on v0.3.2 before the v0.3.3
review-only change. Its planned setup power interruption was missed and is not
claimed as tested on that spare. Earlier interruption/fault-injection evidence
remains the baseline. The firmware is not represented as independently audited.

Manufacturing records contain 223 distinct latest PASS boards and ten failed
boards kept separately. The operator reported shipment; the exact selected
200 recipient MACs were not recorded. Per-board records remain in the private
manufacturing backup.

The MLH source tag and release package identify the same shipped firmware in
the public firmware repository and the private new-app monorepo. Their Git
commit IDs differ; the original source/manifest/binary hashes establish alignment.
