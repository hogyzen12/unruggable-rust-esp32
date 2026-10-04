# ESP32 Solana Signer

Active firmware for the ESP32-S3 + SE050 signer on the Unruggable V1 board.

The firmware now also has a board-profile layer for newer dual-element-style
ESP32-S3 boards where the SE050 is populated but the Tropic footprint may still
be empty. The legacy `unruggable-v1` profile remains the default. The
`dual-element-se050` profile now maps directly to **Unruggable V2.0**:
`SE_ENA=15`, `SE_SDA=26`, `SE_SCL=21`, and the TFT on
`CS=42/SCK=41/SDA=40/DC=39/RST=38/BL=37`, with `USER_BTN=IO5`. The older
`tropic-multidevice-breadboard` repo remains only an architecture reference,
not the source of truth for the S3 PCB pin map.

## Active Surface

- `src/main.rs`: USB serial protocol, auth state, and top-level firmware flow
- `src/se050.rs`: SE050 T=1 over I2C transport, key management, and Ed25519 signing
- `src/falcon.rs`: pure-Rust FN-DSA/Falcon-512 keygen, sign, verify, and transcript helpers
- `src/display.rs`: ST7735 display UI and idle screensaver
- `src/bin/peripheral_probe.rs`: bring-up probe for SE050 I2C, Tropic SPI, and TFT wiring on board-profile-aware hardware
- `src/transaction_verify.rs`: lightweight transaction preview/parsing logic
- `scripts/`: flash/reset helpers
- `test_firmware.py`: serial smoke test
- `tools/hybrid-harness/`: host-side harness that verifies SE050 Ed25519, binding, and FN-DSA signatures
- `tools/vector-falcon-interop/`: host-only probe for `fn-dsa` vs `solana-falcon512` wire compatibility
- `tools/vector-device-harness/`: host-side bridge that talks to the live device, verifies binding, derives the Vector identity/PDA, and prints the `initialize` or digest-signing inputs for the Vector flow
- `tools/vector-onchain-harness/`: host-side RPC harness for the real `initialize -> fetch nonce -> advance` Vector flow
- `tools/pq-multisig-harness/`: host-side localnet harness for provisioning 3 Falcon/Vector members plus one `pq_multisig_vault`, then driving the hardened async multisig flow: `2-of-3` / `3-of-3` SOL withdraw, cancel + close lifecycle, vault-owned SPL transfer, self-governed config update, pending-member rotation plus acceptance, and post-rotation quorum proof
- `tools/pq-multisig-ops/`: operator-style host CLI for async multisig proposal creation, approval, revocation, cancellation, closure, execution, and pending-member acceptance through fixture-backed or device-backed Falcon/Vector members
- `docs/vector-falcon-transaction-model.md`: notes on the Vector-style Solana + Falcon transaction flow
- `docs/falcon-multisig-vault-architecture.md`: proposed 2-of-3 post-quantum multisig vault architecture layered on top of the existing Falcon/Vector policy wallet primitive
- `docs/pq-multisig-mainnet-readiness-plan.md`: pre-mainnet plan for operator indexing, proposal discovery UX, and current mainnet cost estimates for the PQ multisig product
- `docs/pq-multisig-falcon-status.md`: current status snapshot of the Falcon multisig work so it can be resumed cleanly after firmware work
- `docs/falcon-mainnet-launch-checklist.md`: execution checklist for freezing the canary hardware wallet, deploying `pq_multisig_vault`, running the first hardware-backed mainnet FalconOnly vault canary, and getting the repo ready for GitHub publication
- `docs/dual-element-board-review.md`: current review status of the newer SE050-first / Tropic-ready board bring-up, including the actual Unruggable V2.0 GPIO map
- `pq-multisig-vault/`: Solana workspace for the Falcon-gated multisig vault primitive, including a host-side instruction-builder crate plus an on-chain vault program with localnet-tested async proposal lifecycle, `2-of-3` / `3-of-3` SOL withdraw, canonical-ATA SPL transfer, config-update, and acceptance-based member rotation support
  - FalconOnly v1 is now enforced at the program boundary: hybrid/classical
    member sets are intentionally rejected, and every member must be a live
    Vector/Falcon PDA owned by `FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU`
- `buildnflash.md`: build and flashing commands
- `PATH_B_APDU_NOTES.md`: SE050 APDU implementation notes
- `SCH_Unruggable-V1.pdf`: board schematic

## Archived Material

Legacy firmware variants, experiments, helper bins, and stale docs/assets now live under `archive/`.
Nothing in `archive/` is part of the active build.

## Active Protocol

- `GET_INFO`, `GET_TIME`, `SET_TIME`, `SET_MODE:PIN:<pin>`, `UNLOCK:PIN:<pin>`, `WIPE_KEYS`
- `GET_PUBKEY`, `SE050_STATUS`, `SE050_REINIT`, `SIGN:<base64_serialized_solana_message>`
- `PQC_STATUS`, `PQC_INIT`, `PQC_GET_PUBKEY`, `PQC_GET_BINDING`
- `PQC_SIGN_VECTOR_DIGEST:<base64_32_byte_digest>`
- `SIGN_HYBRID:<base64_serialized_solana_message>`

`SIGN` remains the Solana-compatible path and returns `SIGNATURE:<base64_ed25519_sig>`.

`SIGN_HYBRID` signs the same Solana message twice:

- `SIGNATURE:<base64_ed25519_sig>` from the SE050
- `PQC_SIGNATURE:<base64_fn_dsa_sig>` from the ESP32
- `PQC_PUBKEY:<base64_fn_dsa_pubkey>`
- `PQC_BINDING_SIG:<base64_ed25519_binding_sig>`

The binding signature is the SE050 Ed25519 key signing a transcript that binds the SE050 public key to the FN-DSA public key generated on the ESP32.

`PQC_SIGN_VECTOR_DIGEST` is the Vector-compatible Falcon path. It signs a
host-supplied 32-byte digest with original Falcon transcript rules and returns:

- `PQC_VECTOR_SIGNATURE:<base64_falcon512_sig>`
- `PQC_VECTOR_IDENTITY:<base64_sha256_of_wire_pubkey>`
- `PQC_PUBKEY:<base64_falcon512_wire_pubkey>`

`GET_INFO` now also includes `BOARD=<profile>` so host tooling can tell which
board profile the firmware was built for.

## Test Harnesses

- `SIGNER_BOARD_PROFILE=dual-element-se050 cargo +esp build --bin peripheral_probe`
  Build the low-level board probe that checks the SE050 on I2C, the Tropic/TR01 footprint on SPI, and the TFT control/SPI pins using the active board profile.
- `python3 test_firmware.py`
  Basic serial smoke test for setup, unlock, PQC init, and optional signing.
- `./scripts/run_hybrid_harness.sh --port /dev/cu.usbmodemXXXX --pin 246810 --sample-tx-b64 <b64>`
  Host-side verifier that checks the returned Ed25519 signature, SE050 binding signature, Vector-compatible Falcon digest signature, and the FN-DSA signature from `SIGN_HYBRID`. On boards with `USER_BTN`, this harness now expects physical hold-to-confirm prompts during the sensitive actions.
- `./scripts/flash_and_hybrid_harness.sh --port /dev/cu.usbmodemXXXX --sample-tx-b64 <b64>`
  Flashes the firmware and immediately runs the full hybrid harness.
- `cargo +stable run --manifest-path tools/vector-falcon-interop/Cargo.toml`
  Host-only probe that proves `HASH_ID_ORIGINAL_FALCON` is accepted by `solana-falcon512`, while the current raw FN-DSA transcript is not.
- `./scripts/run_vector_device_harness.sh --port /dev/cu.usbmodemXXXX --pin 246810 [--payer <base58>] [--nonce-hex <64hex>]`
  Live-device bridge for the real Vector flow. It unlocks the device, verifies the SE050-to-Falcon binding, prints the Vector identity and PDA, prints the `initialize` instruction shape, and if you provide a current Vector nonce it will ask the ESP32 to sign the corresponding 32-byte Vector digest and print the `advance` instruction shape.
- `./scripts/run_vector_onchain_harness.sh --port /dev/cu.usbmodemXXXX --rpc-url <url> --mode full-empty --fee-payer-file ~/.config/solana/id.json`
  End-to-end host-side RPC harness for Vector `initialize` plus an empty `advance`, using the ESP32 for Falcon digest signing and a local keypair as the outer Solana fee payer.
- `./scripts/run_vector_onchain_harness.sh --port /dev/cu.usbmodemXXXX --rpc-url <url> --mode full-withdraw --withdraw-lamports 3000000 --fee-payer-file ~/.config/solana/id.json`
  End-to-end host-side RPC harness for a real value-moving Vector transaction: initialize if needed, top up the Vector PDA if needed, then submit `advance + passthrough(withdraw)` with the ESP32 producing the Falcon signature.
- `./scripts/run_vector_onchain_harness.sh --port /dev/cu.usbmodemXXXX --rpc-url <url> --mode full-spl-transfer --fee-payer-file ~/.config/solana/id.json`
  End-to-end host-side RPC harness for a real Falcon-authorized SPL token transfer: with no `--spl-mint`, it creates a temporary test mint; with `--spl-mint <mint>`, it uses an existing Tokenkeg SPL mint such as USDC, ensures the device-wallet ATA, Vector-owned source ATA, plus destination ATA exist, auto-bootstraps the Vector ATA from the hardware wallet’s Ed25519 ATA when possible, and otherwise prints the exact device wallet + ATA that still need funding before it submits `advance + passthrough(transfer_checked)`.
- `./scripts/run_vector_onchain_harness.sh --port /dev/cu.usbmodemXXXX --rpc-url <url> --mode full-stake-delegate --fee-payer-file ~/.config/solana/id.json`
  End-to-end host-side RPC harness for a real Falcon-authorized native stake delegation: initialize if needed, create a stake account whose staker and withdrawer are the Vector PDA, then submit `advance + passthrough(delegate_stake)` with the ESP32 producing the Falcon signature.
- `./scripts/run_vector_onchain_harness.sh --port /dev/cu.usbmodemXXXX --rpc-url <url> --mode full-raydium-cpmm-swap --swap-min-out <raw_out> --fee-payer-file ~/.config/solana/id.json`
  End-to-end host-side RPC harness for a real Falcon-authorized Raydium CPMM swap: initialize if needed, auto-discover or pin a Raydium CPMM pool, bootstrap the input mint from the hardware wallet ATA into a Vector-owned input ATA, then submit `advance + passthrough(swap_base_input)`. The final swap leg now compiles as a V0 transaction with the Raydium pool's lookup table, which is required on mainnet for the CPMM account footprint. The first canary defaults to `USDC -> USDT`, but the mode accepts arbitrary input/output mints plus an optional pinned `--raydium-cpmm-pool-id`. If the output mint is WSOL, `--swap-unwrap-sol` closes the Vector-owned WSOL ATA inside the same Falcon-authorized passthrough and delivers native SOL to the receiver.
- `./scripts/run_vector_onchain_harness.sh --port /dev/cu.usbmodemXXXX --rpc-url <url> --mode full-jupiter-router-swap --fee-payer-file ~/.config/solana/id.json`
  End-to-end host-side RPC harness for a real Falcon-authorized Jupiter router swap: initialize if needed, bootstrap the input mint from the hardware wallet ATA into a Vector-owned input ATA, then call Jupiter `/swap/v2/build`, compose the returned instructions plus lookup tables into `advance + passthrough(...)`, and submit the final swap leg as a V0 transaction. The harness reads `UNRUGGABLE_JUPITER_API_KEY` from the environment or from a `partner_secrets.local` file such as the sibling `../../unruggable-app/partner_secrets.local`.
- `./scripts/run_mainnet_falcon_e2e.sh --withdraw-receiver <pk> --stake-vote-account <pk>`
  Mainnet canary wrapper that runs the three proven Falcon flows in sequence from the hardware wallet model: `full-withdraw`, `full-stake-delegate`, and a tiny Raydium `USDC -> SOL` swap using the WSOL unwrap path. Defaults are intentionally conservative so the script is repeatable on a modestly funded canary wallet.
- `./scripts/build_vector_falcon_program.sh`
  Builds the local `vector/programs/falcon512` SBF program.
- `./scripts/start_vector_falcon_local_validator.sh --build --mint-keypair ~/.config/solana/id.json`
  Starts a local validator and loads the local Falcon Vector program under the chosen program ID.
- `./scripts/run_vector_localnet_onchain_harness.sh --port /dev/cu.usbmodemXXXX --fee-payer-file ~/.config/solana/id.json --mode full-empty`
  Convenience wrapper around the on-chain harness for a local validator at `http://127.0.0.1:8899`.
- `./scripts/build_pq_multisig_vault_program.sh`
  Builds the local `pq_multisig_vault` SBF program.
- `./scripts/start_pq_multisig_local_validator.sh --build --mint-keypair ~/.config/solana/id.json`
  Starts a local validator and loads both the Falcon Vector program and the local `pq_multisig_vault` program.
- `./scripts/run_pq_multisig_localnet_harness.sh`
  Provisions 3 synthetic Falcon/Vector members plus one vault on the local validator, then drives the hardened async lifecycle through the member Vector PDAs: a vault-owned SOL withdraw, a cancelled-and-closed proposal, a vault-owned SPL transfer, a self-governed config update, a pending-member replacement plus explicit acceptance, and a final post-rotation `3-of-3` SOL withdraw proving the rotated-out member is rejected while the rotated-in member can originate the next proposal.
- `./scripts/run_pq_multisig_ops.sh --mode <subcommand> ...`
  Operator-style wrapper around `tools/pq-multisig-ops` for proposal-by-proposal multisig use. It supports fixture-backed or serial-device-backed Falcon members, scans proposals directly from chain with `list-proposals`, and exposes the hardened async lifecycle directly: create, approve, revoke, cancel, close, execute, and `accept-pending-member`.
- `./scripts/run_pq_multisig_ops_localnet_e2e.sh`
  Localnet async operator E2E around the operator CLI. It provisions fixture members, initializes their Vector PDAs plus a vault, then proves the full async operator flow end to end: proposal discovery via `list-proposals`, `show-proposal`, SOL withdraw, revoke + cancel + close, canonical-ATA SPL transfer, config update to `3-of-3`, replacement with explicit acceptance, rotated-out rejection, and a final post-rotation `3-of-3` proposal.
  Before the successful vault init, it also proves FalconOnly v1 by attempting
  a hybrid/classical member set and confirming the vault init is rejected.
- `./scripts/run_pq_multisig_ops_localnet_quickstart.sh`
  One-command dev onboarding path. It starts a fresh local validator, waits for RPC readiness, runs the full CLI-based async operator E2E, then shuts the validator down automatically unless `--keep-validator` is set.
- `./scripts/run_pq_multisig_ops_localnet_hardware_e2e.sh --port /dev/cu.usbmodemXXXX`
  Localnet FalconOnly `2-of-3` proof with one real hardware Falcon member plus two fixture Falcon members. It prepares the device, initializes the device Vector PDA, initializes a FalconOnly vault, proves a hardware-authored proposal can be approved and executed, then proves a fixture-authored proposal can be approved by the real hardware member and executed.
- `./scripts/run_pq_multisig_ops_localnet_hardware_quickstart.sh --port /dev/cu.usbmodemXXXX`
  One-command hardware-member onboarding path. It starts the PQ validator, runs the full hardware-backed E2E above, then tears the validator down automatically.
- `./scripts/deploy_pq_multisig_vault.sh --program-keypair-file <path>`
  Builds and deploys the `pq_multisig_vault` SBF program to the selected cluster
  using the given program keypair, fee payer, and upgrade authority.
- `./scripts/run_pq_multisig_ops_mainnet_hardware_canary.sh --port /dev/cu.usbmodemXXXX --vault-program-id <pubkey>`
  Mainnet FalconOnly canary around the same operator CLI surface proven on
  localnet. It initializes one real hardware Falcon member plus two fixture
  Falcon members, creates a `2-of-3` vault, funds it with a small SOL canary,
  proves a hardware-authored SOL withdraw proposal, then proves a
  fixture-authored SOL withdraw approved by the real hardware member. Both
  executed proposals are closed so proposal rent returns to the vault PDA.
- `./scripts/run_pq_multisig_dashboard.sh`
  Thin local dashboard for vault/proposal discovery. It only reads chain state and prepares `--manifest-only` handoff JSON for the local CLI signer path; it does not sign or submit transactions from the browser.
- `./scripts/run_vector_onchain_harness.sh --port /dev/cu.usbmodemXXXX --mode probe-device-payer`
  No-RPC probe that asks the SE050 `SIGN` path to sign dummy Vector `initialize` and `advance` outer Solana messages.

## Current Vector Status

- `PQC_SIGN_VECTOR_DIGEST` on the ESP32 is working and verified against `solana-falcon512`.
- The new on-chain harness is ready for the immediate path of `local fee payer + ESP32 Falcon Vector authorization`.
- The local validator path now also supports a real Falcon-authorized lamport withdrawal via `full-withdraw`, using `advance + passthrough(withdraw)` and a host-funded top-up when the Vector PDA only holds its rent floor.
- The local validator path now also supports a real Falcon-authorized SPL token transfer via `full-spl-transfer`, using `advance + passthrough(transfer_checked)` from a Vector-owned token account.
- The local validator path now also supports a real Falcon-authorized native stake delegation via `full-stake-delegate`, using `advance + passthrough(delegate_stake)` against a stake account whose authorities are the Vector PDA.
- The on-chain harness now also has a first Falcon-authorized Raydium CPMM swap mode via `full-raydium-cpmm-swap`, built on the same `advance + passthrough(...)` pattern. On mainnet it compiles the final swap transaction as V0 and uses the Raydium pool LUT to fit under the transaction size cap. For WSOL outputs it can also unwrap to native SOL inside the same Falcon-authorized passthrough with `--swap-unwrap-sol`.
- The on-chain harness now also has a Falcon-authorized Jupiter router swap mode via `full-jupiter-router-swap`, built on the same `advance + passthrough(...)` pattern but using Jupiter `/swap/v2/build` for routed pricing and lookup-table data.
- The new `pq_multisig_vault` workspace now has a working localnet end-to-end flow across the hardened async lifecycle: members can create, approve, cancel, close, and execute proposals through the Falcon/Vector member path, and the vault PDA executes once threshold is reached.
- FalconOnly v1 is now enforced in `pq_multisig_vault`: init rejects zero,
  duplicate, or non-Vector members; create/approve/cancel/revoke/accept all
  require live Vector/Falcon member accounts, and hybrid vaults are
  intentionally unsupported in v1.
- The new `pq-multisig-ops` CLI and wrapper now prove the same hardened lifecycle in a proposal-style operator model instead of a monolithic test harness: separate members can create, approve, revoke, cancel, close, and execute asynchronously, and a pending replacement member must explicitly accept membership before the rotated set becomes active.
- The operator CLI can now also discover proposals directly from chain with `list-proposals`, including state, queue, and action-kind filters plus member-aware `needs-approval` views, so operators no longer need raw proposal IDs just to find pending work.
- The operator CLI now also supports `--json` for read flows, `--manifest-only` / `--manifest-file` for dashboard handoff into the local signing CLI, and alias-based operator ergonomics via `--aliases-file`, `--vault-alias`, `--member-alias`, and `--new-member-alias`.
- The operator CLI now also prints proposal queues in a compact table format and ships with checked-in bash/zsh completions under `scripts/completions/`.
- After the config update raises the vault to `3-of-3`, the localnet harness now replaces one member through a two-step `replace_member -> accept_pending_member` flow, proves the rotated-out member is rejected on the next approval attempt, and proves the rotated-in member can originate the next successful vault-owned SOL withdraw.
- The compact action encoding for `SPL transfer`, `config update`, and `replace_member` is now small enough to fit inside the member Vector `advance + passthrough(...)` envelope, and the SPL transfer path is bound to the vault's canonical ATA for that mint instead of letting the executor choose an arbitrary same-mint source account.
- The host harnesses now take `--vector-program-id` (or `VECTOR_PROGRAM_ID`) so they can target a local deployment instead of assuming the repo's default address.
- The Falcon Vector program is now deployed on mainnet at `FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU`.
- The first mainnet canaries have passed: `full-empty` initialized the Vector PDA and submitted an empty `advance`, and `full-withdraw` executed a real value-moving `advance + passthrough(withdraw)` against the deployed program.
- The current firmware does **not** yet accept Vector-style outer Solana messages through the SE050 `SIGN` path. The live `probe-device-payer` mode fails on both dummy `initialize` and dummy `advance` messages, so a true `SE050 outer tx signature + Falcon Vector authorization` flow still needs a firmware-side signing-path update.

## PQ Multisig Operator UX

The quickest dev bootstrap is:

```bash
./scripts/run_pq_multisig_ops_localnet_quickstart.sh
```

That starts the PQ validator, runs the CLI-first async operator E2E, proves
proposal discovery and approval queues, then shuts the validator down.

For the first real hardware-backed member proof:

```bash
./scripts/run_pq_multisig_ops_localnet_hardware_quickstart.sh \
  --port /dev/cu.usbmodem1101
```

That flow is now proven end to end with:

- a real hardware member creating proposal `0`
- a fixture member approving proposal `0`
- vault execution of proposal `0`
- a fixture member creating proposal `1`
- the real hardware member approving proposal `1`
- vault execution of proposal `1`

For named vault/member ergonomics, start from:

`[docs/pq-multisig-ops.aliases.example.json](/Users/hogyzen12/coding-project-folders/unruggable-rust-esp32/esp32-solana-signer/docs/pq-multisig-ops.aliases.example.json)`

Typical discovery flow:

```bash
./scripts/run_pq_multisig_ops.sh \
  --mode list-proposals \
  --aliases-file ./docs/pq-multisig-ops.aliases.example.json \
  --vault-alias dev-vault \
  --state active \
  --queue needs-approval \
  --member-alias alice
```

Machine-readable output:

```bash
./scripts/run_pq_multisig_ops.sh \
  --mode show-proposal \
  --aliases-file ./docs/pq-multisig-ops.aliases.example.json \
  --vault-alias dev-vault \
  --proposal-id 7 \
  --json
```

Dashboard/desktop handoff into the local signer:

```bash
./scripts/run_pq_multisig_ops.sh \
  --mode approve \
  --aliases-file ./docs/pq-multisig-ops.aliases.example.json \
  --vault-alias dev-vault \
  --member-alias alice \
  --proposal-id 7 \
  --manifest-only
```

For shell completions:

```bash
./scripts/install_pq_multisig_ops_completions.sh
```

For a thin local dashboard on top of the same CLI:

```bash
./scripts/run_pq_multisig_dashboard.sh
```

Then open `http://127.0.0.1:8787`. The dashboard uses the same `--json` and
`--manifest-only` surfaces as the CLI, so it stays read-only in the browser and
hands actions back to the local signer path instead of moving Falcon approvals
into frontend code.

## Mainnet Path

- Deploy `vector_falcon512.so` under your own program ID on devnet first, then mainnet. The artifact in this repo is the program the local validator already loads.
- Deploy `pq_multisig_vault` only after you are comfortable with the FalconOnly
  v1 constraint: every member must be a live Vector/Falcon PDA, so hybrid
  Ed25519 + Falcon vaults are intentionally out of scope for the first mainnet
  release.
- Use:

  ```bash
  ./scripts/deploy_pq_multisig_vault.sh \
    --program-keypair-file <path-to-program-keypair.json>
  ```

  to deploy `pq_multisig_vault`, then:

  ```bash
  ./scripts/run_pq_multisig_ops_mainnet_hardware_canary.sh \
    --port /dev/cu.usbmodemXXXX \
    --vault-program-id <deployed_vault_program_id>
  ```

  to prove the first hardware-backed FalconOnly `2-of-3` vault on mainnet.
- Use a local or relayer fee payer for the outer transaction. Today the firmware's SE050 `SIGN` path still rejects Vector-sized outer Solana messages, so mainnet should follow the already-working pattern of `outer Ed25519 fee payer + ESP32 Falcon digest signer`.
- Roll out in canary order: `full-empty`, then a tiny `full-withdraw`, then a tiny `full-spl-transfer`, then `full-stake-delegate`, then a tiny `full-raydium-cpmm-swap`, then a tiny `full-jupiter-router-swap`, then richer protocol bundles.
- Existing AMMs or staking protocols do not need Falcon-specific code, but they do need to be callable with the Vector PDA as the relevant authority or delegate during CPI. That is the compatibility filter to check before trying a mainnet protocol flow.
