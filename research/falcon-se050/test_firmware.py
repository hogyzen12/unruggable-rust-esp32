#!/usr/bin/env python3
"""
Smoke test for the active ESP32-S3 + SE050 signer firmware.

Flow covered:
  WIPE_KEYS -> blank checks -> SET_MODE:PIN -> GET_PUBKEY -> UNLOCK:PIN
  -> PQC_INIT -> PQC_GET_PUBKEY -> PQC_GET_BINDING

Optional:
  Set SAMPLE_TX_B64 to exercise SIGN and SIGN_HYBRID with a real serialized
  Solana transaction.
"""

import os
import re
import sys
import time

import serial


PORT = os.environ.get("ESP_PORT", "/dev/cu.usbmodemXXXX")
BAUD = 115200
PIN = os.environ.get("TEST_PIN", "246810")
SAMPLE_TX_B64 = os.environ.get("SAMPLE_TX_B64")
READ_TIMEOUT = float(os.environ.get("READ_TIMEOUT_SECS", "4.0"))
ACTION_TIMEOUT = float(os.environ.get("ACTION_TIMEOUT_SECS", "35.0"))


def read_available(ser, duration=1.0):
    end = time.time() + duration
    chunks = []
    while time.time() < end:
        waiting = ser.in_waiting
        if waiting:
            chunks.append(ser.read(waiting).decode("utf-8", errors="ignore"))
            time.sleep(0.05)
        else:
            time.sleep(0.05)
    return "".join(chunks).strip()


def send_command(ser, cmd, wait_time=1.0):
    print(f"\n>>> {cmd}")
    ser.write(f"{cmd}\n".encode())
    ser.flush()
    response = read_available(ser, wait_time)
    if response:
        print(f"<<< {response}")
    return response


def require_contains(label, response, needle):
    if needle not in response:
        raise RuntimeError(f"{label} did not contain {needle!r}: {response!r}")


def require_regex(label, response, pattern):
    if re.search(pattern, response) is None:
        raise RuntimeError(f"{label} did not match {pattern!r}: {response!r}")


def main():
    print("=" * 60)
    print("ESP32-S3 SE050 Signer Smoke Test")
    print("=" * 60)

    if len(PIN) != 6 or not PIN.isdigit():
        raise RuntimeError("TEST_PIN must be exactly 6 digits")

    try:
        print(f"\nConnecting to {PORT} at {BAUD} baud...")
        ser = serial.Serial(PORT, BAUD, timeout=0.2)
        ser.dtr = False
        ser.rts = False
        time.sleep(2)
        ser.reset_input_buffer()
        print("Connected.")

        boot = read_available(ser, 1.5)
        if boot:
            print(f"\nBOOT <<< {boot}")

        print("\n" + "=" * 60)
        print("TEST 1: WIPE TO BLANK")
        print("=" * 60)
        response = send_command(ser, "WIPE_KEYS", wait_time=1.5)
        require_contains("WIPE_KEYS", response, "WIPED")

        response = send_command(ser, "GET_INFO", wait_time=READ_TIMEOUT)
        require_contains("GET_INFO", response, "AUTH_MODE=UNSET")
        require_contains("GET_INFO", response, "FINALIZED=0")
        require_contains("GET_INFO", response, "LOCKED=0")
        require_contains("GET_INFO", response, "RETRIES_LEFT=10")
        require_contains("GET_INFO", response, "KEY_STATE=UNINITIALIZED")
        require_contains("GET_INFO", response, "PQC_STATE=UNINITIALIZED")

        response = send_command(ser, "GET_PUBKEY", wait_time=READ_TIMEOUT)
        require_contains("GET_PUBKEY", response, "ERROR:WALLET_NOT_INITIALIZED")

        response = send_command(ser, "SE050_STATUS", wait_time=READ_TIMEOUT)
        require_contains("SE050_STATUS", response, "SE050_BLANK")

        response = send_command(ser, "PQC_STATUS", wait_time=READ_TIMEOUT)
        require_contains("PQC_STATUS", response, "PQC_BLANK")

        print("\n" + "=" * 60)
        print("TEST 2: OTP DISABLED")
        print("=" * 60)
        response = send_command(ser, "SET_MODE:OTP_BEGIN", wait_time=READ_TIMEOUT)
        require_contains("SET_MODE:OTP_BEGIN", response, "ERROR:PIN_REQUIRED")

        print("\n" + "=" * 60)
        print("TEST 3: PIN SETUP CREATES WALLET")
        print("=" * 60)
        response = send_command(ser, f"SET_MODE:PIN:{PIN}", wait_time=ACTION_TIMEOUT)
        require_contains("SET_MODE:PIN", response, "MODE_SET:PIN")

        response = send_command(ser, "GET_INFO", wait_time=READ_TIMEOUT)
        require_contains("GET_INFO", response, "AUTH_MODE=PIN")
        require_contains("GET_INFO", response, "FINALIZED=1")
        require_contains("GET_INFO", response, "LOCKED=0")
        require_contains("GET_INFO", response, "RETRIES_LEFT=10")
        require_contains("GET_INFO", response, "KEY_STATE=READY")
        require_contains("GET_INFO", response, "PQC_STATE=UNINITIALIZED")

        response = send_command(ser, "GET_PUBKEY", wait_time=READ_TIMEOUT)
        require_regex("GET_PUBKEY", response, r"PUBKEY:[1-9A-HJ-NP-Za-km-z]+")

        response = send_command(ser, "SE050_STATUS", wait_time=READ_TIMEOUT)
        require_contains("SE050_STATUS", response, "SE050_OK")

        print("\n" + "=" * 60)
        print("TEST 4: PIN UNLOCK")
        print("=" * 60)
        response = send_command(ser, f"UNLOCK:PIN:{PIN}", wait_time=READ_TIMEOUT)
        require_regex("UNLOCK:PIN", response, r"UNLOCKED_UNTIL:\d+")

        print("\n" + "=" * 60)
        print("TEST 5: INIT PQC KEY")
        print("=" * 60)
        response = send_command(ser, "PQC_STATUS", wait_time=READ_TIMEOUT)
        require_contains("PQC_STATUS", response, "PQC_BLANK")

        response = send_command(ser, "PQC_INIT", wait_time=ACTION_TIMEOUT)
        require_contains("PQC_INIT", response, "PQC_READY")
        require_regex("PQC_INIT", response, r"PQC_PUBKEY:[A-Za-z0-9+/=]+")
        require_regex("PQC_INIT", response, r"PQC_BINDING_SIG:[A-Za-z0-9+/=]+")

        response = send_command(ser, "GET_INFO", wait_time=READ_TIMEOUT)
        require_contains("GET_INFO", response, "PQC_STATE=READY")

        response = send_command(ser, "PQC_GET_PUBKEY", wait_time=READ_TIMEOUT)
        require_regex("PQC_GET_PUBKEY", response, r"PQC_PUBKEY:[A-Za-z0-9+/=]+")

        response = send_command(ser, "PQC_GET_BINDING", wait_time=READ_TIMEOUT)
        require_regex("PQC_GET_BINDING", response, r"PQC_BINDING_SIG:[A-Za-z0-9+/=]+")

        if SAMPLE_TX_B64:
            print("\n" + "=" * 60)
            print("TEST 6: SIGN")
            print("=" * 60)
            print("Using SAMPLE_TX_B64 from the environment.")
            response = send_command(ser, f"SIGN:{SAMPLE_TX_B64}", wait_time=ACTION_TIMEOUT)
            require_contains("SIGN", response, "SIGNATURE:")

            print("\n" + "=" * 60)
            print("TEST 7: SIGN_HYBRID")
            print("=" * 60)
            response = send_command(ser, f"SIGN_HYBRID:{SAMPLE_TX_B64}", wait_time=ACTION_TIMEOUT)
            require_contains("SIGN_HYBRID", response, "SIGNATURE:")
            require_contains("SIGN_HYBRID", response, "PQC_SIGNATURE:")
            require_contains("SIGN_HYBRID", response, "PQC_PUBKEY:")
            require_contains("SIGN_HYBRID", response, "PQC_BINDING_SIG:")
        else:
            print("\nSIGN test skipped.")
            print(
                "Set SAMPLE_TX_B64 to a base64-encoded serialized Solana transaction "
                "to test SIGN and SIGN_HYBRID."
            )

        print("\n" + "=" * 60)
        print("TEST 8: WIPE RETURNS DEVICE TO BLANK")
        print("=" * 60)
        response = send_command(ser, "WIPE_KEYS", wait_time=ACTION_TIMEOUT)
        require_contains("WIPE_KEYS", response, "WIPED")

        response = send_command(ser, "GET_INFO", wait_time=READ_TIMEOUT)
        require_contains("GET_INFO", response, "AUTH_MODE=UNSET")
        require_contains("GET_INFO", response, "FINALIZED=0")
        require_contains("GET_INFO", response, "LOCKED=0")
        require_contains("GET_INFO", response, "RETRIES_LEFT=10")
        require_contains("GET_INFO", response, "KEY_STATE=UNINITIALIZED")
        require_contains("GET_INFO", response, "PQC_STATE=UNINITIALIZED")

        response = send_command(ser, "GET_PUBKEY", wait_time=READ_TIMEOUT)
        require_contains("GET_PUBKEY", response, "ERROR:WALLET_NOT_INITIALIZED")

        response = send_command(ser, "PQC_STATUS", wait_time=READ_TIMEOUT)
        require_contains("PQC_STATUS", response, "PQC_BLANK")

        print("\nAll smoke tests passed.")
        ser.close()

    except serial.SerialException as err:
        print(f"\nERROR: Could not open serial port {PORT}")
        print(f"Details: {err}")
        print("\nTroubleshooting:")
        print("1. Set ESP_PORT to the correct device, e.g. ESP_PORT=/dev/cu.usbmodem101")
        print("2. Check that the board is connected and enumerated over USB")
        print("3. On macOS, run: ls /dev/cu.usbmodem* /dev/cu.usbserial*")
        sys.exit(1)
    except RuntimeError as err:
        print(f"\nSMOKE TEST FAILED: {err}")
        sys.exit(1)
    except KeyboardInterrupt:
        print("\nInterrupted.")
        sys.exit(130)


if __name__ == "__main__":
    main()
