# Unruggable V2 firmware

The default `main` branch contains the working **dual-secure-element firmware**
for the Unruggable V2 board: **ESP32-S3 + SE050 + Tropic**, display, USB and BLE.
The current qualified baseline is **v0.3.3**, build `v2-ff212385a4d13c18`.

## Working firmware and tests

| Location | Purpose |
| --- | --- |
| [firmware/iphone-ble-signer/](firmware/iphone-ble-signer/) | Main V2 firmware and board diagnostics |
| [tools/firmware-host-tests/](tools/firmware-host-tests/) | Current firmware logic and display tests |
| [tools/firmware-production/](tools/firmware-production/) | Package creation, factory flashing and its tests |
| [tools/firmware-lot/](tools/firmware-lot/) | Manufacturing record reporting and its tests |
| [releases/mlh-2026/](releases/mlh-2026/README.md) | v0.3.3 release instructions, manifest and qualification |

The historical `iphone-ble-signer` directory name covers both desktop USB and
Bluetooth operation. Its path remains fixed because the published manifest and
test harnesses reference it. Firmware source and the released binaries are
unchanged by this repository cleanup.

From the repository root:

```sh
bash tools/check-firmware.sh
```

Follow the itemized [monorepo → firmware handoff checklist](docs/firmware-handoff.md)
for new development, exports, checks and publication. The legacy sync helper is
retired. CI checks current source and preserved releases independently.

Use the [release instructions](releases/mlh-2026/README.md) for build prerequisites
and existing-wallet or blank-board flashing. Download the exact qualified
package from the [v0.3.3 release](https://github.com/hogyzen12/unruggable-rust-esp32/releases/tag/firmware-v0.3.3-mlh-2026).

## App relationship

This V2 source was developed with the new app monorepo,
`hogyzen12/unruggable-app-pure-UX`. The qualified v0.3.3 baseline matched 78
manifest-listed source files in that monorepo.
[Current export provenance](firmware/v2-source.json) records the committed
revision and hashes for the working firmware and shared tools.
The app does not load this checkout at runtime
or require it for a normal desktop/iPhone build. See
[the exact dependency boundary](docs/app-dependencies.md).

Keep firmware and app protocol changes coordinated. The monorepo remains the
current development source of truth; committed changes are exported here and
qualified separately before release. This repository's `main` is the V2 firmware
checkout, not a collection of unrelated
board experiments. Changing source requires a new build identity and qualification;
the existing release tags and archived binaries stay fixed.

## Preserved earlier work

- [ESP32-only preorder v0.1.0](firmware/esp32-only/README.md): frozen source,
  firmware images, checksums and provenance for the original preorder boards.
- [Legacy development archive](archive/README.md): older ESP32/2FA code,
  transaction-builder examples and Squads experiments retained with Git history.
- [Falcon/SE050 research branch](https://github.com/hogyzen12/unruggable-rust-esp32/tree/research/falcon-se050):
  recovered Falcon firmware, multisig work, test harnesses and historical
  checkpoint notes. It is separate from the qualified V2 firmware.

See [maintenance](docs/firmware-maintenance.md) and
[test history](docs/test-history.md) for what is active, archived or kept locally.
