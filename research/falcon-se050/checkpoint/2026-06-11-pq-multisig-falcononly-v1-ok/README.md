# PQ Multisig FalconOnly V1 OK

Date: 2026-06-11

## Goal

Harden `pq_multisig_vault` from an architectural Falcon-first design into an
actually enforced **FalconOnly v1** protocol before mainnet deployment.

That meant:

1. rejecting hybrid/classical member sets at vault initialization
2. requiring live Vector/Falcon member accounts at the approval boundary
3. enforcing the same rule on pending-member acceptance during rotation
4. proving the async CLI/operator flow still works end to end

## What Changed

- `initialize_vault` now rejects:
  - zero member addresses
  - duplicate member addresses
  - non-Vector member accounts
- member actions now require a live Vector/Falcon account owned by
  `FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU`:
  - `create_proposal_from_member`
  - `approve_proposal_from_member`
  - `revoke_proposal_from_member`
  - `cancel_proposal`
  - `accept_pending_member`
- the operator CLI now preflights FalconOnly init locally before it submits the
  transaction
- the localnet async operator E2E now includes a negative hybrid-init proof

## Validation

Compile checks:

```bash
cargo check --target aarch64-apple-darwin --manifest-path pq-multisig-vault/Cargo.toml
cargo check --target aarch64-apple-darwin --manifest-path tools/pq-multisig-ops/Cargo.toml
cargo check --target aarch64-apple-darwin --manifest-path tools/pq-multisig-harness/Cargo.toml
```

End-to-end localnet proof:

```bash
./scripts/run_pq_multisig_ops_localnet_quickstart.sh
```

## Verified Behavior

- hybrid/classical init was rejected before vault creation
- FalconOnly happy-path init succeeded immediately after
- the rebuilt validator and CLI ran on matching artifacts
- full async operator lifecycle still passed:
  - SOL withdraw
  - revoke + cancel + close
  - SPL transfer
  - config update to `3-of-3`
  - replace-member
  - accept-pending-member
  - rotated-out rejection
  - post-rotation `3-of-3` SOL withdraw

## Latest Localnet Txs

- SOL execute tx: `5sj4MLk8kui3vhYmeSq2CxG4fKNGYXEhewU6XfsJbZzF78UuGBsyqgQ3yaHXVNMwDe138d5UgGPzuaKRUcvWcP6c`
- Close tx: `2xRVc6aaGUfyvbCeCUZDXBTqPGTAMT9HhiVKk8hCuihbsUH3rMFPpf5ru5HU872uCeAvsm2PBypMKpjpovPAAvtN`
- SPL execute tx: `aRzS2EKEu6xni2BFY9heRWb6Nr13N3YRDF7AxCRHoA9hiYkvSQNsHVSLLysWnG2YZJ5vh1DV7Ng6488Z8GtRS7U`
- Config execute tx: `4ANbzYoY4hx6MyaiXSbksHs5c3cMN1fevGEBhWJVfYLiKbQtK4MYvSJYUfciZvjxK2NEeVkaFpPSpLMjHzdCj4MD`
- Replace execute tx: `4PNCKEeLH4ufGDvRa9hzYJ3Ya1k9F7uh4n9RpVARJYmTHa8qnvCLFSu5a7GFfBTnpqaJwbcfEkmixKTuhFV4Vmw8`
- Accept tx: `35gfct5cmK4C6zS7CETxkMNKov4WJE73jScPwFuLNh9nkQRuy36XLRa9RZ9fQnqx4yNomALkdiFCERFMcfqrqeFP`
- Post-rotation tx: `5u5YbpGVSikMDXTF9B5xZRCdJ9iNDyVw6jHC7fuYnGx2WZj23sVazKcyksy7HXUSibBMAgRM8GuJoFyG2mZS3A78`

## Final Vault State

- threshold: `3`
- member count: `3`
- config version: `2`
- next proposal id: `6`
- timelock slots: `4`
- proposal TTL slots: `768`
- pending member: `none`

## Release Boundary

FalconOnly v1 means:

- every member is a Vector/Falcon PDA
- hybrid Ed25519 + Falcon vaults are intentionally unsupported
- the vault is positioned as a **PQ asset-control primitive**, not a hybrid
  migration product
