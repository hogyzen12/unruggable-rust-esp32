Checkpoint: `2026-06-07-vector-device-harness-ok`

State captured:
- SE050 Ed25519 signing still works on-device for normal Solana-compatible `SIGN`.
- ESP32 Falcon-512 signing works on-device for:
  - `SIGN_HYBRID`
  - `PQC_SIGN_VECTOR_DIGEST`
- Host verification proves the Vector-compatible firmware path uses original
  Falcon transcript rules accepted by `solana-falcon512`.
- The live-device Vector bridge harness builds and runs, verifies the SE050
  binding, derives the Vector identity and PDA, and prints the `initialize`
  instruction shape for the current device keypair.

Live-device commands that passed:

```bash
./scripts/run_hybrid_harness.sh \
  --port /dev/cu.usbmodem1101 \
  --pin <PIN> \
  --sample-tx-b64 aGVsbG8= \
  --no-final-wipe
```

```bash
./scripts/run_vector_device_harness.sh \
  --port /dev/cu.usbmodem1101 \
  --pin <PIN>
```

```bash
./scripts/run_vector_device_harness.sh \
  --port /dev/cu.usbmodem1101 \
  --pin <PIN> \
  --payer 2g3Q3gM1KE8tsFx7UtS7MTz9gkHzy5UxTtXGtwyskzsb
```

Current live-device derived values:
- SE050 pubkey: `2g3Q3gM1KE8tsFx7UtS7MTz9gkHzy5UxTtXGtwyskzsb`
- Vector identity hex: `71d6960184f495a79c8e3807d65bcd8cb00ae55ca3269d040a45f179b2d29ece`
- Vector PDA: `FwyiquiwY2Jij1shNrVxW3mrJrVCQPFdFrMg26XAA4jL`
- Vector PDA bump: `254`

Next step:
1. Submit the Vector `initialize` instruction on-chain.
2. Fetch the created Vector account.
3. Read the current nonce from the account header.
4. Feed that nonce into `run_vector_device_harness.sh --nonce-hex ...`.
5. Use the returned digest signature in the real Vector `advance` instruction.

Copied here as a fallback snapshot:
- top-level build/config docs
- `src/main.rs`
- `src/falcon.rs`
- `tools/hybrid-harness/`
- `tools/vector-device-harness/`
- `tools/vector-falcon-interop/`
- `docs/vector-falcon-transaction-model.md`
- `scripts/run_vector_device_harness.sh`
