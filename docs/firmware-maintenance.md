# Firmware maintenance

## Branches and layout

- `main`: working dual-element V2 firmware, active host/factory/lot tests and
  release tooling. Current qualified baseline: v0.3.3.
- `research/falcon-se050`: recovered experimental SE050/Falcon, Vector and
  multisig source, test harnesses and historical checkpoint notes.
- `master`: retained previous repository layout at `48c2189` for existing links
  and local checkouts. New firmware repository work starts on `main`.
- `firmware/esp32-only/releases/v0.1.0/`: immutable preorder archive.
- `archive/legacy-development/`: older public examples and development source.

The `firmware-v0.3.3-mlh-2026` and
`firmware-v0.1.0-esp32-preorders-2026` release tags and download packages remain
unchanged. The V2 manifest pins 78 source files and their paths; the preorder
manifest pins its recovered source, images and evidence summaries.

## Firmware and app ownership

The qualified V2 code currently matches the new app monorepo at
`firmware/iphone-ble-signer/`. Develop coordinated changes in that monorepo and
export qualified firmware revisions here. Keep release tags and packages fixed;
future source changes need a new build identity, package and qualification.
Do not import older root-crate or Falcon research files over the V2 baseline.

See [app-dependencies.md](app-dependencies.md) for the app's internal firmware
copy, shared tests and legacy local build-tool references.

## Tests and records

Keep the current firmware host tests and factory/lot tests active. Keep historic
research harnesses and checkpoint notes with their research branch. Archive old
examples instead of mixing their entry points with V2. See
[test-history.md](test-history.md).

Existing local development workspaces are retained in place, including their
uncommitted changes, nested repositories, local keys and full checkpoint data.
The research branch is a selected source snapshot, not a public backup of private
operator data. Build caches and private manufacturing records are not committed.

From the repository root, `python3 firmware/verify.py` verifies the current
release baselines. The MLH verifier also accepts an extracted original package
through `--package`.
