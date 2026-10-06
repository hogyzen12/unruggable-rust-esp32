#!/usr/bin/env python3
"""Reset an SE050 signer device back to blank and verify the result."""

import argparse
import sys
import time

import serial


READ_TIMEOUT = 4.0
ACTION_TIMEOUT = 35.0


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


def main():
    parser = argparse.ArgumentParser(
        description="Send WIPE_KEYS to the active SE050 firmware and verify blank state."
    )
    parser.add_argument("--port", required=True, help="Serial port, e.g. /dev/cu.usbmodem101")
    parser.add_argument("--baud", type=int, default=115200, help="Baud rate. Default: 115200")
    args = parser.parse_args()

    try:
        print(f"Connecting to {args.port} at {args.baud} baud...")
        ser = serial.Serial(args.port, args.baud, timeout=0.2)
        ser.dtr = False
        ser.rts = False
        time.sleep(2)
        ser.reset_input_buffer()

        boot = read_available(ser, 1.5)
        if boot:
            print(f"\nBOOT <<< {boot}")

        response = send_command(ser, "WIPE_KEYS", wait_time=ACTION_TIMEOUT)
        require_contains("WIPE_KEYS", response, "WIPED")

        response = send_command(ser, "GET_INFO", wait_time=READ_TIMEOUT)
        require_contains("GET_INFO", response, "AUTH_MODE=UNSET")
        require_contains("GET_INFO", response, "KEY_STATE=UNINITIALIZED")

        response = send_command(ser, "GET_PUBKEY", wait_time=READ_TIMEOUT)
        require_contains("GET_PUBKEY", response, "ERROR:WALLET_NOT_INITIALIZED")

        response = send_command(ser, "SE050_STATUS", wait_time=READ_TIMEOUT)
        require_contains("SE050_STATUS", response, "SE050_BLANK")

        ser.close()
        print("\nDevice is blank and ready for first-time PIN setup.")
    except serial.SerialException as err:
        print(f"ERROR: Could not open serial port {args.port}: {err}", file=sys.stderr)
        sys.exit(1)
    except RuntimeError as err:
        print(f"RESET FAILED: {err}", file=sys.stderr)
        sys.exit(1)
    except KeyboardInterrupt:
        print("\nInterrupted.", file=sys.stderr)
        sys.exit(130)


if __name__ == "__main__":
    main()
