# PQ Multisig V1 Review Fixes OK

Date: 2026-06-13

## Scope

Address the two review findings selected for FalconOnly v1 before mainnet:

1. `close_proposal` should not let arbitrary closers redirect proposal rent.
2. Generic CPI bundle proposals should not remain publicly available in v1 if
   the operator tooling cannot present them safely.

## What Changed

- `close_proposal` now refunds proposal and proposal-tx lamports back into the
  vault PDA, not to a caller-chosen receiver.
- `create_close_proposal_instruction(...)` was updated across the core crate,
  operator CLI, and local harness to pass the vault account as the close
  receiver target.
- `ACTION_KIND_CPI_BUNDLE` is now explicitly reserved/disabled in FalconOnly v1:
  - proposal creation rejects it
  - execution rejects it
  - operator surfaces label it as reserved/disabled rather than implying support
- developer docs/status files were updated to reflect the narrowed v1 surface.

## Validation

Compile checks:

```bash
cargo check --target aarch64-apple-darwin --manifest-path pq-multisig-vault/Cargo.toml
cargo check --target aarch64-apple-darwin --manifest-path tools/pq-multisig-ops/Cargo.toml
cargo check --target aarch64-apple-darwin --manifest-path tools/pq-multisig-harness/Cargo.toml
```

Local operator quickstart:

```bash
./scripts/run_pq_multisig_ops_localnet_quickstart.sh --no-build
```

## Verified Behavior

- FalconOnly hybrid-init rejection still passed
- async proposal discovery / approve / execute still passed
- cancelled proposal moved into the `closable` queue
- `close` succeeded after the refund change
- SPL transfer, config update, replace-member, accept-pending-member, and
  post-rotation `3-of-3` execution all still passed after the public-surface
  change

## Localnet Txs

- SOL execute tx: `ZmMQG4Bq4e2PCAfhXeadxDvBrdK79fggPXJm5U754o6apcUQ2pmkH7fMHttjoRgUj7MBVY5QDMyQpY4WwN83PY8`
- Close tx: `ncrH52kBQzb9iWL5XJ3F6kq1in5xEwCkZKV66WLRBuad3Rkskk3i5nz81pLm8Ne8EdwzqLgE5h7Nm9Lfm6WoPfz`
- SPL execute tx: `3D5QvypfszjJqB2ikSna913YVcAgq6w5jeavBWS33BkEVayySv256FTFdkEPPGqkMXM4fLZyWFHsFyzwrZX41GVi`
- Config execute tx: `4bo1n47U9cmsNPPZP2yheEoQcfmzYSk4B1TDr2hXpGhm8v8syTxhsidA1WFoNH4iLnf7uZmB26VbBNUSfW3qUwtj`
- Replace execute tx: `4Duxx5FGpHcrETcfcYKTXGjUHgiBzaNg89Fuefvx36TdrKLB27XvjyxSszWgxz6QADg1VnpGtcpnt8ujgj52LMMh`
- Accept tx: `HhYttzNf4uiPaxJXi3oep9wjqi32zqqR89Tx7CfPR74D1T46nEmacMe4vDq4xsy3uW5bqAV8FeDbUjz44BV2o1c`
- Post-rotation tx: `ct2CvTpDKDrrtoQXft5tFb5WtY2mWHt1mqvcnayNVGUnGQwHh5QBq4v6ztEzLiEuJwHvWEz2KF4vbtnjxEhrFTe`

## Release Effect

FalconOnly v1 is now tighter in the two places called out by review:

- proposal rent can no longer be siphoned by arbitrary closers
- v1 no longer exposes opaque generic bundle proposals as a supposedly usable
  public proposal type
