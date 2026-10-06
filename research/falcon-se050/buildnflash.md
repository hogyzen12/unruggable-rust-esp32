# Historical SE050/Falcon development commands

For the stable ESP32-only firmware or the monorepo-derived dual-element release,
start with [firmware/README.md](firmware/README.md). The commands below build the
older development crate at this workspace's root.

---

cargo +esp build

This build is the ESP32-S3 + SE050 showpiece firmware:

- PIN-only setup
- on-device button confirmations for wallet creation, Falcon init, sign, hybrid sign, Vector digest sign, and wipe on boards that expose `USER_BTN`
- 2FA/OTP setup disabled
- fresh or wiped units are blank until first PIN setup
- `SET_MODE:PIN:<6digits>` creates the SE050 wallet and finalizes auth
- `WIPE_KEYS`, `FACTORY_WIPE`, or `RESET_DEVICE` return the device to blank

Board-profile notes:

- `unruggable-v1` is still the default profile in this repo.
- `dual-element-se050` is for newer ESP32-S3 boards that follow the newer
  dual-element architecture. It now maps directly to **Unruggable V2.0**:
  - SE050 `ENA=15`, `SDA=26`, `SCL=21`
  - Tropic `CSN=47`, `SCK=33`, `MISO=34`, `MOSI=48`
  - TFT `CS=42`, `SCK=41`, `SDA=40`, `DC=39`, `RST=38`, `BL=37`
- Do not blindly reuse the `tropic-multidevice-breadboard` GPIO numbers on the
  S3 build; that reference firmware targets a classic ESP32, not the S3.

The flashable debug image is:

`target/xtensa-esp32s3-espidf/debug/esp32-solana-signer`

Recommended scripts:

- Destructive blank-device smoke test:

`./scripts/flash_and_smoke_se050.sh --port /dev/cu.usbmodemXXXX --allow-wipe --erase`

- Safe flash + hybrid cryptographic verification harness:

`./scripts/flash_and_hybrid_harness.sh --port /dev/cu.usbmodemXXXX --sample-tx-b64 <base64_payload>`

- Flash only:

`./scripts/flash_se050_dev.sh --port /dev/cu.usbmodemXXXX`

- Build the low-level peripheral probe for dual-element bring-up:

`SIGNER_BOARD_PROFILE=dual-element-se050 cargo +esp build --bin peripheral_probe`

- Flash the probe when you want to confirm both secure elements are electrically visible:

`espflash flash --chip esp32s3 --flash-size 8mb target/xtensa-esp32s3-espidf/debug/peripheral_probe --port /dev/cu.usbmodemXXXX --monitor`

- Flash a newer dual-element-style board once you know the real SE050 pins:

`./scripts/flash_se050_dev.sh --port /dev/cu.usbmodemXXXX --board-profile dual-element-se050`

- If you need to override the V2.0 defaults for a later PCB spin:

`./scripts/flash_se050_dev.sh --port /dev/cu.usbmodemXXXX --board-profile dual-element-se050 --se050-sda-pin <gpio> --se050-scl-pin <gpio> [--se050-enable-pin <gpio>]`

- Reset an already-flashed unit back to blank:

`python3 scripts/reset_se050_blank.py --port /dev/cu.usbmodemXXXX`

Manual flash command:

`espflash flash --chip esp32s3 --flash-size 8mb target/xtensa-esp32s3-espidf/debug/esp32-solana-signer --port /dev/cu.usbmodemXXXX`

If you explicitly want a totally clean flash + NVS state first:

`espflash erase-flash --port /dev/cu.usbmodemXXXX`

Then flash again:

`espflash flash --chip esp32s3 --flash-size 8mb target/xtensa-esp32s3-espidf/debug/esp32-solana-signer --port /dev/cu.usbmodemXXXX`

Expected blank-state protocol after boot or after `WIPE_KEYS`:

- `GET_INFO` includes `AUTH_MODE=UNSET` and `KEY_STATE=UNINITIALIZED`
- `GET_PUBKEY` returns `ERROR:WALLET_NOT_INITIALIZED`
- `SE050_STATUS` returns `SE050_BLANK`

Expected first-time setup flow:

1. `SET_MODE:PIN:<PIN>`
2. `GET_PUBKEY`
3. `UNLOCK:PIN:<PIN>`
4. `SIGN:<base64_payload>`

On **Unruggable V2.0**, the screen now prompts for a physical button hold during:

- initial wallet creation
- `PQC_INIT`
- `SIGN`
- `SIGN_HYBRID`
- `PQC_SIGN_VECTOR_DIGEST`
- `WIPE_KEYS`

Useful local smoke test:

`python3 test_firmware.py`

Useful local hybrid harness:

`./scripts/run_hybrid_harness.sh --port /dev/cu.usbmodemXXXX --pin <PIN> --sample-tx-b64 <base64_payload>`

That harness now also checks `PQC_SIGN_VECTOR_DIGEST` by asking the ESP32 to
sign a 32-byte demo digest and verifying the returned signature with
`solana-falcon512`.

Useful local Falcon/Vector wire-compatibility probe:

`cargo +stable run --manifest-path tools/vector-falcon-interop/Cargo.toml`

Useful live-device Vector bridge harness:

`./scripts/run_vector_device_harness.sh --port /dev/cu.usbmodemXXXX --pin <PIN>`

If you want it to also print the `initialize` instruction shape, pass a payer:

`./scripts/run_vector_device_harness.sh --port /dev/cu.usbmodemXXXX --pin <PIN> --payer <base58_fee_payer>`

After you have actually submitted `initialize` and fetched the current Vector nonce
from the created on-chain account, you can ask the device to sign the real Vector
digest and print the matching `advance` instruction shape:

`./scripts/run_vector_device_harness.sh --port /dev/cu.usbmodemXXXX --pin <PIN> --nonce-hex <64_hex_nonce>`

Useful live-device/on-chain Vector RPC harness:

`./scripts/run_vector_onchain_harness.sh --port /dev/cu.usbmodemXXXX --rpc-url <url> --mode full-empty --fee-payer-file ~/.config/solana/id.json`

That harness talks to the device over serial, submits `initialize` if needed,
fetches the on-chain Vector nonce, asks the ESP32 to sign the real 32-byte
Vector digest, and submits an empty `advance`.

Useful live-device/on-chain Vector withdraw harness:

`./scripts/run_vector_onchain_harness.sh --port /dev/cu.usbmodemXXXX --rpc-url <url> --mode full-withdraw --withdraw-lamports 3000000 --fee-payer-file ~/.config/solana/id.json`

That path does a real value-moving transaction under Falcon authorization:
it initializes if needed, tops the Vector PDA up if needed so the account
stays rent-exempt, asks the ESP32 to sign the `advance` digest for
`advance + passthrough(withdraw)`, and submits the final transaction.

Useful live-device/on-chain Vector SPL transfer harness:

`./scripts/run_vector_onchain_harness.sh --port /dev/cu.usbmodemXXXX --rpc-url <url> --mode full-spl-transfer --fee-payer-file ~/.config/solana/id.json`

That path does a real Falcon-authorized SPL transfer under Vector control:
it initializes if needed, and either:

- with no `--spl-mint`, creates a temporary SPL mint, creates a
  Vector-owned source ATA plus destination ATA, and mints test tokens into
  the source account
- with `--spl-mint <mint>`, uses an existing Tokenkeg SPL mint, ensures the
  device-wallet ATA, Vector-owned source ATA, plus destination ATA exist,
  auto-bootstraps the Vector ATA from the hardware wallet’s Ed25519 ATA when
  possible, and otherwise prints the exact device wallet + ATA that need
  funding

Then it asks the ESP32 to sign the `advance` digest for
`advance + passthrough(transfer_checked)` and submits the final transaction.

Useful no-RPC device-payer probe:

`./scripts/run_vector_onchain_harness.sh --port /dev/cu.usbmodemXXXX --mode probe-device-payer`

That probe currently fails on live hardware for both dummy Vector `initialize`
and dummy Vector `advance` messages, which means the current SE050 `SIGN` path
still rejects Vector-style outer Solana messages. The immediate working route is
therefore `local fee payer + ESP32 Falcon Vector authorization`.

That blocker is now gone: the Falcon Vector program is deployed on mainnet at
`FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU`, and the first mainnet canaries
(`full-empty` and `full-withdraw`) have already passed against that address.

Useful local Falcon Vector validator flow:

`./scripts/build_vector_falcon_program.sh`

`./scripts/start_vector_falcon_local_validator.sh --build --mint-keypair ~/.config/solana/id.json`

Then in another shell:

`./scripts/run_vector_localnet_onchain_harness.sh --port /dev/cu.usbmodemXXXX --fee-payer-file ~/.config/solana/id.json --mode full-empty`

For a real Falcon-authorized lamport withdrawal on localnet:

`./scripts/run_vector_localnet_onchain_harness.sh --port /dev/cu.usbmodemXXXX --fee-payer-file ~/.config/solana/id.json --mode full-withdraw --withdraw-lamports 3000000`

For a real Falcon-authorized SPL token transfer on localnet:

`./scripts/run_vector_localnet_onchain_harness.sh --port /dev/cu.usbmodemXXXX --fee-payer-file ~/.config/solana/id.json --mode full-spl-transfer`

For a real Falcon-authorized native stake delegation on localnet:

`./scripts/run_vector_localnet_onchain_harness.sh --port /dev/cu.usbmodemXXXX --fee-payer-file ~/.config/solana/id.json --mode full-stake-delegate`

That path initializes if needed, creates a fresh stake account funded by the
outer fee payer, sets both stake authorities to the Vector PDA, asks the ESP32
to sign the `advance` digest for `advance + passthrough(delegate_stake)`, and
then verifies on-chain that the stake account is delegated to the chosen vote
account.

For a first Falcon-authorized Raydium CPMM swap canary:

`./scripts/run_vector_onchain_harness.sh --port /dev/cu.usbmodemXXXX --rpc-url <url> --fee-payer-file ~/.config/solana/id.json --mode full-raydium-cpmm-swap --swap-min-out <raw_out>`

That mode defaults to a `USDC -> USDT` CPMM canary, auto-discovers the
highest-liquidity Raydium CPMM pool for the two mints unless you pin
`--raydium-cpmm-pool-id`, bootstraps the Vector-owned input ATA from the
hardware wallet ATA if needed, and then submits `advance + passthrough(swap_base_input)`.
On mainnet the final swap transaction is compiled as a V0 message using the
Raydium pool's lookup table, because the legacy transaction format is too small
for the CPMM account set. If the output mint is WSOL, add `--swap-unwrap-sol`
to close the Vector-owned WSOL ATA inside the same Falcon-authorized passthrough
and deliver native SOL to the receiver.

For a first Falcon-authorized Jupiter router swap canary:

`./scripts/run_vector_onchain_harness.sh --port /dev/cu.usbmodemXXXX --rpc-url <url> --fee-payer-file ~/.config/solana/id.json --mode full-jupiter-router-swap --swap-amount-in 1000000`

That mode defaults to a `USDC -> USDT` routed canary, reads
`UNRUGGABLE_JUPITER_API_KEY` from the environment or a `partner_secrets.local`
file, bootstraps the Vector-owned input ATA from the hardware wallet ATA if
needed, then calls Jupiter `/swap/v2/build` and submits the final swap leg as a
V0 message using the lookup tables returned by Jupiter.

For the first localnet Falcon `2-of-3` multisig-vault canary:

`./scripts/build_pq_multisig_vault_program.sh`

`./scripts/start_pq_multisig_local_validator.sh --build --mint-keypair ~/.config/solana/id.json`

Then in another shell:

`./scripts/run_pq_multisig_localnet_harness.sh`

That flow provisions 3 synthetic Falcon/Vector member PDAs plus one
`pq_multisig_vault`, creates a SOL-withdraw proposal from member 0 via
`advance + passthrough(multisig::create_proposal_from_member)`, records the
second approval from member 1 via the same Falcon-gated member path, then
executes the vault-owned SOL withdraw once the `not_before_slot` is reached.

That harness now continues past the first SOL canary and also drives:

- a cancelled-and-closed proposal to prove async lifecycle cleanup
- a vault-owned `SPL transfer` proposal, approved through two member Vector
  PDAs and executed by the vault PDA using the vault's canonical ATA for that
  mint
- a self-governed `config update` proposal that raises the threshold and
  updates the vault policy slots through the same `2-of-3` Falcon path
- a two-step member rotation where `replace_member` installs a pending member
  and `accept_pending_member` finalizes the swap
- a final post-rotation `3-of-3` SOL withdraw proposal created by the new
  member, while the rotated-out member is rejected on approval

For the proposal-style async operator workflow on localnet:

`./scripts/run_pq_multisig_ops_localnet_e2e.sh`

That wrapper drives the same hardened vault lifecycle through the new
`tools/pq-multisig-ops` CLI instead of the monolithic harness. It proves:

- direct proposal discovery with `list-proposals`
- decoded inspection with `show-proposal`
- separate create / approve / revoke / cancel / close / execute stages
- vault-owned `SOL withdraw`
- cancelled proposal cleanup
- canonical-ATA vault-owned `SPL transfer`
- self-governed `config update`
- `replace_member -> accept_pending_member`
- rotated-out rejection and a final post-rotation `3-of-3` proposal

For the easiest dev onboarding path:

`./scripts/run_pq_multisig_ops_localnet_quickstart.sh`

That wrapper starts the PQ local validator, waits for RPC readiness, runs the
full CLI-based async operator E2E above, and tears the validator down
automatically.

For the first real hardware-backed member proof:

`./scripts/run_pq_multisig_ops_localnet_hardware_quickstart.sh --port /dev/cu.usbmodemXXXX`

That wrapper starts the same PQ local validator, prepares the connected Falcon
device, then proves both directions that matter for production:

- hardware member creates proposal `0`, fixture member approves, vault executes
- fixture member creates proposal `1`, hardware member approves, vault executes

For ad hoc operator actions instead of the full wrapper:

`./scripts/run_pq_multisig_ops.sh --mode <subcommand> ...`

The CLI can now discover proposals directly from chain before you sign anything.
Typical operator flow:

```bash
./scripts/run_pq_multisig_ops.sh \
  --mode list-proposals \
  --vault-seed pq-mainnet-demo \
  --state active \
  --queue needs-approval \
  --member-fixture-file /tmp/member-1.json
```

Then inspect a specific proposal:

```bash
./scripts/run_pq_multisig_ops.sh \
  --mode show-proposal \
  --vault-seed pq-mainnet-demo \
  --proposal-id 7
```

Then approve or execute it with the same wrapper.

For named vault/member aliases, copy:

`docs/pq-multisig-ops.aliases.example.json`

Then use alias-driven discovery:

```bash
./scripts/run_pq_multisig_ops.sh \
  --mode list-proposals \
  --aliases-file ./docs/pq-multisig-ops.aliases.example.json \
  --vault-alias dev-vault \
  --queue needs-approval \
  --member-alias alice
```

For integrations or a future dashboard handoff, the CLI can emit a structured
manifest instead of executing immediately:

```bash
./scripts/run_pq_multisig_ops.sh \
  --mode approve \
  --aliases-file ./docs/pq-multisig-ops.aliases.example.json \
  --vault-alias dev-vault \
  --member-alias alice \
  --proposal-id 7 \
  --manifest-only
```

For machine-readable reads:

```bash
./scripts/run_pq_multisig_ops.sh \
  --mode show-proposal \
  --vault-seed pq-mainnet-demo \
  --proposal-id 7 \
  --json
```

For shell completion setup:

```bash
./scripts/install_pq_multisig_ops_completions.sh
```

For a thin local proposal dashboard that stays read-only and only prepares
manifest handoffs back into the CLI signer path:

```bash
./scripts/run_pq_multisig_dashboard.sh
```

Then open:

`http://127.0.0.1:8787`

For a single-command mainnet canary of send + stake + swap:

`./scripts/run_mainnet_falcon_e2e.sh --withdraw-receiver <pubkey> --stake-vote-account <vote_account>`

That wrapper runs the three proven Falcon flows in sequence:
`full-withdraw`, `full-stake-delegate`, and a small `USDC -> SOL` Raydium swap
using `--swap-unwrap-sol`.

For the FalconOnly multisig deployment step itself:

`./scripts/deploy_pq_multisig_vault.sh --program-keypair-file <path-to-program-keypair.json>`

For the first hardware-backed FalconOnly mainnet vault canary:

`./scripts/run_pq_multisig_ops_mainnet_hardware_canary.sh --port /dev/cu.usbmodemXXXX --vault-program-id <deployed_vault_program_id>`

That canary reuses the same operator CLI flow already proven on localnet, but
targets a deployed mainnet `pq_multisig_vault`: one real hardware Falcon
member, two fixture Falcon members, one hardware-authored SOL withdraw, one
fixture-authored SOL withdraw approved by the hardware member, and close of
both executed proposals.

If you want to load the local validator under a different program ID, pass it
to the validator launcher as `--program-id <base58>` and pass the same value to
the harness as `--vector-program-id <base58>` (or set `VECTOR_PROGRAM_ID`).

Mainnet rollout after localnet:

1. Deploy `vector_falcon512.so` under your own program ID on devnet first, then mainnet.
2. Keep using a local or relayer fee payer for the outer Solana transaction.
3. Point the harness at the deployed program with `--rpc-url <cluster>` and `--vector-program-id <program_id>`.
4. Start with `full-empty`, then a tiny `full-withdraw`, then a tiny `full-spl-transfer`, then `full-stake-delegate`.
5. Then try a tiny `full-raydium-cpmm-swap`.
6. Then try a tiny `full-jupiter-router-swap`.
7. Only after those pass should you attempt richer protocol bundles like staking-app CPIs or protocol-specific routed flows.
