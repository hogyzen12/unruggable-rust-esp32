Checkpoint: `2026-06-07-vector-onchain-harness`

State captured:
- ESP32 Falcon-512 signing still works on-device, including the Vector-specific
  `PQC_SIGN_VECTOR_DIGEST` path.
- The host-side on-chain Vector harness exists in
  `tools/vector-onchain-harness/` and compiles cleanly.
- Live hardware `status` mode works and derives the current Vector identity and
  PDA from the connected device.
- A live no-RPC device-payer probe shows the current SE050 `SIGN` path rejects
  Vector-style outer Solana messages for both dummy `initialize` and dummy
  `advance`.

Commands validated on hardware:

```bash
./scripts/run_vector_onchain_harness.sh \
  --port /dev/cu.usbmodem1101 \
  --mode status \
  --pin <PIN>
```

```bash
./scripts/run_vector_onchain_harness.sh \
  --port /dev/cu.usbmodem1101 \
  --mode probe-device-payer \
  --pin <PIN>
```

Observed live-device values:
- SE050 pubkey: `2g3Q3gM1KE8tsFx7UtS7MTz9gkHzy5UxTtXGtwyskzsb`
- Vector identity hex: `71d6960184f495a79c8e3807d65bcd8cb00ae55ca3269d040a45f179b2d29ece`
- Vector PDA: `FwyiquiwY2Jij1shNrVxW3mrJrVCQPFdFrMg26XAA4jL`
- Vector PDA bump: `254`

Current practical path:
1. Use a funded local fee payer for the outer Solana transaction.
2. Use the ESP32 Falcon key for Vector `initialize` / `advance` authorization.
3. Come back and loosen or split the firmware `SIGN` path if we want true
   `SE050 outer tx signer + Falcon Vector authorization` on the same device.
