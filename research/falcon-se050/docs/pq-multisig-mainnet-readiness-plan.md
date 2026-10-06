# PQ Multisig Mainnet Readiness Plan

Date: 2026-06-08

## Goal

Ship the current Falcon-gated `pq_multisig_vault` as a real operator-friendly
mainnet product, not just a localnet primitive.

That means two things before mainnet:

1. operators need proposal discovery and lifecycle visibility without reading
   raw accounts manually
2. we need an explicit cost model for deployment, vault setup, and ongoing
   proposal operations

## Product Boundary

The current cryptographic and on-chain boundary is already good:

- member approvals are Falcon-gated through Vector PDAs
- vault execution is threshold-gated onchain
- relayers are transport only

What is missing is the operator layer:

- proposal inbox
- decoded proposal details
- action status (`needs approval`, `executable`, `cancelled`, `closable`)
- member state visibility
- pending-member acceptance visibility

Pre-mainnet, that should be added as a read-heavy operator system sitting on top
of the existing CLI and vault program, not by moving signing into a browser.

## Recommended Pre-Mainnet Architecture

### 1. Keep signing in the CLI / device flow

Do not put signing into a browser or backend signer before mainnet.

The operator UX should stay:

- **read / discover / filter in the dashboard**
- **approve / revoke / cancel / accept in the local operator CLI**

The CLI now already covers the minimal discovery path with direct onchain
`list-proposals` scanning and queue filters, so the dashboard can be built as a
read-heavy convenience layer rather than a blocker for basic operator use.

The dashboard should generate an action manifest or deep-link payload that the
local operator tool can consume. This keeps the current security boundary
intact.

### 2. Add a dedicated read model

The onchain source of truth remains:

- `vault` accounts
- `proposal` accounts
- `proposal_tx` accounts
- member Vector PDA accounts

But operators should not discover proposals by raw RPC inspection. We need a
small indexer + API that derives:

- vault summaries
- proposal summaries
- decoded action previews
- lifecycle events
- member readiness
- pending tasks

### 3. Use a two-layer indexer

#### A. Backfill / reconciliation layer

Use periodic RPC reads to rebuild canonical state:

- `getProgramAccounts` for `pq_multisig_vault` accounts
- filtered by account size to separate:
  - vault accounts
  - proposal accounts
  - proposal-tx accounts
- `getMultipleAccounts` to hydrate related objects in batches

This is the durable recovery path and should be treated as canonical state.

#### B. Realtime layer

Use websocket subscriptions for low-latency UX:

- `programSubscribe` on the vault program for proposal / vault changes
- `accountSubscribe` on watched member Vector PDAs when the operator is
  looking at a specific vault or member set

Realtime events should update an optimistic cache, but canonical reconciliation
should still run periodically so websocket drops never wedge the dashboard.

## Indexer Data Model

Recommended tables / collections:

### `vaults`

- `vault_pubkey`
- `config_version`
- `threshold`
- `member_count`
- `proposal_ttl_slots`
- `timelock_slots`
- `pending_member_index`
- `pending_member_pda`
- `next_proposal_id`
- `slot_last_seen`

### `vault_members`

- `vault_pubkey`
- `member_index`
- `member_vector_pda`
- `enabled`
- `weight`
- `is_pending`
- `active_from_config_version`

### `proposals`

- `vault_pubkey`
- `proposal_id`
- `proposal_pubkey`
- `proposal_tx_pubkey`
- `state`
- `action_kind`
- `action_hash`
- `config_version`
- `created_slot`
- `not_before_slot`
- `expires_at_slot`
- `executed_slot`
- `proposer_member_index`
- `approval_bitmap`
- `approval_count`
- `decoded_summary_json`

### `proposal_events`

- `vault_pubkey`
- `proposal_id`
- `event_type`
  - `created`
  - `approved`
  - `revoked`
  - `cancelled`
  - `executed`
  - `closed`
  - `accepted_pending_member`
- `member_index` nullable
- `tx_signature`
- `slot`

### `vault_balances`

Derived and refreshed periodically:

- SOL balance
- canonical ATA balances by mint
- optional stake account list and delegated balances

## API Surface

Pre-mainnet, keep the API read-only plus manifest generation.

### Read endpoints

- `GET /vaults`
- `GET /vaults/:vault`
- `GET /vaults/:vault/proposals`
- `GET /vaults/:vault/proposals?state=active`
- `GET /vaults/:vault/proposals?queue=needs_approval`
- `GET /vaults/:vault/proposals?queue=executable`
- `GET /vaults/:vault/proposals?queue=closable`
- `GET /vaults/:vault/proposals/:proposalId`
- `GET /vaults/:vault/activity`
- `GET /members/:vectorPda`

### Derived queue endpoints

These are important because they make the product feel like a real multisig:

- `needs_approval`
  proposals active, not expired, not executed, and not yet approved by the
  current member
- `executable`
  proposals active, threshold met, `slot >= not_before_slot`
- `closable`
  proposals cancelled / executed / expired with both proposal accounts still
  open
- `needs_acceptance`
  vault has a pending member and the viewing member is that pending member
- `expiring_soon`
  active proposals with `expires_at_slot - current_slot <= N`

### Action manifest endpoints

No remote signing pre-mainnet. Instead expose:

- `POST /vaults/:vault/proposals/:proposalId/build-approve-manifest`
- `POST /vaults/:vault/proposals/:proposalId/build-revoke-manifest`
- `POST /vaults/:vault/proposals/:proposalId/build-cancel-manifest`
- `POST /vaults/:vault/proposals/:proposalId/build-execute-manifest`
- `POST /vaults/:vault/build-create-manifest`
- `POST /vaults/:vault/build-accept-manifest`

Each manifest should contain the exact `run_pq_multisig_ops.sh` arguments plus
the decoded proposal summary so the local CLI / desktop bridge can execute it.

## Dashboard UX

### Vault List

Show:

- vault name / label
- threshold
- member set
- active proposal count
- executable proposal count
- pending member acceptance banner
- vault balances

### Proposal Inbox

Default tabs:

- `Needs My Approval`
- `Executable`
- `Closable`
- `All Active`
- `History`

Each row should show:

- proposal id
- action kind
- decoded action summary
- proposer
- approval count / threshold
- not-before countdown
- expiry countdown
- latest lifecycle event

### Proposal Detail

Must show:

- exact action summary
- action hash
- config version
- member approval matrix
- tx account pubkey
- decoded payload fields
- lifecycle events
- action buttons that generate the local CLI manifest

### Pending Member UX

This must be first-class.

If a vault has:

- `pending_member_index != NO_PENDING_MEMBER_INDEX`

show:

- who is being replaced
- who the incoming member is
- whether acceptance has happened
- a dedicated `Accept Membership` call-to-action for the incoming member

### Closable Proposal UX

Do not hide this. A production multisig needs cleanup queues.

The dashboard should surface:

- cancelled proposals that still occupy rent
- executed proposals that still occupy rent
- expired proposals that need closure

## Rollout Plan Before Mainnet

### Phase 0: Current state

Already done:

- hardened async lifecycle
- revoke / cancel / close
- acceptance-based rotation
- localnet operator e2e

### Phase 1: Read-only operator backend

Build next:

- indexer + Postgres (or SQLite for very small internal-only use)
- read API
- reconciler
- proposal decoder service

Acceptance criteria:

- list vaults
- list proposals
- show decoded proposals
- show executable / closable queues

### Phase 2: Operator dashboard

Build next:

- web UI for vault list and proposal inbox
- proposal detail page
- member activity / status page
- manifest generation UX

Acceptance criteria:

- no operator needs raw RPC or explorer to find a proposal
- every lifecycle action can be initiated from the UI and handed off to the
  local CLI

### Phase 3: Internal mainnet canary

Deploy the vault program to mainnet and run:

- one internal `2-of-3` vault
- one SOL withdraw proposal
- one SPL transfer proposal
- one cancel / close lifecycle
- one config update
- one replace-member + accept flow

Acceptance criteria:

- indexer recovers correctly from restart
- websocket drops are harmless because reconciliation catches up
- operators can discover and act on proposals only through the dashboard + CLI

### Phase 4: Production hardening

Before external use:

- alerts on websocket lag and indexer drift
- alerts on executable proposals older than `N` minutes
- alerts on pending members not accepted after `N` hours
- alerts on closable proposal backlog
- backup / restore for the indexer DB
- explicit relayer key management and runbooks

## Mainnet Cost Estimate

All figures below were measured against your mainnet RPC on 2026-06-08 using
the current built artifacts.

### 1. Program deployment

#### Incremental cost for `pq_multisig_vault` only

- built artifact size: `62,000` bytes
- peak deploy requirement: `0.86601888 SOL`
  - temporary buffer: `0.43241088 SOL`
  - ProgramData: `0.43246656 SOL`
  - program account: `0.00114144 SOL`
- steady locked after deploy: `0.43360800 SOL`

#### Full stack cost if you redeploy both programs yourself

- existing Vector Falcon program steady locked: `0.92574264 SOL`
- full stack steady locked (`vector_falcon512 + pq_multisig_vault`):
  `1.34596656 SOL`
- full stack peak during deployment:
  `2.68953888 SOL`

### 2. One-time live vault footprint

For a new production `2-of-3` vault using three fresh member Vector accounts:

- `3 x` member Vector PDAs: `3 * 0.00847728 = 0.02543184 SOL`
- vault state account: `0.00244992 SOL`
- base live footprint before token accounts: `0.02788176 SOL`

If the vault will operate with two canonical SPL token accounts open:

- `2 x` token accounts: `2 * 0.00203928 = 0.00407856 SOL`
- base live footprint including two vault ATAs: `0.03196032 SOL`

### 3. Temporary per-proposal storage

These are deposits while a proposal is open. They are recoverable when the
proposal is closed, and the close refund returns to the vault PDA rather than
to an arbitrary caller-selected receiver.

- SOL withdraw proposal:
  - proposal account: `0.00200448 SOL`
  - proposal-tx account: `0.00144768 SOL`
  - total: `0.00345216 SOL`
- SPL transfer proposal total: `0.00368184 SOL`
- config update proposal total: `0.00329208 SOL`
- replace-member proposal total: `0.00340344 SOL`

### 4. Transaction fee floor

With no priority fee, the current Solana base fee is `5,000` lamports per
signature, and our relayer transactions are single-signer transactions. So the
fee floor is:

- create + approve + execute (`2-of-3`): `0.000015 SOL`
- create + 2 approvals + execute + accept: `0.000025 SOL`
- revoke + cancel + close cleanup sequence: `0.000015 SOL`

Priority fees are extra and depend on the compute budget instructions you
request.

## Recommendation

Before mainnet:

1. build the read-only indexer + proposal discovery API
2. ship the dashboard with manifest-based local signing
3. deploy `pq_multisig_vault` to mainnet
4. run one internal `2-of-3` canary vault

That is the shortest path from “working localnet primitive” to a production
operator-grade post-quantum multisig product.
