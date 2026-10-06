# Falcon 2-of-3 Multisig Vault Architecture

Note: this document captures the broader architecture direction. The shipped
FalconOnly v1 deliberately narrows the public surface: generic CPI bundle
proposals are reserved/disabled, and proposal close refunds return to the vault
PDA rather than to an arbitrary closer-selected receiver.

## Summary

The right next primitive is **not** "Squads plus one Falcon signer".
It is a new program-owned vault that accepts approvals from **three
independent Falcon policy wallets** and executes once **two of the three**
have approved the same proposal.

The important design choice is:

- reuse the existing **1-of-1 Falcon policy wallet** we already have
- make the new multisig program aggregate approvals from those wallets
- do **not** re-implement Falcon verification inside the multisig first

That gives us a cleaner path:

1. keep the current working Falcon/Vector wallet as the member primitive
2. add a new `pq_multisig_vault` program above it
3. later add a richer "Squads replacement" UI and transaction model on top

## What Becomes PQ And What Does Not

This design makes the **vault authority path** post-quantum:

- a relayer cannot move vault assets without threshold member approvals
- stake, SPL, SOL, and protocol actions are all gated by Falcon-controlled
  member wallets

What does **not** become post-quantum is Solana's outer transport layer:

- the relayer fee payer is still a normal Solana signer
- blockhashes, fee payment, and packet submission are still classical Solana

That is acceptable for this design because the fee payer is only transport.
If a relayer key is compromised, the attacker can spend relayer SOL, but they
still cannot move vault-controlled assets without the Falcon-gated member
approvals.

## Why This Is The Right Layering

We already proved the current Falcon policy wallet can:

- send SOL
- transfer SPL tokens
- delegate stake
- execute compact swaps

That means we already have a working **post-quantum 1-of-1 authority
primitive**.

A 2-of-3 multisig does not need a brand-new on-chain Falcon verifier if each
member is already a Falcon-gated policy wallet. The multisig can simply trust
"member PDA X approved this proposal" and count approvals.

So the architecture is:

- **member auth layer**: existing `vector_falcon512` policy wallets
- **vault layer**: new multisig vault program
- **app layer**: later "Squads replacement" UX on top

## Chosen Primitive

Each multisig member is a **Vector PDA** backed by its own Falcon key:

- `member_a_vector_pda`
- `member_b_vector_pda`
- `member_c_vector_pda`

Each of those member wallets keeps its own:

- Falcon identity
- Falcon prepared pubkey state
- Vector nonce
- independent approval history

The new multisig vault stores those three member PDA addresses and a
threshold of `2`.

The vault itself is another PDA:

- `vault_pda`

That vault PDA is the authority for:

- SOL held by the vault
- SPL token accounts owned by the vault
- stake accounts whose staker/withdrawer is the vault PDA
- protocol positions whose authority/delegate is the vault PDA

## Core Authorization Model

The approval chain is:

1. A member device signs a Vector `advance` digest with Falcon.
2. That `advance` authorizes a `passthrough(...)`.
3. The passthrough performs a CPI into the multisig vault program.
4. The multisig vault sees the calling member PDA as a signer and records
   approval.
5. After `2-of-3` member approvals are recorded, anyone can execute the
   proposal.
6. The vault program replays the stored instruction bundle under
   `vault_pda` signer seeds.

The multisig program therefore does **not** need to verify Falcon itself in
v1. It only needs to verify:

- this signer is one of the configured member PDAs
- this member has not already approved
- this approval matches the exact stored proposal hash
- the proposal is still active

## Programs

### 1. Existing Member Program

Keep using the deployed Falcon Vector program:

- `FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU`

This remains the **member policy wallet** primitive.

### 2. New Vault Program

Add a new program, conceptually:

- `pq_multisig_vault`

This program does:

- vault config
- proposal storage
- approval counting
- proposal execution
- config changes through self-governed proposals

It does **not** do Falcon signature verification in v1.

## Accounts

## Vault PDA

Single PDA that acts as both:

- config/state account
- signer authority for downstream CPI

Suggested seeds:

```text
["pq_vault", vault_seed]
```

Suggested fields:

```text
version: u8
bump: u8
threshold: u8              // 2
member_count: u8           // 3
config_version: u64        // increments on member/threshold change
next_proposal_id: u64
timelock_slots: u64        // optional, can be 0 in v1
proposal_ttl_slots: u64    // default proposal expiry window
reserved: [u8; ...]
members: [MemberRecord; 3]
```

Suggested `MemberRecord`:

```text
member_pda: Pubkey         // Vector PDA for this member
enabled: u8
weight: u8                // 1 in v1
reserved: [u8; ...]
```

Optional metadata for UI, not authorization-critical:

- member label
- bound Ed25519 hardware wallet pubkey
- Falcon identity hash

## Proposal PDA

Suggested seeds:

```text
["proposal", vault_pda, proposal_id_le]
```

Suggested fields:

```text
version: u8
state: u8                 // Active, Executed, Cancelled, Expired
proposal_id: u64
vault: Pubkey
config_version: u64
created_slot: u64
not_before_slot: u64
expires_at_slot: u64
proposer_member_index: u8
approval_bitmap: u8       // 3 bits used in 2-of-3 v1
approval_count: u8
action_kind: u8           // CpiBundle, ReplaceMember, ChangeThreshold, etc.
action_hash: [u8; 32]
tx_account: Pubkey
executed_slot: u64
reserved: [u8; ...]
```

## Proposal Transaction Account

Store the exact execution bundle separately so proposals stay small and
fixed-size.

Suggested seeds:

```text
["proposal_tx", proposal_pda]
```

Store a canonical serialized instruction bundle.

For v1, use a straightforward layout:

```text
num_instructions: u16
for each instruction:
  program_id: [u8; 32]
  num_accounts: u16
  for each account:
    pubkey: [u8; 32]
    flags: u8              // writable, signer_requested
  data_len: u16
  data: [u8; data_len]
```

This is intentionally close to Solana instruction structure, but stored
on-chain and proposal-hashed once.

`action_hash = sha256(canonical_proposal_tx_bytes)`

## Instruction Set

## 1. `initialize_vault`

Creates the vault PDA with:

- three member Vector PDA addresses
- threshold `2`
- initial config version `0`

This is a one-time setup instruction.

## 2. `create_proposal_from_member`

Called **via CPI** from one member Vector wallet.

Behavior:

- verifies caller signer is one of the configured member PDAs
- creates `proposal_pda`
- creates `proposal_tx_pda`
- stores the transaction bundle
- computes and stores `action_hash`
- records the proposer's approval immediately

This means proposal creation is also the first approval.

Top-level transaction shape:

```text
[member_vector_advance]
[member_vector_passthrough(multisig.create_proposal_from_member)]
```

## 3. `approve_proposal_from_member`

Called **via CPI** from another member Vector wallet.

Behavior:

- verifies caller signer is one of the configured member PDAs
- verifies proposal is still active
- verifies proposal `config_version` matches current vault config
- verifies provided `expected_action_hash` matches stored hash
- sets the member's approval bit
- increments `approval_count` if this is a new approval

Top-level transaction shape:

```text
[member_vector_advance]
[member_vector_passthrough(multisig.approve_proposal_from_member)]
```

## 4. `revoke_proposal_from_member`

Optional in v1 but useful.

Called via CPI from a member Vector wallet before execution.

Behavior:

- clears that member's approval bit
- decrements approval count

## 5. `execute_proposal`

Permissionless.

Behavior:

- verifies proposal is active
- verifies `approval_count >= threshold`
- verifies not expired / not before
- loads `proposal_tx_pda`
- verifies remaining accounts match stored transaction bundle
- replays the stored CPIs with `vault_pda` signer seeds
- marks proposal executed

Top-level transaction shape:

```text
[execute_proposal]
```

The executor can be any relayer. No Falcon signature is needed at execution
time because Falcon approvals have already been recorded.

## 6. `cancel_proposal`

Optional in v1.

Reasonable rules:

- proposer can cancel before threshold is reached
- any member can cancel an expired proposal
- or cancellation itself requires threshold approval if already approved

## 7. `close_proposal`

Permissionless cleanup after:

- executed
- cancelled
- expired

Closes the proposal accounts and refunds rent.

## 8. Config Change Actions

Do **not** add a separate admin key.

Config changes should be self-governed proposal kinds:

- `ReplaceMember { slot, new_member_pda }`
- `ChangeThreshold { new_threshold }`
- `SetTimelock { new_timelock_slots }`

These are executed by the vault program itself once threshold approvals are
met. In the hardened rotation flow, `ReplaceMember` installs a *pending* member
and the incoming member must later finalize the swap with
`accept_pending_member`. That opt-in step prevents the vault from silently
rotating into an unowned or dead member PDA.

That is the path from "vault primitive" to "Squads replacement".

## Exact Approval Semantics

The multisig does not need a second custom Falcon digest in v1.

The member already signs the Vector `advance` digest, and that digest commits
to the exact `passthrough(multisig.approve_...)` instruction being executed.

Still, the multisig approval instruction should carry:

```text
proposal_id
expected_action_hash
```

and the program should verify both.

That gives us:

- Falcon signature commits to the specific approval CPI
- multisig program verifies the proposal hash being approved
- proposal contents are immutable after creation

## Execution Semantics

During `execute_proposal`, the vault program should:

1. load the stored compiled instruction bundle
2. verify the caller supplied remaining accounts in the exact stored order
3. verify writable expectations
4. ignore or reject any requested signer except the vault PDA itself
5. promote only `vault_pda` to signer during `invoke_signed`

This mirrors the safety model of the current Vector passthrough:

- arbitrary CPI is allowed
- signer escalation is not
- only the program-owned vault PDA may sign

## Proposal Hash

Suggested domain-separated hash:

```text
sha256(
  "UNRUGGABLE_PQ_MULTISIG_PROPOSAL_V1" ||
  vault_pda ||
  config_version_le ||
  proposal_id_le ||
  action_kind_u8 ||
  not_before_slot_le ||
  expires_at_slot_le ||
  sha256(canonical_proposal_tx_bytes)
)
```

This hash should be stored in the proposal header and echoed in approval
instructions.

## Replay Protection

Replay protection exists at three layers:

### Member Layer

Each member Vector wallet advances its own nonce every approval tx.

So a member Falcon approval tx cannot be replayed through the member wallet.

### Proposal Layer

Each proposal stores:

- approval bitmap
- state
- action hash

So a member cannot approve the same proposal twice, and an executed proposal
cannot be replayed.

### Vault Config Layer

Each proposal stores the vault `config_version`.

If the member set or threshold changes before execution:

- old proposals stop being valid

This avoids approvals surviving a member-rotation event.

## 2-of-3 Flow

### Step 1. Member A proposes

Member A's hardware wallet signs a Vector `advance` that authorizes:

```text
multisig.create_proposal_from_member(proposal_tx, expected_action_hash, ...)
```

Effects:

- proposal created
- transaction bundle stored
- approval bitmap becomes `001`

### Step 2. Member B approves

Member B signs its own Vector `advance` authorizing:

```text
multisig.approve_proposal_from_member(proposal_id, expected_action_hash)
```

Effects:

- approval bitmap becomes `011`
- threshold `2` is now met

### Step 3. Relayer executes

Any relayer submits:

```text
multisig.execute_proposal(...)
```

Effects:

- vault PDA signs the stored CPI bundle
- proposal becomes executed

Member C never needs to sign.

## What Assets This Vault Can Control

Anything that can already be controlled by a program PDA:

- SOL
- SPL token accounts
- stake authorities
- DEX positions and delegates
- lending positions
- custom protocol authorities

This means the multisig vault is not limited to simple transfers. It can own
the same protocol surface the current single-member Falcon wallet already
owns.

## Why This Is Better Than "Falcon Inside Squads"

Trying to retrofit standard Squads misses the security boundary.

What we want is:

- approvals rooted in Falcon-controlled policy wallets
- threshold counted on-chain
- vault execution under a program PDA

That is a new primitive, not a patched classical multisig.

The right mental model is:

- **current Vector Falcon wallet** = PQ 1-of-1 policy wallet
- **new vault program** = PQ T-of-N policy vault
- **future app layer** = Squads-like UX on top

## Why Not Direct Multi-Falcon Verify In The Vault First

That is possible, but it is the wrong first step.

A direct multi-Falcon vault would need to:

- store three Falcon prepared pubkeys itself
- verify Falcon signatures in the vault program
- define a brand-new approval transcript
- duplicate logic we already have working in the member wallet primitive

The layered approach is better because it:

- reuses the working Falcon member wallet
- reuses the working hardware flow
- reuses the current deployed Falcon verifier path
- keeps the new vault program focused on threshold logic and execution

## V1 Scope

The first build should do only this:

1. initialize a 2-of-3 vault with three member Vector PDAs
2. create proposal from member A
3. approve from member B
4. cancel and close a proposal asynchronously
5. execute a simple SOL withdraw from the vault
6. execute an SPL transfer from the vault's canonical ATA for that mint

That is enough to prove the primitive.

## V2 Scope

After that:

- member rotation with explicit acceptance
- threshold changes
- approval revocation
- richer metadata
- UI layer
- batch proposal explorer
- protocol-specific helpers

## Bottom Line

The exact architecture should be:

- **Falcon policy wallet per member**
- **new PQ multisig vault program above those wallets**
- **2-of-3 approvals recorded by member Vector-PDA CPI**
- **permissionless execution from a vault PDA once threshold is reached**

That gives us a real post-quantum multisig primitive on Solana without
depending on classical member signatures and without rebuilding the working
Falcon verifier path from scratch.
