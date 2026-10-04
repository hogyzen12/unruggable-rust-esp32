Run from the repository root:

```sh
cargo test --locked --manifest-path tools/firmware-host-tests/Cargo.toml
```

These tests import the real firmware modules rather than copies. They cover BLE
connection ownership, root-key creation under simulated faults, line framing,
access-bundle cryptography, and transaction-preview robustness. They do not
exercise ESP-IDF, radio pairing, I2C, SPI, display, or physical-button timing.
