# What the app depends on

Checked against `unruggable-app-pure-UX` on 2026-10-06. The handoff safeguards
update the legacy helper and its missing-source error
in both workspaces, with a checklist in each. App code and the local toolchain
are unchanged.

## Running or building the app

The desktop/iPhone app communicates with the board over USB serial or BLE using
its own transport and protocol code. The app's Cargo dependencies and build
script do not import this sibling firmware checkout. Normal app builds do not
build ESP-IDF firmware.

The app monorepo contains its own firmware copy at `firmware/iphone-ble-signer/`.
At the v0.3.3 baseline, all 78 release-manifest entries matched this
repository's copy. Current exports have separate commit/hash provenance in
`firmware/v2-source.json`. App tests and package tools use the monorepo's own source.

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

`tools/sync-iphone-firmware.sh` is now a retired entry point that fails without
copying files in either workspace. Its former default could overwrite V2 source
with the old development tree. Use the explicit committed-source exporter in
the clean firmware checkout; see [the checklist](firmware-handoff.md).

## Future decoupling

The remaining local environment coupling can be removed by giving firmware builds
an independently configured ESP-IDF/toolchain installation. The source-copy
helper has already been retired. Further decoupling belongs in the app's developer tooling, separate
from reorganizing these firmware sources. No app behavior or protocol change is
required to remove those old filesystem references.
