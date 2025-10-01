# ESP32 Solana Hardware Signer

![ESP32 Signer](https://github.com/hogyzen12/unruggable-rust-esp32/blob/master/icon.png)


A hardware-based approach to secure Solana transaction signing using an ESP32 microcontroller. This project aims to provide an affordable yet secure alternative to expensive hardware wallets for signing Solana blockchain transactions.

## Overview

This project consists of two main components:

1. **ESP32 Firmware**: Rust-based firmware for the ESP32 that securely generates and stores a Solana keypair, allowing signing operations only when physically triggered by pressing the BOOT button.
2. **Host Applications**: Programs to interface with the ESP32 hardware signer, implemented in Rust, Go, and JavaScript.

The ESP32 acts as a secure element that never exposes the private key, requiring physical interaction (button press) to authorize transaction signing.

## How It Works

1. On first boot, the ESP32 generates a new Ed25519 keypair and stores it securely in non-volatile storage
2. When connected to a host computer, it responds to simple serial commands:
   - `GET_PUBKEY`: Returns the public key for creating transactions
   - Base64-encoded message: Triggers the signing process (requires button press)
3. For each signing request, the ESP32 waits for a physical button press on GPIO0 (BOOT button)
4. After pressing the button, the message is signed with the private key and returned as a Base64-encoded signature
5. The host computer then attaches this signature to the transaction and submits it to the Solana network

## Hardware Requirements

- ESP32 Development Board (any variant with the BOOT button on GPIO0)
- USB-C cable (or appropriate cable for your ESP32 board)
- Computer for running the host application

## Quick Start

### Setting Up the ESP32

1. Install the Rust toolchain for ESP32:
```bash
cargo install espup --locked
espup install
. $HOME/export-esp.sh
```

2. Flash the firmware to your ESP32:
```bash
cd esp32-solana-signer
cargo +esp build
cargo install espflash --locked
espflash flash target/xtensa-esp32-espidf/debug/esp32-solana-signer --port /dev/tty.usbserial-0001
```

3. Optionally, monitor the ESP32 output:
```bash
sudo espflash monitor --port /dev/tty.usbserial-0001
```

Note: Replace `/dev/tty.usbserial-0001` with the appropriate port for your system (e.g., `COM3` on Windows, `/dev/ttyUSB0` on Linux).

### Using the Signer

Once the firmware is flashed, the ESP32 will automatically generate a new Solana keypair on first boot, or load an existing one from NVS on subsequent boots.

#### Signing a Transaction

1. Connect the ESP32 to your computer via USB
2. Run one of the host applications to prepare an unsigned transaction:
```bash
cd solana-transaction-builder/rust/solana-tx-signer
cargo run
```

3. When the transaction is sent to the ESP32, press the BOOT button on the ESP32 to confirm and sign the transaction
4. The host application will automatically receive the signature and submit the transaction to the Solana network

Example terminal output:
```
Requested public key from ESP32
Received ESP32 public key: Hy5oibb1cYdmjyPJ2fiypDtKYvp1uZTuPkmFzVy7TL8c
Serialized Transaction Message (Base64): AQABA/wY8x7qf58iGUmJZzXONjNNWRtNNqfjrli/woXCfZRVV2dG9oKhrxsZn8/dvHHvGs4pjwdyafMunVJIIbimOMkAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAPi+WpxhpGeo6pYuD//BusKy41SfzTiQcaVatG99p/y7AQICAAEMAgAAAEBCDwAAAAAA
Sent to ESP32: AQABA/wY8x7qf58iGUmJZzXONjNNWRtNNqfjrli/woXCfZRVV2dG9oKhrxsZn8/dvHHvGs4pjwdyafMunVJIIbimOMkAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAPi+WpxhpGeo6pYuD//BusKy41SfzTiQcaVatG99p/y7AQICAAEMAgAAAEBCDwAAAAAA
Received from ESP32: J2j/MgmB8EMw4z5JUSd17Kc5wYRxqREUf+1YQUyEqPduJlet7Iihz5zw7n1fiawWZfYixKkKLkeMVkaHEjR4Cw==
Transaction submitted with ID: nhcaMcizWGhRy9BxZ1yQ15pmp6gAYJKzkDkodn5XMAuKwmzDjqg6i3GKSETgZbdga3FirpGF9Z9MNbNDV7MMqPp
```

## Protocol Description

The ESP32 hardware signer communicates via a simple serial protocol:

| Command | Description | Response |
|---------|-------------|----------|
| `GET_PUBKEY` | Request the public key | Base58-encoded public key string |
| `<base64_message>` | Request to sign a message (transaction) | Base64-encoded signature (after button press) |

## Implementation Details

### ESP32 Firmware (Rust)

The ESP32 firmware is built using the Rust programming language with the `esp-idf-svc` framework. Key features include:

- Ed25519 key generation and storage in NVS
- Simple UART-based communication protocol
- Button input handling for physical confirmation
- Non-blocking operation

### Host Applications

#### Rust Implementation

The Rust implementation uses the Solana SDK to create transactions and communicate with the ESP32 via serial port.

#### Go Implementation

The Go implementation uses the `gagliardetto/solana-go` library for Solana interaction and the `tarm/serial` package for ESP32 communication.

#### JavaScript Implementation

A simple JavaScript utility to create and serialize an unsigned Solana transaction for signing by the ESP32.

## Project Structure

```
.
├── esp32-solana-signer       # ESP32 firmware (Rust)
│   ├── Cargo.lock
│   ├── Cargo.toml
│   ├── build.rs
│   ├── rust-toolchain.toml   # Specifies the ESP32 Rust toolchain
│   └── src
│       └── main.rs           # Main firmware code
└── solana-transaction-builder # Host applications
    ├── go                     # Go implementation
    │   ├── go.mod
    │   ├── go.sum
    │   └── signer.go         # Go client for ESP32 communication
    ├── js                     # JavaScript implementation
    │   ├── make_unsigned_tx_b64.js
    │   ├── package-lock.json
    │   └── package.json
    └── rust                   # Rust implementation
        └── solana-tx-signer
            ├── Cargo.lock
            ├── Cargo.toml
            └── src
                └── main.rs    # Rust client for ESP32 communication
```

## Limitations & Future Work

- Currently supports only basic transfer transactions
- No display for transaction review (future enhancement)
- Limited to one keypair per device
- Future plans:
  - Add OLED display support for transaction details
  - Support for multiple accounts
  - Hardware security enhancements (secure boot, etc.)
  - Additional transaction types
  - Improve button press handling for a smoother user experience

## Troubleshooting

### Button Press Timing
The current implementation requires the BOOT button to be pressed while the ESP32 is waiting for input after receiving a transaction. The firmware waits for the button press before signing, so you don't need to time it precisely with sending the transaction.

### Serial Connection Issues
If you have trouble with serial connections:
- Ensure you're using the correct port name (check in Device Manager on Windows, `ls /dev/tty.*` on macOS, or `ls /dev/ttyUSB*` on Linux)
- Try unplugging and reconnecting the ESP32
- Make sure you have the correct USB drivers installed - ldport
- Check that no other program is using the serial port

### Build Issues
If you encounter build issues:
- Make sure you've sourced the ESP environment: `. $HOME/export-esp.sh`
- Verify that the ESP-IDF toolchain is properly installed
- Try running with the specific toolchain: `cargo +esp build`

## License

This project is licensed under the MIT License - see the LICENSE file for details.
