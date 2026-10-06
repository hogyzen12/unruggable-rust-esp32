# Falcon Mainnet Launch Checklist

Date: 2026-06-15

## Goal

Get the Falcon hardware wallet + FalconOnly multisig stack into a clean
**internal mainnet canary** state, with a stable device that retains keys, a
deployed `pq_multisig_vault`, a repeatable operator flow, and a repo state that
is publishable to GitHub.

This is the execution checklist, not the deep architecture doc.

## Current Boundary

What is already true:

- hardware Falcon signing works on the real Unruggable V2.0 board
- host PIN flow works end to end with physical button confirmation
- localnet FalconOnly multisig works end to end
- localnet hardware-backed Falcon multisig member flow works end to end
- the first mainnet Vector/Falcon wallet canaries already work
- `pq_multisig_vault` mainnet deployment and first mainnet canary are now
  scripted

What is **not** yet claimed here:

- public consumer-ready hardware wallet UX
- audited production release for meaningful external TVL
- final Tropic-backed key protection model

So the target is:

- **internal mainnet-ready**
- **GitHub-publishable**
- **clean operator runbook**

## Phase 0: Freeze The Canonical Canary Device

Pick one canonical hardware wallet for the first mainnet vault canary and stop
wiping it after this phase.

### Checklist

- [ ] flash the exact firmware build you intend to keep
- [ ] verify `BOARD=dual-element-se050`
- [ ] set the real PIN you want to keep on the device
- [ ] initialize SE050 wallet key material
- [ ] initialize Falcon/PQC state
- [ ] record:
  - [ ] SE050 pubkey
  - [ ] hardware wallet pubkey
  - [ ] Falcon pubkey
  - [ ] Vector identity
  - [ ] Vector PDA
- [ ] verify:
  - [ ] `PQC_SIGN_VECTOR_DIGEST`
  - [ ] `SIGN`
  - [ ] `SIGN_HYBRID`
- [ ] do **not** final-wipe the device
- [ ] fund the hardware wallet with small mainnet canary balances

### Recommended command

Use the hardware harness but leave the device configured:

```bash
./scripts/run_hybrid_harness.sh \
  --port /dev/cu.usbmodem1101 \
  --pin <YOUR_PIN> \
  --initial-wipe \
  --sample-tx-b64 aGVsbG8= \
  --no-final-wipe
```

Success criteria:

- `AUTH_MODE=PIN`
- `KEY_STATE=READY`
- `PQC_STATE=READY`
- device remains configured at the end

## Phase 1: Firmware Release Candidate Gate

Before using that device for the first mainnet vault, treat the firmware as a
release candidate.

### Checklist

- [ ] host PIN setup/unlock path verified
- [ ] on-device PIN entry path manually verified
- [ ] button confirmation verified for:
  - [ ] PIN setup
  - [ ] Falcon init
  - [ ] Falcon digest sign
  - [ ] Ed25519 sign
  - [ ] hybrid sign
  - [ ] wipe
- [ ] screensaver / idle screen visually acceptable
- [ ] top-row display artifact resolved or accepted
- [ ] no unexpected auto-approval without button hold
- [ ] no wipe as part of final verification

### Go / no-go

If any of these fail, do **not** move to mainnet vault deployment yet.

## Phase 2: Multisig Program Release Candidate Gate

This is the gate for `pq_multisig_vault`.

### Checklist

- [ ] FalconOnly v1 enforced on-chain
- [ ] hybrid/classical member init rejected
- [ ] close refunds return to the vault PDA
- [ ] generic CPI bundle reserved/disabled in v1
- [ ] localnet async flow still green:
  - [ ] create
  - [ ] approve
  - [ ] revoke
  - [ ] cancel
  - [ ] close
  - [ ] execute
  - [ ] config update
  - [ ] replace member
  - [ ] accept pending member
- [ ] localnet hardware-backed member proof still green
- [ ] operator CLI can discover proposals cleanly

### Current command surface

- local async ops proof:

```bash
./scripts/run_pq_multisig_ops_localnet_quickstart.sh
```

- hardware-backed local proof:

```bash
./scripts/run_pq_multisig_ops_localnet_hardware_quickstart.sh \
  --port /dev/cu.usbmodem1101
```

## Phase 3: Mainnet Deployment Preparation

Create the exact deployment materials before touching mainnet.

### Checklist

- [ ] generate / choose the dedicated `pq_multisig_vault` program keypair
- [ ] decide upgrade authority
- [ ] decide fee payer for deployment
- [ ] build the SBF artifact
- [ ] record:
  - [ ] program keypair path
  - [ ] expected program id
  - [ ] artifact size
  - [ ] deployment cost budget
- [ ] ensure deploy wallet holds enough SOL

### Cost baseline

Current measured estimate for `pq_multisig_vault` only:

- built artifact size: `62,000` bytes
- peak deploy requirement: `0.86601888 SOL`
- steady locked after deploy: `0.43360800 SOL`

## Phase 4: Deploy `pq_multisig_vault` To Mainnet

### Checklist

- [ ] deploy with explicit program keypair
- [ ] confirm on-chain program id matches expectation
- [ ] confirm upgrade authority is correct
- [ ] record deployment tx + `solana program show`

### Command

```bash
./scripts/deploy_pq_multisig_vault.sh \
  --program-keypair-file <path-to-program-keypair.json>
```

Success criteria:

- deploy command succeeds
- `solana program show` resolves the new program
- recorded vault program id is the one used by the operator CLI

## Phase 5: First Internal Mainnet FalconOnly Vault Canary

This is the first real end-to-end target:

- hardware wallet signs Falcon approvals
- CLI submits member actions
- deployed mainnet `pq_multisig_vault` executes threshold-gated actions

### Checklist

- [ ] hardware device left configured from Phase 0
- [ ] hardware canary wallet funded
- [ ] fee payer funded
- [ ] run first scripted hardware-backed canary
- [ ] record:
  - [ ] hardware member PDA
  - [ ] fixture PDAs
  - [ ] vault PDA
  - [ ] create / approve / execute / close txs

### Command

```bash
./scripts/run_pq_multisig_ops_mainnet_hardware_canary.sh \
  --port /dev/cu.usbmodem1101 \
  --vault-program-id <deployed_vault_program_id>
```

### Canary scope

This script currently proves:

- one hardware-authored SOL withdraw
- one fixture-authored SOL withdraw approved by the hardware member
- close of both executed proposals

## Phase 6: Expand Mainnet Action Coverage

After the first SOL canary succeeds, widen the action set.

### Checklist

- [ ] SPL transfer on mainnet
- [ ] explicit cancel / close lifecycle on mainnet
- [ ] config update on mainnet
- [ ] replace-member + accept on mainnet
- [ ] at least one vault with more than one real hardware member

## Phase 7: Operator Readiness

The minimum operator story before external users should be:

- CLI works
- proposal discovery works
- read-only dashboard works
- manifests / handoff are clean

### Checklist

- [ ] `list-proposals` works against mainnet vault
- [ ] `show-proposal` works against mainnet vault
- [ ] alias file checked and documented
- [ ] dashboard can read the mainnet vault state
- [ ] operator runbook written for:
  - [ ] create
  - [ ] approve
  - [ ] revoke
  - [ ] cancel
  - [ ] close
  - [ ] execute
  - [ ] accept pending member

## Phase 8: GitHub Publication Checklist

Before publishing, make the repo safe and understandable.

### Secrets / sensitive material

- [ ] no real private key JSONs committed
- [ ] no funded canary keypairs committed
- [ ] no API keys committed
- [ ] no private RPC URLs committed unless intentional
- [ ] no personal temp files or machine-specific junk committed

### Repo hygiene

- [ ] README reflects current mainnet path
- [ ] firmware path and multisig path both documented
- [ ] active scripts are documented
- [ ] checkpoints kept, but clearly marked as historical
- [ ] stale experiments are either archived or removed
- [ ] sample alias/config files are safe to publish

### Publication artifacts

- [ ] add a clear repo status note:
  - [ ] what is production-ready for internal canaries
  - [ ] what is still experimental
- [ ] add a release checklist issue or milestone
- [ ] add a short SECURITY.md or equivalent warning note
- [ ] add a “known limitations” section for deferred button/Tropic work

## Recommended Execution Order

If starting now, do it in this exact order:

1. Freeze one canonical hardware device and keep its keys.
2. Run the final firmware RC checklist on that exact device.
3. Build and deploy `pq_multisig_vault` to mainnet.
4. Run the scripted hardware-backed FalconOnly mainnet canary.
5. Expand mainnet action coverage beyond SOL withdraw.
6. Clean and publish the repo to GitHub.

## Recommended “Done” Definition

Call this **internal mainnet ready** when:

- one real device is stable and funded
- `pq_multisig_vault` is deployed on mainnet
- the hardware-backed mainnet canary passes
- at least one follow-up nontrivial vault action passes on mainnet
- operators can discover and act on proposals cleanly through CLI

Call this **publicly releasable** only after:

- repo cleanup is complete
- docs are publication-quality
- security review / audit plan is explicit
- deferred firmware trust improvements are clearly disclosed
