# Firmware families

| Folder | Board | Role |
| --- | --- | --- |
| [esp32-only/](esp32-only/README.md) | Original ESP32 without secure elements | Frozen v0.1.0 preorder source, binaries and aggregate evidence |
| [dual-element/](dual-element/README.md) | V2 ESP32-S3 with SE050 and Tropic | Index for the already-published v0.3.3 MLH release |
| [iphone-ble-signer/](iphone-ble-signer/) | V2 ESP32-S3 with SE050 and Tropic | Exact source location pinned by the original MLH manifest |

`dual-element/` and `iphone-ble-signer/` describe the same firmware family. The
original source path stays fixed so the existing release verifier, source
identity and build tools continue to work.

From the repository root:

```sh
python3 firmware/verify.py
```

Release snapshots are immutable references. See
[maintenance notes](../docs/firmware-maintenance.md) before importing a new release.
