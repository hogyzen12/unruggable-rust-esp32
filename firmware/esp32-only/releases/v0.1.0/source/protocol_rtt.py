#!/usr/bin/env python3
import argparse
import re
import time

import serial

STARTUP_SETTLE_SECS = 1.0
INTERLINE_SETTLE_SECS = 0.15


def run(
    port: str,
    baud: int,
    timeout: float,
    commands: list[str],
    expect_regexes: list[str],
) -> int:
    failed = False
    with serial.Serial(port, baudrate=baud, timeout=timeout) as ser:
        ser.dtr = False
        ser.rts = False
        # Opening the USB serial port can reset ESP32 dev boards. Give the
        # firmware time to boot fully before sending the first command.
        time.sleep(STARTUP_SETTLE_SECS)
        for idx, cmd in enumerate(commands):
            payload = (cmd + "\n").encode("ascii", errors="strict")
            t0 = time.perf_counter()
            ser.reset_input_buffer()
            ser.write(payload)
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
            resp = last_line
            print(f"{cmd} -> {resp or '<timeout>'} ({dt_ms:.1f} ms)")

            expected = expect_regexes[idx] if expect_regexes else None
            if not resp:
                failed = True
                continue
            if expected is not None:
                if re.fullmatch(expected, resp) is None:
                    failed = True
            elif resp.startswith("ERROR:"):
                failed = True

    return 1 if failed else 0


def main() -> int:
    ap = argparse.ArgumentParser(description="Measure UART protocol round-trip latency.")
    ap.add_argument("--port", required=True, help="Serial port, e.g. /dev/cu.usbserial-0001")
    ap.add_argument("--baud", type=int, default=115200, help="Baud rate (default: 115200)")
    ap.add_argument("--timeout", type=float, default=4.0, help="Read timeout seconds (default: 4.0)")
    ap.add_argument(
        "--cmd",
        action="append",
        default=[],
        help="Command to send. Repeat flag to send multiple commands.",
    )
    ap.add_argument(
        "--expect-regex",
        action="append",
        default=[],
        help="Full-match regex expected for the corresponding --cmd. Repeat once per command.",
    )
    args = ap.parse_args()

    commands = args.cmd or ["GET_INFO", "GET_PUBKEY", "GET_TIME"]
    if args.expect_regex and len(args.expect_regex) != len(commands):
        ap.error("--expect-regex must be supplied once per command")

    return run(args.port, args.baud, args.timeout, commands, args.expect_regex)


if __name__ == "__main__":
    raise SystemExit(main())
