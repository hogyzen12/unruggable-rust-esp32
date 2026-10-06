# PQ Multisig Mainnet Tooling Ready

Date: 2026-06-15

## Goal

Bridge the gap between the proven hardware-backed localnet FalconOnly vault
flow and the first real mainnet deployment / canary flow.

## What Was Added

### 1. Vault deploy wrapper

New script:

- `scripts/deploy_pq_multisig_vault.sh`

Purpose:

- build `pq_multisig_vault` if needed
- deploy the SBF artifact with an explicit program keypair
- print the final on-chain program metadata via `solana program show`

### 2. Mainnet hardware canary wrapper

New script:

- `scripts/run_pq_multisig_ops_mainnet_hardware_canary.sh`

Purpose:

- reuse the existing `pq-multisig-ops` CLI for a real mainnet FalconOnly canary
- initialize one hardware-backed Falcon member plus two fixture Falcon members
- initialize a FalconOnly `2-of-3` vault on mainnet
- fund the vault with a small SOL canary amount
- prove:
  - hardware-created SOL withdraw proposal
  - fixture approval
  - execution
  - close
  - fixture-created SOL withdraw proposal
  - hardware approval
  - execution
  - close

This mirrors the already-proven localnet hardware-member flow, but targets a
deployed mainnet `pq_multisig_vault`.

## Validation

The new scripts were validated structurally:

- `bash -n scripts/deploy_pq_multisig_vault.sh`
- `bash -n scripts/run_pq_multisig_ops_mainnet_hardware_canary.sh`

No mainnet deployment or canary was executed in this checkpoint.

## Current Mainnet Path

1. Deploy `pq_multisig_vault`:

   ```bash
   ./scripts/deploy_pq_multisig_vault.sh \
     --program-keypair-file <path-to-program-keypair.json>
   ```

2. Run the first hardware-backed mainnet canary:

   ```bash
   ./scripts/run_pq_multisig_ops_mainnet_hardware_canary.sh \
     --port /dev/cu.usbmodemXXXX \
     --vault-program-id <deployed_vault_program_id>
   ```

## Why This Matters

At this point the missing work is no longer protocol invention. The core
FalconOnly vault, CLI workflow, and hardware-backed member path already exist.
What was missing was a clean mainnet operator path.

This checkpoint closes that tooling gap.

