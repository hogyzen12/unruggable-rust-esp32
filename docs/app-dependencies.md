# What the app depends on

Checked against `unruggable-app-pure-UX` on 2026-10-06. This cleanup changes the
public firmware repository, not the app working tree or its local toolchain.

## Running or building the app

The desktop/iPhone app communicates with the board over USB serial or BLE using
its own transport and protocol code. The app's Cargo dependencies and build
script do not import this sibling firmware checkout. Normal app builds do not
build ESP-IDF firmware.

The app monorepo contains its own firmware copy at `firmware/iphone-ble-signer/`.
All 78 entries pinned by the v0.3.3 release manifest match this repository's
copy. That is the source used by the app's firmware tests and package tools.

## Tests that share firmware code

These references are internal to the app monorepo, not links to this checkout:

- `src/backend/transaction_guards.rs` includes `signing_review.rs` under
  `#[cfg(test)]` to check app transaction markers against the device decoder.
- `tests/hardware_clear_signing.rs` and `tests/firmware_v1_parser.rs` include the
  firmware review/parser modules.
- `examples/hardware_usb_check.rs` uses the monorepo's review module.
- `tools/firmware-host-tests/` includes firmware modules and the vendored display
  driver from the same monorepo.
- Firmware package/factory tools also use that internal source tree.

Keep these compatibility tests when coordinating changes to app transaction
construction, serial/BLE commands and firmware review behavior.

## The two references to the older local checkout

`tools/flash-iphone-wallet-firmware.sh` builds/flashes the monorepo's own firmware,
but optionally borrows ESP-IDF 5.2.3 and tools from:

`/Users/hogyzen12/coding-project-folders/unruggable-rust-esp32/esp32-solana-signer/.embuild/espressif`

It supplies unset `IDF_PATH`, `IDF_TOOLS_PATH` and `IDF_PYTHON_ENV_PATH` values
when those directories exist, and prepends available compiler/CMake/Ninja tools
to `PATH`. This is a developer build-environment convenience, not an app runtime
or firmware-source dependency. The original directory is retained in place.

`tools/sync-iphone-firmware.sh` is an older, manually invoked source-copy helper.
Its default source is that same old ESP32/SE050 working directory. Running it can
overwrite the monorepo's newer firmware. It is not part of the normal app build;
the monorepo's existing orientation notes already say not to use it as a routine
pre-build step. It was not invoked or changed during this cleanup.

## Future decoupling

The remaining local environment coupling can be removed by giving firmware builds
an independently configured ESP-IDF/toolchain installation and retiring the old
source-copy helper. That change belongs in the app's developer tooling, separate
from reorganizing these firmware sources. No app behavior or protocol change is
required to remove those old filesystem references.
