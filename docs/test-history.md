# Firmware tests and history

## Active V2 checks

The default branch keeps these existing checks and their original source paths:

```sh
python3 firmware/verify.py
cargo test --locked --manifest-path tools/firmware-host-tests/Cargo.toml
python3 -m unittest discover -s tools/firmware-production -v
python3 -m unittest discover -s tools/firmware-lot -v
```

The [MLH release record](../releases/mlh-2026/README.md) documents the original
USB/BLE, device UI, signing, factory and other qualification evidence. That record
and its original manifest have not been rewritten. Running host tests during
repository cleanup is not another hardware qualification or firmware rebuild.

App transaction compatibility tests live in the app monorepo and use its own
copy of the same V2 firmware modules; see [app dependencies](app-dependencies.md).

## Preserved earlier tests

The [ESP32 preorder archive](../firmware/esp32-only/releases/v0.1.0/README.md)
contains the original non-secure flashing/QA helpers, aggregate batch records and
October recovery bench results.

The [Falcon research branch](https://github.com/hogyzen12/unruggable-rust-esp32/tree/research/falcon-se050)
preserves the later SE050/Falcon firmware, Vector/multisig harnesses and selected
June checkpoint notes. Those notes describe the runs performed then; they do not
claim that all historical on-chain or hardware tests were rerun during this
cleanup. Full local records, build artifacts and private key material remain in
the original development workspace.

Older public ESP32/2FA and transaction-builder examples remain in
[archive/legacy-development/](../archive/legacy-development/).
