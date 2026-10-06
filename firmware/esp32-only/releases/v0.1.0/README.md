# ESP32-only v0.1.0 — 2026 preorder archive

This archive preserves the firmware recovered from the original ESP32-only
development project on 2026-10-06. The operator confirmed it as the firmware used
for the customer preorder boards. It is an archival publication of existing
firmware; no firmware code was changed or rebuilt for publication.

| Item | Location |
| --- | --- |
| Preserved release ELF | [firmware/signer.elf](firmware/signer.elf) |
| Flashable images and saved build configuration | [firmware/](firmware/) |
| Original source and locked dependencies | [source/](source/) |
| Setup and UART protocol | [source/DEV_UNIT_INFO.md](source/DEV_UNIT_INFO.md) |
| Batch, source and recovery provenance | [evidence/provenance.md](evidence/provenance.md) |
| Public file manifest | [manifest.json](manifest.json) |
| File checksums | [SHA256SUMS](SHA256SUMS) |

## Identity and behavior

- Target: classic ESP32, revision 3.1 in the recorded batches; 4 MB flash.
- Software Ed25519 keys in NVS; no external secure element or display.
- Reported version: `0.1.0`.
- Fresh units are blank. A wallet is created after six-digit PIN setup and a
  two-second BOOT-button hold. NONE and OTP setup are disabled in this baseline.
- Signing and private-key export require the appropriate unlocked state and
  physical approval. Ten failed authentication attempts lock the wallet.
- Missing or inconsistent key storage is reported as a fault instead of silently
  generating a replacement wallet.

The recovered app image is **557,456 bytes**, matching the May batch logs.
Its SHA-256 is
`95e8173b21821eb3cc8256a27d70165307e091668e4755c97e2fd7eef9299cb7`.
The ELF was preserved from `target-dev/`, not rebuilt. The app binary was
generated from that ELF during recovery using espflash 3.3.0 with the original
ESP32 / DIO / 40 MHz / 4 MB settings.

## Verify

From this archive directory, on macOS:

```sh
shasum -a 256 -c SHA256SUMS
```

From the repository root, the family verifier also checks the pinned manifest:

```sh
python3 firmware/verify.py --family esp32-only
```

## Flash a new blank ESP32 board

The recovery bench used **esptool 4.11.0** at 115200 baud. With that version
installed, run from this archive directory and replace the sample serial port:

```sh
python3 -m esptool --chip esp32 --port /dev/cu.usbserial-XXXX --baud 115200 \
  write_flash --flash_mode dio --flash_freq 40m --flash_size 4MB \
  0x1000 firmware/bootloader.bin \
  0x8000 firmware/partition-table.bin \
  0x10000 firmware/signer.bin
```

If automatic bootloader entry fails, hold BOOT, tap EN/RESET, then release BOOT
and retry. The command writes the three firmware regions without a whole-flash
erase. NVS is at `0x9000..0xefff`; flashing these images does not reset an existing
owner's wallet. The historical `source/provision_dev.sh` erases flash by default,
so it is not the command to use for preserving an existing wallet.

At 115200 baud, a blank board should answer newline-terminated UART commands:

```text
GET_INFO -> INFO;VER=0.1.0;AUTH_MODE=UNSET;FINALIZED=0;LOCKED=0;RETRIES_LEFT=10;KEY_STATE=UNINITIALIZED
GET_PUBKEY -> ERROR:WALLET_NOT_INITIALIZED
GET_TIME -> TIME:0
SIGN:AA== -> ERROR:MODE_UNSET
```

These checks passed on the recovery bench after all three written-image hashes
were verified. No PIN or wallet was created and no signature was produced during
that bench check. See [the public bench summary](evidence/bench-summary.json).

## Source and historical tools

`source/` contains the original local source, configuration and non-secure
provisioning/QA helpers. The recovered runtime and build files match the local
recovery snapshot byte for byte; helper files absent from that smaller snapshot
were copied unchanged from the original project. See `manifest.json` for each
file's origin and checksum.

The underlying checkout was based on `33b9c91` with uncommitted firmware changes;
that commit alone does not identify the preorder firmware. The archived `main.rs`
also matches all 859 lines captured during the May 1 firmware review.

Historical operator notes retain their original paths and commands. Rebuilding
requires the Espressif `esp` Rust toolchain and pinned ESP-IDF prerequisites and
creates a new artifact, rather than replacing the recovered binary's identity:

```sh
cd source
cargo +esp build --locked --release --target xtensa-esp32-espidf
```

Original flash logs did not include binary hashes, and the `target/` directory
used by the batch commands is no longer available. Operator confirmation,
matching source, protocol responses and image size establish this reference;
they do not provide a byte-for-byte readback of every shipped device.
