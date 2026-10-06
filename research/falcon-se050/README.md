# SE050 / Falcon / Vector / multisig research

This branch preserves the later local research work separately from the
[working dual-element V2 firmware on main](https://github.com/hogyzen12/unruggable-rust-esp32/tree/main).
It is a recovered development snapshot, not a qualified shipping release.

## Preserved work

- `src/`, `assets/`, `vendor/` and the root Cargo/config files: ESP32-S3 firmware
  with SE050-protected wrapping, software Falcon signing, board/display changes
  and diagnostic binaries.
- `pq-multisig-vault/`: FalconOnly multisig program and core helpers.
- `tools/` and `scripts/`: hardware/Vector/multisig harnesses, operator CLI,
  dashboard, localnet and mainnet workflows.
- `vector/`: original nested Vector repository source, including its five local
  modifications. Its base revision is recorded in the provenance manifest.
- `solana-falcon512/`: the exact tracked upstream dependency source and fixtures.
- `checkpoint/`: historical June checkpoint Markdown notes. Full run payloads,
  device logs, program artifacts and operator keypairs remain in the original
  local development workspace.
- `docs/`: architecture, readiness plans, board bring-up and research status.

The snapshot preserves 244 recovered files. Runtime source and tooling are copied
unchanged. Literal PIN values in 14 Markdown notes are replaced with `<PIN>`;
the manifest records both the original and public hashes for those notes.
Original notes remain local. The nested
repository metadata was replaced by recorded upstream URLs/revisions and file
hashes; their source directories are ordinary tracked files in this snapshot.
The public upstream sample signing-key fixture is retained for tests; it is not
an operator wallet key.

## Verify the recovery

From the repository root:

```sh
python3 research/falcon-se050/verify-snapshot.py
```

`source-provenance.json` records the original checkout/branch, selected file
hashes, nested repository revisions and exclusions. That original dirty checkout
and its private files remain intact locally. The snapshot is not a full backup
of the original machine or an assertion that all old deployment paths still work.

No source was rebuilt for device deployment and no hardware/on-chain test was
rerun during this archival step. Checkpoint notes describe their historical runs;
read the relevant notes and scripts before resuming research. Existing local
absolute paths and network/deployment commands have deliberately been retained
as historical source rather than silently rewritten.

The V2 source and release packages elsewhere in this branch remain the same as
main. Do not copy this research firmware over the qualified V2 source tree.
