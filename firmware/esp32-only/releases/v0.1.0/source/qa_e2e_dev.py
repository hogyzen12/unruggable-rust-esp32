#!/usr/bin/env python3
import argparse
from pathlib import Path
import subprocess
import sys


def run_step(cmd: list[str], label: str) -> int:
    print()
    print(f"== {label} ==")
    print(" ".join(cmd))
    result = subprocess.run(cmd)
    return result.returncode


def main() -> int:
    ap = argparse.ArgumentParser(
        description="Flash one dev unit, run full PIN/wallet QA, and leave it in the verified QA state."
    )
    ap.add_argument("--port", required=True, help="Serial port, e.g. /dev/cu.usbserial-0001")
    ap.add_argument("--pin", required=True, help="Six-digit QA PIN to set during verification")
    ap.add_argument("--baud", type=int, default=115200, help="Baud rate (default: 115200)")
    ap.add_argument(
        "--read-timeout",
        type=float,
        default=4.0,
        help="Read timeout for non-button QA commands in seconds (default: 4.0)",
    )
    ap.add_argument(
        "--action-timeout",
        type=float,
        default=35.0,
        help="Read timeout for button-confirmed QA commands in seconds (default: 35.0)",
    )
    ap.add_argument(
        "--yes",
        action="store_true",
        help="Do not pause before button-confirmed QA actions",
    )
    ap.add_argument(
        "--skip-build",
        action="store_true",
        help="Pass --skip-build to the flash step and reuse the existing release artifact",
    )
    ap.add_argument(
        "--flash-batch-tag",
        default="qa-e2e-flash",
        help="Batch tag for the initial flash step",
    )
    ap.add_argument(
        "--flash-log-csv",
        default="",
        help="Optional CSV path for the initial flash step",
    )
    args = ap.parse_args()

    if len(args.pin) != 6 or not args.pin.isdigit():
        ap.error("--pin must be exactly 6 digits")

    root = Path(__file__).resolve().parent
    provision_script = root / "provision_dev.sh"
    qa_script = root / "qa_sample_dev.py"

    flash_cmd = [str(provision_script), "--port", args.port, "--skip-efuse-check"]
    if args.skip_build:
        flash_cmd.append("--skip-build")
    if args.flash_batch_tag:
        flash_cmd.extend(["--batch-tag", args.flash_batch_tag])
    if args.flash_log_csv:
        flash_cmd.extend(["--log-csv", args.flash_log_csv])

    qa_cmd = [
        sys.executable,
        "-u",
        str(qa_script),
        "--port",
        args.port,
        "--pin",
        args.pin,
        "--baud",
        str(args.baud),
        "--read-timeout",
        str(args.read_timeout),
        "--action-timeout",
        str(args.action_timeout),
        "--keep-after-pass",
    ]
    if args.yes:
        qa_cmd.append("--yes")

    print(
        "If espflash stalls at 'Connecting...', hold BOOT, tap EN once, "
        "keep holding BOOT until 'Using flash stub' appears, then release BOOT."
    )
    print(
        "During QA you will need: a 2-second hold for SET_MODE:PIN, "
        "one press for EXPORT_PRIVKEY, and one press for SIGN."
    )

    flash_rc = run_step(flash_cmd, "Step 1/2: Flash Blank Dev Firmware")
    if flash_rc != 0:
        print("Flash step failed. QA did not run.", file=sys.stderr)
        return flash_rc

    qa_rc = run_step(qa_cmd, "Step 2/2: Run PIN/Wallet QA")
    if qa_rc != 0:
        print("QA step failed. The board may still contain a QA wallet.", file=sys.stderr)
        print(
            "Run ./reset_dev_unit.sh --port "
            f"{args.port} to wipe and reflash it back to blank stock.",
            file=sys.stderr,
        )
        return qa_rc

    print()
    print("End-to-end QA PASSED.")
    print("The board now contains the verified QA PIN/wallet state.")
    print(f"Run ./reset_dev_unit.sh --port {args.port} to wipe and reflash it blank for shipping.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
