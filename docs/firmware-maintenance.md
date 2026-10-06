# Firmware maintenance

## Repository map

- `firmware/esp32-only/releases/v0.1.0/`: frozen recovered preorder firmware,
  including binaries, source, original operator helpers and aggregate evidence.
- `firmware/dual-element/`: index for the V2 firmware family.
- `firmware/iphone-ble-signer/`: exact V2 v0.3.3 source at its manifest-pinned paths.
- `releases/mlh-2026/`: original V2 release documentation, manifest and verifier.
- `esp32-solana-signer/`, `twofa/`, `solana-transaction-builder/` and other older
  directories: historical development material retained in place.

## Preserve development and released identities

This housekeeping was prepared in a separate checkout based on the published
`master` commit `dd1f86d`. Existing dirty development checkouts were not switched,
reset, cleaned, staged or merged into this publication. Their working files remain
in place. This commit archives the selected preorder source and tools; it is not
a backup of every unrelated local experiment.

Do not use the older top-level crate as the reference for supporting preorder
owners. It predates the PIN/setup/key-lifecycle changes in the recovered archive.
The new public archive captures those uncommitted changes as a named reference.

Keep release source, binaries and manifests fixed. A rebuild or firmware edit
needs a new versioned archive, new hashes and its own qualification. Add updated
documentation outside a frozen archive where possible.

## ESP32-only

Use the v0.1.0 archive as the stable preorder support reference. The operator
confirmed its shipping use, while the provenance notes retain the limits of the
historical logs. Keep full per-device station logs and device dumps local.

## Dual-element / V2

The source of truth for development is `firmware/iphone-ble-signer/` in
`hogyzen12/unruggable-app-pure-UX`. Qualify future releases there, then export their
source, manifests and packages into this public repository with a new release
identity. Do not pull V2 source from the older ESP32 or SE050/Falcon experiments.

The existing `firmware-v0.3.3-mlh-2026` tag and package remain unchanged. Its
manifest includes 78 source files across `firmware/iphone-ble-signer/` and `tools/`.
Preserve those paths so its verifier and source-derived build identity stay valid.

Verify both checked-out families with `python3 firmware/verify.py`. The existing
MLH verifier also accepts an extracted original package through `--package`.
