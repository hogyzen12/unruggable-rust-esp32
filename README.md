# Unruggable firmware

Choose the firmware family that matches your board:

| Firmware family | Hardware | Preserved release |
| --- | --- | --- |
| [ESP32-only](firmware/esp32-only/README.md) | Original ESP32, BOOT button, software keys; no secure elements or display | **v0.1.0 — 2026 customer preorder firmware** |
| [Dual-element / V2](firmware/dual-element/README.md) | ESP32-S3 + SE050 + Tropic, display, USB/BLE | **v0.3.3 — 2026 MLH firmware**, derived from the new app monorepo |

The ESP32-only archive preserves the recovered source and firmware used for the
early preorder boards. The operator confirmed its identity on 2026-10-06. The
archive records the historical evidence and its limits: the original flash logs
did not record per-board binary hashes.

The dual-element release was already published. Its source, original manifest,
release tag and download package remain unchanged. Development for that family
belongs in `firmware/iphone-ble-signer/` in the new app monorepo,
`hogyzen12/unruggable-app-pure-UX`.

## Find source, binaries and instructions

- [ESP32-only v0.1.0 archive and flashing instructions](firmware/esp32-only/releases/v0.1.0/README.md)
- [ESP32-only downloadable release](https://github.com/hogyzen12/unruggable-rust-esp32/releases/tag/firmware-v0.1.0-esp32-preorders-2026)
- [V2 v0.3.3 source, owner instructions and qualification](releases/mlh-2026/README.md)
- [V2 original downloadable release](https://github.com/hogyzen12/unruggable-rust-esp32/releases/tag/firmware-v0.3.3-mlh-2026)

Verify both checked-out firmware families without compiling or connecting a board:

```sh
python3 firmware/verify.py
```

The V2 source remains at `firmware/iphone-ble-signer/` because the published
manifest pins those exact paths. `firmware/dual-element/` is its family index.
The older top-level `esp32-solana-signer/`, `twofa/` and transaction-builder
directories remain as historical development material; use the family links
above when selecting shipped firmware.

See [firmware maintenance](docs/firmware-maintenance.md) for the repository layout,
release provenance and the relationship to ongoing development.
