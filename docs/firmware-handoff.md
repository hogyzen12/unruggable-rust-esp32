# V2 development and publication checklist

Develop firmware in `unruggable-app-pure-UX/firmware/iphone-ble-signer/`.
Publish selected, committed firmware revisions to `unruggable-firmware/main`.
The public repository is `hogyzen12/unruggable-rust-esp32`; its local clean
checkout is named `unruggable-firmware`. Do not use the old ESP32 workspace as
a source and do not run the retired `sync-iphone-firmware.sh` helper.

## One-time safeguards completed

- [x] Legacy source-copy helper fails without copying, in both workspaces.
- [x] Missing-firmware flash error no longer recommends that helper.
- [x] Export reads a full commit SHA, previews by default and records provenance.
- [x] Export is limited to firmware, host tests, factory/package tools and lot tools.
- [x] Apply refuses dirty destinations, dirty source export paths and symlinks.
- [x] GitHub checks current V2 tests/provenance separately from frozen releases.
- [x] Original preorder images, V2 release manifest/verifier and release tags stay fixed.

## 1. Develop and commit in the app monorepo

- [ ] Start from the current V2 tree and keep app/protocol changes coordinated.
- [ ] Run host, factory and lot tests below in the monorepo. For protocol or
  transaction changes, also run the relevant app tests, including
  `hardware_clear_signing`, `firmware_v1_parser` and `hardware_legacy_compat`.
  Preserve the original preorder compatibility expectations.
- [ ] Commit intended firmware/tool changes. Do not bundle unrelated local app
  work. The four exported paths must have no uncommitted changes or untracked files.
- [ ] Record the full revision using `git rev-parse HEAD`. Push the app changes
  through its normal workflow so the referenced revision is recoverable.

```sh
cargo +1.92.0 test --locked --manifest-path tools/firmware-host-tests/Cargo.toml
python3 -m unittest discover -s tools/firmware-production -v
python3 -m unittest discover -s tools/firmware-lot -v
```

## 2. Preview and export in the clean firmware checkout

- [ ] Update `main` with `git pull --ff-only`, then create a firmware update branch.
- [ ] Preview the exact committed revision. Replace `FULL_MONOREPO_COMMIT_SHA`
  below with the 40-character SHA from step 1.

```sh
python3 tools/firmware-handoff/handoff.py export \
  --source ../unruggable-app-pure-UX \
  --revision FULL_MONOREPO_COMMIT_SHA
```

- [ ] Review the listed additions, modifications and deletions. Repeat the same
  command with `--apply` to write the export. Apply requires a clean destination.
- [ ] Review `git diff` and `git status --short`, including new files and
  `firmware/v2-source.json`. Check public suitability of new source/docs; no
  private station records, PINs, keys or customer identifiers belong in an export.
- [ ] Run `bash tools/check-firmware.sh` (Python 3.10+, Rust 1.92.0 and full Git
  history/tags). Install the host compiler with
  `rustup toolchain install 1.92.0 --profile minimal` if needed.
- [ ] Commit the reviewed export and push the branch. Open/merge its PR once both
  **Current V2 source and host tests** and **Preserved preorder and V2 releases** pass.

The exporter owns only these trees, including tracked additions/removals:

| Exported path | Purpose |
| --- | --- |
| `firmware/iphone-ble-signer/` | V2 source, assets, vendor code, build scripts/config |
| `tools/firmware-host-tests/` | Tests importing the actual firmware modules |
| `tools/firmware-production/` | Package and factory tools/tests |
| `tools/firmware-lot/` | Lot report tools/tests |

It reads Git objects, not loose files; never builds, flashes, commits or pushes.
Unrelated dirty app files are not exported. A dirty file in an exported tree
blocks the operation. New shared dependencies outside these trees require a
reviewed exporter change first. Root flash/environment wrappers, this repository's
CI, release records and archives are maintained here, outside export ownership.

`firmware/v2-source.json` records the monorepo SHA and each exported file's SHA-256
and executable mode. It proves local consistency with the recorded export;
GitHub does not access the private app repo to authenticate that commit. Reviewers
with monorepo access can rerun the preview. Do not hand-edit the record to hide
source drift; make the change in the monorepo and export its new commit.

## 3. Qualify and publish a new firmware release

A source push is not a hardware-qualified release. For a new release:

- [ ] Update the firmware version and any order-specific packager policy in the
  monorepo; export the final committed revision again. Do not modify old manifests.
- [ ] Build a fresh package from a clean source checkout with the documented
  Xtensa/ESP-IDF environment. Use a new output directory; never reuse a released one.
  The packager records content-derived build identity and the source revision.
- [ ] Record the matching app revision and USB/BLE protocol compatibility, then
  qualify the exact package on the intended dual-element hardware: setup/PIN,
  both secure elements, display/button, USB/BLE, signing approval/rejection,
  reconnect/reboot and relevant regression/factory checks. Preserve evidence.
- [ ] Add a new `releases/<release-name>/` record with source/app revisions,
  build ID, artifact hashes, test results and qualification outcome. Update the
  README's qualified baseline only after this is complete.
- [ ] Publish a new tag/release and its exact package/checksums. Download the
  uploaded assets and verify them. Keep the old preorder and v0.3.3 tags/assets fixed.

`python3 firmware/verify.py` checks historical releases: the preorder archive in
this checkout and v0.3.3 source at its pinned original Git commit. It deliberately
allows newer working V2 source. Use `python3 tools/firmware-handoff/handoff.py verify`
for the current export. The unchanged `releases/mlh-2026/verify.py` is for an exact
v0.3.3 checkout/package and will reject newer source if run directly on it.

## Deferred local environment cleanup

The flash wrapper can still borrow tools from the old workspace's `.embuild`.
Keep that directory until an independent ESP-IDF installation has been tested.
This is a build-tool dependency, not a firmware-source or app-runtime dependency.
