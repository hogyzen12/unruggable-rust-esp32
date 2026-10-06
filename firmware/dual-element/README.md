# Dual-element — Unruggable V2 firmware v0.3.3

For **ESP32-S3 + SE050 + Tropic** boards with a display and USB/BLE transports.
The published MLH release is **v0.3.3**, build **`v2-ff212385a4d13c18`**.

| Item | Location |
| --- | --- |
| Release instructions, behavior and qualification | [releases/mlh-2026/README.md](../../releases/mlh-2026/README.md) |
| Exact published source | [firmware/iphone-ble-signer/](../iphone-ble-signer/) |
| Original manifest | [manifest.json](../../releases/mlh-2026/manifest.json) |
| Original binaries and factory package | [GitHub release](https://github.com/hogyzen12/unruggable-rust-esp32/releases/tag/firmware-v0.3.3-mlh-2026) |

Development belongs in `firmware/iphone-ble-signer/` in the new app monorepo,
`hogyzen12/unruggable-app-pure-UX`. Qualified releases flow from that repository
into this public firmware repository.

- Monorepo release commit: `6505a7a61b9746e2445687c0878a95d59cb1f17c`
- Original public release commit: `dd1f86d6633b66e203e8c78f5cd3130eb67ab98c`
- Release tag in both repositories: `firmware-v0.3.3-mlh-2026`
- Original archive SHA-256: `2755627f0f7f5c81d8cf2e938c994505411a4c5cf56728ffafa78029e08a45df`

The original 78 source files, their paths, manifest and released package are
preserved. This folder is an index, not another copy of the source to develop.

```sh
python3 firmware/verify.py --family dual-element
```

For an extracted original release package, also verify its binaries and archived
source with the existing release verifier:

```sh
python3 releases/mlh-2026/verify.py --package /path/to/unruggable-mlh-firmware-v0.3.3/firmware
```

Use the release instructions above for existing-wallet and blank-board flashing.
The original qualified partition layout and factory procedure remain authoritative.
