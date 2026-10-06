# Preorder firmware provenance

## Operator confirmation

On 2026-10-06, the operator identified April as the SPI runs and May/June as the
customer preorder runs, then confirmed the recovered firmware as the firmware
used for those preorder boards.

## Historical station records

The original `dev_flash_log.csv` contains 788 successful flash records for 773
distinct boards from April 9 through June 27, 2026. All report version 0.1.0,
`AUTH_MODE=UNSET`, `FINALIZED=0`, `LOCKED=0`, `RETRIES_LEFT=10`,
`KEY_STATE=UNINITIALIZED`, and `ERROR:WALLET_NOT_INITIALIZED` for the public key.

| Preorder batch | Recorded UTC dates | Successful flash records | Distinct boards within batch |
| --- | --- | ---: | ---: |
| `dev-STUK` | May 1 and May 23 | 25 | 25 |
| `dev-2026-05-27-r1` | May 27–28 | 189 | 187 |
| `dev-2026-05-30-r2` | May 29–June 27 | 319 | 315 |

Across those batches there are **533 successful records and 523 distinct boards**.
Some boards appear more than once or in more than one batch. The records describe
successful flashes, not a recipient shipment ledger. June 21–22 accounts for
283 records on 283 distinct boards.

[batch-summary.json](batch-summary.json) contains the aggregate counts and the
SHA-256 of the original CSV. The original CSV, device identifiers, detailed bench
records and local device backups remain in the local recovery archive; this
public archive contains aggregate evidence.

## Source and binary recovery

- Original project: `dev-firmware/unruggable-rust-esp32/esp32-solana-signer`.
- Checkout base: `33b9c91` (August 18, 2025), with uncommitted firmware changes.
- Preserved ELF: `target-dev/xtensa-esp32-espidf/release/esp32-solana-signer`,
  last modified May 1, 2026.
- Historical batch commands selected `target/`, which is no longer present.
- The recovered app image is 557,456 bytes. All image-size entries found in the
  saved May station logs have that size.
- All 859 lines of `src/main.rs` captured in the May 1 review match the recovered
  source. SHA-256:
  `c563097d2af3e9c35c7836f20893103d413f5614ea17d5039cb64605e167d0b4`.
- Later work in the reviewed May sessions concerned station scripts, GUI/hub
  operation and operator documentation; no separate preorder runtime fork was found.
- The original local recovery manifest SHA-256 is
  `694e5719ef56e3bfabc74fea31f791e5b91bfd50d5cf8b82fad6dbee6046fc78`.
  This publication has its own manifest because it includes aggregate evidence
  and additional original operator helpers, rather than per-device records.

The source and binary bytes were not changed for publication. The SHA-256 values
in this public archive were established during recovery/publication; they are
not checksums captured at the original shipping station. Version 0.1.0, matching
image size and source evidence cannot establish exact binary identity on every
shipped board in the absence of original per-board hashes.

## Recovery bench check

The recovered bootloader, partition table and app were flashed to a replacement
ESP32-D0WD-V3 revision 3.1 board on October 6, 2026. All three write hashes passed,
and the blank-device status checks passed. No eFuses were changed, no whole-flash
erase was performed, and no wallet, PIN or signature was created.

See [bench-summary.json](bench-summary.json) for the exact responses and settings.
