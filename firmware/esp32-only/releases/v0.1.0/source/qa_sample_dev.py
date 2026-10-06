#!/usr/bin/env python3
import argparse
import base64
from pathlib import Path
import re
import subprocess
import sys
import time

import serial

STARTUP_SETTLE_SECS = 1.0
INTERLINE_SETTLE_SECS = 0.15


BASE58_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
BASE58_INDEX = {ch: idx for idx, ch in enumerate(BASE58_ALPHABET)}


def b58decode(value: str) -> bytes:
    if not value:
        return b""

    acc = 0
    for ch in value:
        if ch not in BASE58_INDEX:
            raise ValueError(f"invalid base58 character: {ch!r}")
        acc = acc * 58 + BASE58_INDEX[ch]

    decoded = bytearray()
    while acc:
        acc, mod = divmod(acc, 256)
        decoded.append(mod)
    decoded.reverse()

    leading_zeroes = 0
    for ch in value:
        if ch == "1":
            leading_zeroes += 1
        else:
            break

    return b"\x00" * leading_zeroes + bytes(decoded)


def expect_regex(response: str, pattern: str, label: str) -> None:
    if re.fullmatch(pattern, response) is None:
        raise RuntimeError(f"{label} mismatch: expected /{pattern}/, got {response!r}")


def send_cmd(ser: serial.Serial, cmd: str, timeout: float) -> str:
    original_timeout = ser.timeout
    try:
        ser.timeout = timeout
        ser.reset_input_buffer()
        t0 = time.perf_counter()
        ser.write((cmd + "\n").encode("ascii", errors="strict"))
        deadline = t0 + timeout
        last_line = ""
        last_rx_at = None
        while time.perf_counter() < deadline:
            line = ser.readline()
            now = time.perf_counter()
            if line:
                last_line = line.decode("utf-8", errors="replace").strip()
                last_rx_at = now
                continue
            if last_line and last_rx_at is not None and now - last_rx_at >= INTERLINE_SETTLE_SECS:
                break
        dt_ms = (time.perf_counter() - t0) * 1000.0
    finally:
        ser.timeout = original_timeout

    response = last_line
    print(f"{cmd} -> {response or '<timeout>'} ({dt_ms:.1f} ms)")
    if not response:
        raise RuntimeError(f"{cmd} timed out")
    return response


def maybe_prompt(message: str, assume_yes: bool) -> None:
    if assume_yes:
        print(message)
        return
    input(f"{message}\nPress Enter to continue...")


def main() -> int:
    ap = argparse.ArgumentParser(
        description="Run the destructive sample QA flow on one dev unit."
    )
    ap.add_argument("--port", required=True, help="Serial port, e.g. /dev/cu.usbserial-0001")
    ap.add_argument("--pin", required=True, help="Six-digit test PIN to set during QA")
    ap.add_argument("--baud", type=int, default=115200, help="Baud rate (default: 115200)")
    ap.add_argument(
        "--read-timeout",
        type=float,
        default=4.0,
        help="Read timeout for non-button commands in seconds (default: 4.0)",
    )
    ap.add_argument(
        "--action-timeout",
        type=float,
        default=35.0,
        help="Read timeout for button-confirmed commands in seconds (default: 35.0)",
    )
    ap.add_argument(
        "--yes",
        action="store_true",
        help="Do not pause before button-confirmed actions",
    )
    ap.add_argument(
        "--keep-after-pass",
        action="store_true",
        help="Leave the QA wallet on the unit instead of reflashing it blank after a pass",
    )
    ap.add_argument(
        "--reflash-skip-build",
        action="store_true",
        help="Pass --skip-build to provision_dev.sh when reflashing blank after a pass",
    )
    ap.add_argument(
        "--reflash-skip-efuse-check",
        action="store_true",
        help="Pass --skip-efuse-check to provision_dev.sh when reflashing blank after a pass",
    )
    ap.add_argument(
        "--reflash-batch-tag",
        default="sample-qa-reflash",
        help="Batch tag to pass to provision_dev.sh for the post-QA blank reflash",
    )
    ap.add_argument(
        "--reflash-log-csv",
        default="",
        help="Optional CSV log path to pass to provision_dev.sh for the post-QA blank reflash",
    )
    args = ap.parse_args()

    if len(args.pin) != 6 or not args.pin.isdigit():
        ap.error("--pin must be exactly 6 digits")

    challenge_bytes = b"sample-qa-signature-check"
    challenge_b64 = base64.b64encode(challenge_bytes).decode("ascii")

    try:
        with serial.Serial(args.port, baudrate=args.baud, timeout=args.read_timeout) as ser:
            ser.dtr = False
            ser.rts = False
            # Opening the USB serial port can reset ESP32 dev boards. Wait for
            # the firmware to finish booting before the first QA command.
            time.sleep(STARTUP_SETTLE_SECS)

            info_blank = send_cmd(ser, "GET_INFO", args.read_timeout)
            expect_regex(
                info_blank,
                r"INFO;VER=.*;AUTH_MODE=UNSET;FINALIZED=0;LOCKED=0;RETRIES_LEFT=10;KEY_STATE=UNINITIALIZED",
                "blank GET_INFO",
            )

            pubkey_blank = send_cmd(ser, "GET_PUBKEY", args.read_timeout)
            expect_regex(
                pubkey_blank,
                r"ERROR:WALLET_NOT_INITIALIZED",
                "blank GET_PUBKEY",
            )

            maybe_prompt(
                f"About to send SET_MODE:PIN:{args.pin}. Hold the device button to confirm setup.",
                args.yes,
            )
            mode_set = send_cmd(ser, f"SET_MODE:PIN:{args.pin}", args.action_timeout)
            expect_regex(mode_set, r"MODE_SET:PIN", "PIN setup")

            info_pin = send_cmd(ser, "GET_INFO", args.read_timeout)
            expect_regex(
                info_pin,
                r"INFO;VER=.*;AUTH_MODE=PIN;FINALIZED=1;LOCKED=0;RETRIES_LEFT=10;KEY_STATE=READY",
                "PIN GET_INFO",
            )

            pubkey_resp = send_cmd(ser, "GET_PUBKEY", args.read_timeout)
            expect_regex(pubkey_resp, r"PUBKEY:[1-9A-HJ-NP-Za-km-z]+", "GET_PUBKEY")
            pubkey_b58 = pubkey_resp.split(":", 1)[1]
            pubkey_bytes = b58decode(pubkey_b58)
            if len(pubkey_bytes) != 32:
                raise RuntimeError(f"expected 32-byte pubkey, got {len(pubkey_bytes)} bytes")

            unlock_resp = send_cmd(ser, f"UNLOCK:PIN:{args.pin}", args.read_timeout)
            expect_regex(unlock_resp, r"UNLOCKED_UNTIL:\d+", "UNLOCK:PIN")

            maybe_prompt(
                "About to send EXPORT_PRIVKEY. Press the device button to authorize backup export.",
                args.yes,
            )
            export_resp = send_cmd(ser, "EXPORT_PRIVKEY", args.action_timeout)
            expect_regex(export_resp, r"PRIVKEY:[1-9A-HJ-NP-Za-km-z]+", "EXPORT_PRIVKEY")
            exported_b58 = export_resp.split(":", 1)[1]
            exported_keypair = b58decode(exported_b58)
            if len(exported_keypair) != 64:
                raise RuntimeError(
                    f"expected 64-byte exported keypair, got {len(exported_keypair)} bytes"
                )
            if exported_keypair[32:] != pubkey_bytes:
                raise RuntimeError("exported keypair pubkey does not match GET_PUBKEY")

            maybe_prompt(
                "About to send SIGN:<challenge>. Press the device button to authorize signing.",
                args.yes,
            )
            sign_resp = send_cmd(ser, f"SIGN:{challenge_b64}", args.action_timeout)
            expect_regex(sign_resp, r"SIGNATURE:[A-Za-z0-9+/=]+", "SIGN")
            signature_b64 = sign_resp.split(":", 1)[1]
            signature = base64.b64decode(signature_b64, validate=True)
            if len(signature) != 64:
                raise RuntimeError(f"expected 64-byte signature, got {len(signature)} bytes")
    except Exception as exc:  # noqa: BLE001
        print(f"Sample QA FAILED: {exc}", file=sys.stderr)
        return 1

    print("Sample QA PASSED.")

    if args.keep_after_pass:
        print("This unit now contains a test wallet.")
        print("Reflash it blank with ./provision_dev.sh before shipping.")
        return 0

    provision_script = Path(__file__).resolve().with_name("provision_dev.sh")
    cmd = [str(provision_script), "--port", args.port]
    if args.reflash_skip_build:
        cmd.append("--skip-build")
    if args.reflash_skip_efuse_check:
        cmd.append("--skip-efuse-check")
    if args.reflash_batch_tag:
        cmd.extend(["--batch-tag", args.reflash_batch_tag])
    if args.reflash_log_csv:
        cmd.extend(["--log-csv", args.reflash_log_csv])

    print("Reflashing QA-passed unit back to blank production-ready state...")
    result = subprocess.run(cmd)
    if result.returncode != 0:
        print(
            "Sample QA passed, but automatic blank reflash failed. "
            "Do not ship this unit until ./provision_dev.sh succeeds.",
            file=sys.stderr,
        )
        return result.returncode

    print("Sample QA PASSED and unit was reflashed blank for production.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
