# Firmware layout

The active default-branch firmware is **V2: ESP32-S3 + SE050 + Tropic**.

| Folder | Role |
| --- | --- |
| [iphone-ble-signer/](iphone-ble-signer/) | Working V2 source, USB/BLE and diagnostics; exact path retained for published manifests and tests |
| [dual-element/](dual-element/README.md) | V2 release identity, source and package index |
| [esp32-only/](esp32-only/README.md) | Frozen v0.1.0 archive for original preorder boards |

Verify both release baselines from the repository root with
`python3 firmware/verify.py`. See [maintenance](../docs/firmware-maintenance.md)
for the research branch, earlier archives and future updates.
