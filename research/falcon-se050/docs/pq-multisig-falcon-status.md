# PQ Multisig Falcon Status

Date: 2026-06-13

## Current State

`pq_multisig_vault` is now in a good **pre-mainnet FalconOnly v1** state.

- localnet async multisig flow works end to end
- operator CLI flow works end to end
- real hardware-backed member flow now also works end to end on localnet
- local dashboard + CLI handoff exists
- FalconOnly membership is now enforced on-chain
- hybrid Ed25519 + Falcon member sets are intentionally rejected in v1

## What Works

- permissionless vault initialization
- async proposals:
  - create
  - approve
  - revoke
  - cancel
  - close
  - execute
- proposal kinds:
  - SOL withdraw
  - SPL transfer
  - config update
  - replace member
- generic CPI bundle proposals are intentionally reserved/disabled in
  FalconOnly v1
- pending-member acceptance flow
- rotated-out member rejection after acceptance
- post-rotation `3-of-3` execution proof

## FalconOnly V1 Boundary

The vault is now explicitly **FalconOnly**:

- every member must be a live Vector/Falcon PDA
- initial member sets reject:
  - zero pubkeys
  - duplicate pubkeys
  - non-Vector accounts
- member-driven actions require a live Vector/Falcon account
- pending-member acceptance requires the same
- proposal close refunds return to the vault PDA, not to an arbitrary closer
- hybrid/classical multisigs are out of scope for v1

## Operator Surface

Main operator surfaces:

- `tools/pq-multisig-ops/`
- `scripts/run_pq_multisig_ops.sh`
- `scripts/run_pq_multisig_ops_localnet_e2e.sh`
- `scripts/run_pq_multisig_ops_localnet_quickstart.sh`
- `scripts/run_pq_multisig_ops_localnet_hardware_e2e.sh`
- `scripts/run_pq_multisig_ops_localnet_hardware_quickstart.sh`
- `scripts/deploy_pq_multisig_vault.sh`
- `scripts/run_pq_multisig_ops_mainnet_hardware_canary.sh`
- `scripts/run_pq_multisig_dashboard.sh`

Current UX features:

- `list-proposals`
- `show-proposal`
- alias-based vault/member naming
- `--json` output
- `--manifest-only` dashboard handoff
- bash/zsh completions

## Latest Verified Flow

Most recent full localnet proof:

- hybrid/classical init rejected
- Falcon-only init succeeded
- SOL execute succeeded
- cancel/close succeeded
- SPL transfer succeeded
- config update to `3-of-3` succeeded
- replace-member succeeded
- accept-pending-member succeeded
- rotated-out member was rejected
- post-rotation `3-of-3` execute succeeded

See:

- `checkpoint/2026-06-11-pq-multisig-falcononly-v1-ok/README.md`

Most recent hardware-backed localnet proof:

- hardware member created proposal `0`
- fixture member approved proposal `0`
- vault executed proposal `0`
- fixture member created proposal `1`
- hardware member approved proposal `1`
- vault executed proposal `1`

See:

- `checkpoint/2026-06-13-pq-multisig-hardware-localnet-e2e-ok/README.md`

Most recent v1 review-fix checkpoint:

- `checkpoint/2026-06-13-pq-multisig-v1-review-fixes-ok/README.md`

## What Is Left Before Mainnet

- deploy `pq_multisig_vault` to mainnet
- run the scripted internal mainnet hardware canary:
  - one hardware Falcon member
  - two fixture Falcon members
  - one hardware-authored SOL withdraw
  - one fixture-authored SOL withdraw approved by hardware
  - close both executed proposals
- swap the local fixture members for multiple real hardware members in the first
  mainnet canary vault
- complete a security review / audit pass on the vault program
- decide whether to keep the current thin dashboard or add a fuller read model

## Recommended Return Point

When we come back to multisig after firmware:

1. deploy the vault program to mainnet with `./scripts/deploy_pq_multisig_vault.sh`
2. run one internal FalconOnly `2-of-3` mainnet canary with `./scripts/run_pq_multisig_ops_mainnet_hardware_canary.sh`
3. extend that canary set to SOL, SPL, cancel/close, config update, and replace-member+accept on mainnet
