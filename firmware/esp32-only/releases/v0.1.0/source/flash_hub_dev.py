#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import json
import os
import re
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import TextIO

import serial
from serial.tools import list_ports

from provision_dev_batch import DEFAULT_BUILD_COMMAND, build_release


APP_ROOT = Path(__file__).resolve().parent
DEFAULT_CONFIG_PATH = APP_ROOT / "flash_profiles.json"
STARTUP_SETTLE_SECS = 2.0
INTERLINE_SETTLE_SECS = 0.15
DEFAULT_CUE_SECS = 1.0
ERASE_ONLY_TIMEOUT_SECS = 90.0


def utc_timestamp(compact: bool = False) -> str:
    fmt = "%Y%m%dT%H%M%SZ" if compact else "%Y-%m-%dT%H:%M:%SZ"
    return datetime.now(timezone.utc).strftime(fmt)


def default_batch_tag(prefix: str = "dev") -> str:
    clean_prefix = prefix.strip() or "dev"
    return f"{clean_prefix}-{datetime.now().strftime('%Y-%m-%d')}-r1"


def sanitize_label(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]+", "-", value).strip("-") or "run"


def resolve_path(base: Path, raw_value: str) -> Path:
    path = Path(raw_value)
    return path.resolve() if path.is_absolute() else (base / path).resolve()


def extract_location(port: object) -> str:
    location = str(getattr(port, "location", "") or "").strip()
    if location:
        return location
    hwid = str(getattr(port, "hwid", "") or "")
    match = re.search(r"LOCATION=([^\s]+)", hwid)
    return match.group(1).strip() if match else ""


def location_matches_prefix(location: str, hub_location_prefix: str) -> bool:
    location = location.strip()
    hub_location_prefix = hub_location_prefix.strip().rstrip(".-")
    if not location or not hub_location_prefix:
        return False
    if location == hub_location_prefix:
        return True
    return location.startswith(hub_location_prefix) and location[len(hub_location_prefix):len(hub_location_prefix) + 1] in {
        ".",
        "-",
    }


def derive_hub_slot(location: str, hub_location_prefix: str) -> str:
    location = location.strip()
    hub_location_prefix = hub_location_prefix.strip().rstrip(".-")
    if not location or not hub_location_prefix:
        return ""
    if location == hub_location_prefix:
        return "root"
    if not location_matches_prefix(location, hub_location_prefix):
        return ""
    remainder = location[len(hub_location_prefix):].lstrip(".-")
    return remainder or "root"


def natural_sort_key(value: str) -> list[object]:
    return [int(part) if part.isdigit() else part.lower() for part in re.split(r"(\d+)", value or "")]


def expect_regex(response: str, pattern: str, label: str) -> None:
    if re.fullmatch(pattern, response) is None:
        raise RuntimeError(f"{label} mismatch: expected /{pattern}/, got {response!r}")


def show_cue(title: str, message: str, seconds: float) -> None:
    if seconds <= 0:
        return
    try:
        import tkinter as tk

        root = tk.Tk()
        root.title(title)
        root.attributes("-topmost", True)
        root.resizable(False, False)
        root.configure(padx=18, pady=14)
        title_label = tk.Label(root, text=title, font=("Helvetica", 15, "bold"))
        title_label.pack(anchor="w")
        body_label = tk.Label(root, text=message, justify="left", wraplength=420)
        body_label.pack(anchor="w", pady=(8, 0))
        root.update_idletasks()
        width = root.winfo_width()
        height = root.winfo_height()
        screen_w = root.winfo_screenwidth()
        x = max(20, screen_w - width - 40)
        y = 40
        root.geometry(f"+{x}+{y}")
        root.after(int(seconds * 1000), root.destroy)
        root.mainloop()
    except Exception:
        print(f"[cue] {title}: {message}")
        time.sleep(seconds)


def send_cmd(
    ser: serial.Serial,
    cmd: str,
    timeout: float,
    log_handle: TextIO,
    display_cmd: str | None = None,
) -> str:
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
    shown_cmd = display_cmd or cmd
    line = f"{shown_cmd} -> {response or '<timeout>'} ({dt_ms:.1f} ms)"
    print(line)
    log_handle.write(line + "\n")
    log_handle.flush()
    if not response:
        raise RuntimeError(f"{shown_cmd} timed out")
    return response


@dataclass
class HubPort:
    slot: str
    device: str
    location: str
    description: str


@dataclass
class SlotResult:
    slot: str
    port: str
    location: str
    status: str
    flash_status: str
    flash_returncode: int
    flash_duration_secs: float
    setup_status: str
    setup_duration_secs: float
    pubkey: str
    log_path: str
    error: str = ""


def load_profile(config_path: Path, profile_id: str | None) -> dict[str, object]:
    raw = json.loads(config_path.read_text(encoding="utf-8"))
    profiles = raw.get("profiles") or []
    if not isinstance(profiles, list) or not profiles:
        raise SystemExit(f"No profiles found in {config_path}")
    default_profile_id = str(raw.get("default_profile_id") or "")
    selected_id = profile_id or default_profile_id or str(profiles[0].get("profile_id") or profiles[0].get("id") or "")
    for item in profiles:
        if not isinstance(item, dict):
            continue
        current_id = str(item.get("profile_id") or item.get("id") or "")
        if current_id == selected_id:
            return item
    raise SystemExit(f"Profile '{selected_id}' not found in {config_path}")


def discover_hub_ports(profile: dict[str, object]) -> list[HubPort]:
    prefixes = [str(item).strip() for item in (profile.get("port_prefixes") or []) if str(item).strip()]
    hub_prefix = str(profile.get("hub_location_prefix") or "").strip().rstrip(".-")
    ports: list[HubPort] = []
    for port in list_ports.comports():
        device = str(getattr(port, "device", "") or "")
        if prefixes and not any(device.startswith(prefix) for prefix in prefixes):
            continue
        location = extract_location(port)
        if hub_prefix and not location_matches_prefix(location, hub_prefix):
            continue
        description = str(getattr(port, "description", "") or getattr(port, "product", "") or "").strip()
        ports.append(
            HubPort(
                slot=derive_hub_slot(location, hub_prefix) or "?",
                device=device,
                location=location,
                description=description or "-",
            )
        )
    ports.sort(key=lambda item: (natural_sort_key(item.slot), natural_sort_key(item.location), natural_sort_key(item.device)))
    return ports


def parse_requested_slots(raw_value: str) -> list[str]:
    return [item for item in re.split(r"[\s,]+", raw_value.strip()) if item]


def select_ports(ports: list[HubPort], slot_filter: list[str], select_all: bool, reverse: bool) -> list[HubPort]:
    if select_all:
        selected_ports = list(ports)
    elif slot_filter:
        port_by_slot = {port.slot: port for port in ports}
        missing_slots = [slot for slot in slot_filter if slot not in port_by_slot]
        if missing_slots:
            raise SystemExit(f"Unknown or unavailable slot(s): {', '.join(missing_slots)}")
        selected_ports = [port_by_slot[slot] for slot in slot_filter]
    else:
        selected_ports = []

    if reverse:
        selected_ports.reverse()

    return selected_ports


def print_ports(profile: dict[str, object], ports: list[HubPort]) -> None:
    profile_name = str(profile.get("name") or profile.get("profile_id") or "profile")
    hub_prefix = str(profile.get("hub_location_prefix") or "").strip() or "(none)"
    print(f"Profile: {profile_name}")
    print(f"Hub prefix: {hub_prefix}")
    if not ports:
        print("No matching hub ports detected.")
        return
    print("Detected hub ports:")
    for port in ports:
        print(f"  slot {port.slot:>4}  {port.device:<22}  {port.location:<10}  {port.description}")


def validate_artifact(project_root: Path, target_dir: Path, profile: dict[str, object], skip_build: bool) -> None:
    build_command = str(profile.get("build_command") or DEFAULT_BUILD_COMMAND).strip() or DEFAULT_BUILD_COMMAND
    if not skip_build:
        build_release(project_root, target_dir, build_command)

    artifact_check = str(profile.get("artifact_check") or "").strip()
    if not artifact_check:
        return
    artifact_path = resolve_path(target_dir, artifact_check)
    if not artifact_path.exists():
        raise SystemExit(f"Missing build artifact: {artifact_path}")


def write_summary(run_dir: Path, results: list[SlotResult]) -> None:
    summary_csv = run_dir / "summary.csv"
    summary_json = run_dir / "summary.json"
    fieldnames = [
        "slot",
        "port",
        "location",
        "status",
        "flash_status",
        "flash_returncode",
        "flash_duration_secs",
        "setup_status",
        "setup_duration_secs",
        "pubkey",
        "log_path",
        "error",
    ]
    with summary_csv.open("w", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(fh, fieldnames=fieldnames)
        writer.writeheader()
        for result in results:
            writer.writerow(
                {
                    "slot": result.slot,
                    "port": result.port,
                    "location": result.location,
                    "status": result.status,
                    "flash_status": result.flash_status,
                    "flash_returncode": result.flash_returncode,
                    "flash_duration_secs": f"{result.flash_duration_secs:.1f}",
                    "setup_status": result.setup_status,
                    "setup_duration_secs": f"{result.setup_duration_secs:.1f}",
                    "pubkey": result.pubkey,
                    "log_path": result.log_path,
                    "error": result.error,
                }
            )
    with summary_json.open("w", encoding="utf-8") as fh:
        json.dump(
            [
                {
                    "slot": result.slot,
                    "port": result.port,
                    "location": result.location,
                    "status": result.status,
                    "flash_status": result.flash_status,
                    "flash_returncode": result.flash_returncode,
                    "flash_duration_secs": result.flash_duration_secs,
                    "setup_status": result.setup_status,
                    "setup_duration_secs": result.setup_duration_secs,
                    "pubkey": result.pubkey,
                    "log_path": result.log_path,
                    "error": result.error,
                }
                for result in results
            ],
            fh,
            indent=2,
        )


def append_section(log_path: Path, text: str) -> None:
    with log_path.open("a", encoding="utf-8") as log_handle:
        log_handle.write(text)


def erase_slot_once(
    port: HubPort,
    log_path: Path,
    attempt: int,
) -> tuple[str, int, float]:
    cmd = ["espflash", "erase-flash", "--port", port.device]

    print()
    print(f"=== slot {port.slot} | {port.device} | {port.location} | erase attempt {attempt} ===")
    print(" ".join(cmd))

    started_at = time.perf_counter()
    with log_path.open("a" if attempt > 1 else "w", encoding="utf-8") as log_handle:
        log_handle.write(f"# erase_attempt={attempt} started_utc={utc_timestamp()}\n")
        log_handle.write(f"# slot={port.slot}\n")
        log_handle.write(f"# location={port.location}\n")
        log_handle.write(f"# command={' '.join(cmd)}\n\n")
        log_handle.flush()

        try:
            completed = subprocess.run(
                cmd,
                cwd=APP_ROOT,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                timeout=ERASE_ONLY_TIMEOUT_SECS,
                check=False,
            )
            output = completed.stdout or ""
            returncode = completed.returncode
        except subprocess.TimeoutExpired as exc:
            output = (exc.stdout or "") if isinstance(exc.stdout, str) else ""
            returncode = 124
            output += f"\nError: erase timed out after {ERASE_ONLY_TIMEOUT_SECS:.0f}s\n"

        if output:
            sys.stdout.write(output)
            if output and not output.endswith("\n"):
                sys.stdout.write("\n")
            log_handle.write(output)

    duration_secs = time.perf_counter() - started_at
    status = "PASS" if returncode == 0 else "FAIL"
    print(f"=== slot {port.slot} erase {status} ({duration_secs:.1f}s) ===")
    return status, returncode, duration_secs


def flash_slot_once(
    project_root: Path,
    target_dir: Path,
    profile: dict[str, object],
    port: HubPort,
    batch_tag: str,
    skip_efuse_check: bool,
    log_path: Path,
    attempt: int,
) -> tuple[str, int, float]:
    provision_script = resolve_path(project_root, str(profile.get("provision_script") or "provision_dev.sh"))
    log_csv = resolve_path(project_root, str(profile.get("log_csv") or "logs/dev_flash_log.csv"))
    cmd = [
        str(provision_script),
        "--skip-build",
        "--port",
        port.device,
        "--batch-tag",
        batch_tag,
        "--log-csv",
        str(log_csv),
    ]
    if skip_efuse_check:
        cmd.append("--skip-efuse-check")

    env = os.environ.copy()
    env["PROVISION_TARGET_DIR"] = str(target_dir)

    print()
    print(f"=== slot {port.slot} | {port.device} | {port.location} | flash attempt {attempt} ===")
    print(" ".join(cmd))

    started_at = time.perf_counter()
    with log_path.open("a" if attempt > 1 else "w", encoding="utf-8") as log_handle:
        log_handle.write(f"# flash_attempt={attempt} started_utc={utc_timestamp()}\n")
        log_handle.write(f"# slot={port.slot}\n")
        log_handle.write(f"# location={port.location}\n")
        log_handle.write(f"# command={' '.join(cmd)}\n\n")
        log_handle.flush()

        process = subprocess.Popen(
            cmd,
            cwd=project_root,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
        assert process.stdout is not None
        for line in process.stdout:
            sys.stdout.write(line)
            log_handle.write(line)
        returncode = process.wait()

    duration_secs = time.perf_counter() - started_at
    status = "PASS" if returncode == 0 else "FAIL"
    print(f"=== slot {port.slot} flash {status} ({duration_secs:.1f}s) ===")
    return status, returncode, duration_secs


def setup_pin_and_verify(
    port: HubPort,
    setup_pin: str,
    baud: int,
    read_timeout: float,
    action_timeout: float,
    cue_secs: float,
    log_path: Path,
) -> tuple[str, float, str, str]:
    started_at = time.perf_counter()
    pubkey = ""
    error = ""
    try:
        with log_path.open("a", encoding="utf-8") as log_handle:
            log_handle.write(f"\n# setup_started_utc={utc_timestamp()}\n")
            log_handle.flush()
            with serial.Serial(port.device, baudrate=baud, timeout=read_timeout) as ser:
                ser.dtr = False
                ser.rts = False
                time.sleep(STARTUP_SETTLE_SECS)

                info_blank = send_cmd(ser, "GET_INFO", read_timeout, log_handle)
                expect_regex(
                    info_blank,
                    r"INFO;VER=.*;AUTH_MODE=UNSET;FINALIZED=0;LOCKED=0;RETRIES_LEFT=10;KEY_STATE=UNINITIALIZED",
                    "blank GET_INFO",
                )

                pubkey_blank = send_cmd(ser, "GET_PUBKEY", read_timeout, log_handle)
                expect_regex(
                    pubkey_blank,
                    r"ERROR:WALLET_NOT_INITIALIZED",
                    "blank GET_PUBKEY",
                )

                show_cue(
                    f"Slot {port.slot}: PIN setup",
                    (
                        "Hold the board button now.\n"
                        "The script will send SET_MODE:PIN in a moment."
                    ),
                    cue_secs,
                )

                mode_set = send_cmd(
                    ser,
                    f"SET_MODE:PIN:{setup_pin}",
                    action_timeout,
                    log_handle,
                    display_cmd="SET_MODE:PIN:<redacted>",
                )
                expect_regex(mode_set, r"MODE_SET:PIN", "PIN setup")

                info_pin = send_cmd(ser, "GET_INFO", read_timeout, log_handle)
                expect_regex(
                    info_pin,
                    r"INFO;VER=.*;AUTH_MODE=PIN;FINALIZED=1;LOCKED=0;RETRIES_LEFT=10;KEY_STATE=READY",
                    "PIN GET_INFO",
                )

                pubkey_resp = send_cmd(ser, "GET_PUBKEY", read_timeout, log_handle)
                expect_regex(pubkey_resp, r"PUBKEY:[1-9A-HJ-NP-Za-km-z]+", "GET_PUBKEY")
                pubkey = pubkey_resp.split(":", 1)[1]

                unlock_resp = send_cmd(
                    ser,
                    f"UNLOCK:PIN:{setup_pin}",
                    read_timeout,
                    log_handle,
                    display_cmd="UNLOCK:PIN:<redacted>",
                )
                expect_regex(unlock_resp, r"UNLOCKED_UNTIL:\d+", "UNLOCK:PIN")
    except Exception as exc:  # noqa: BLE001
        error = str(exc)
        status = "FAIL"
    else:
        status = "PASS"

    duration_secs = time.perf_counter() - started_at
    print(f"=== slot {port.slot} setup {status} ({duration_secs:.1f}s) ===")
    return status, duration_secs, pubkey, error


def process_slot(
    project_root: Path,
    target_dir: Path,
    profile: dict[str, object],
    port: HubPort,
    batch_tag: str,
    skip_efuse_check: bool,
    run_dir: Path,
    flash_retries: int,
    setup_pin: str,
    baud: int,
    read_timeout: float,
    action_timeout: float,
    cue_secs: float,
    erase_only: bool,
) -> SlotResult:
    log_path = run_dir / f"slot-{sanitize_label(port.slot)}-{sanitize_label(port.device)}.log"
    flash_status = "FAIL"
    flash_returncode = 1
    flash_duration_secs = 0.0
    setup_status = "SKIPPED"
    setup_duration_secs = 0.0
    pubkey = ""
    error = ""

    for attempt in range(1, max(1, flash_retries) + 1):
        show_cue(
            f"Slot {port.slot}: {'erase' if erase_only else 'flash'} attempt {attempt}",
            (
                f"Port: {port.device}\n"
                f"Location: {port.location}\n\n"
                "Press and hold the board flash/BOOT button now.\n"
                "If needed:\n"
                "1. Hold BOOT or the board's flash button.\n"
                "2. Tap EN/RESET if needed.\n"
                "3. Keep holding until flashing starts."
            ),
            cue_secs,
        )

        if erase_only:
            flash_status, flash_returncode, flash_duration_secs = erase_slot_once(
                port=port,
                log_path=log_path,
                attempt=attempt,
            )
        else:
            flash_status, flash_returncode, flash_duration_secs = flash_slot_once(
                project_root=project_root,
                target_dir=target_dir,
                profile=profile,
                port=port,
                batch_tag=batch_tag,
                skip_efuse_check=skip_efuse_check,
                log_path=log_path,
                attempt=attempt,
            )
        if flash_status == "PASS":
            break

        action = "erase" if erase_only else "flash"
        error = f"{action} failed with return code {flash_returncode}"
        if attempt >= max(1, flash_retries):
            break
        show_cue(
            f"Slot {port.slot}: retrying {'erase' if erase_only else 'flash'}",
            (
                f"Previous {'erase' if erase_only else 'flash'} attempt failed.\n\n"
                "Get ready to hold the board button again for the retry."
            ),
            cue_secs,
        )

    if flash_status != "PASS":
        return SlotResult(
            slot=port.slot,
            port=port.device,
            location=port.location,
            status="FAIL",
            flash_status=flash_status,
            flash_returncode=flash_returncode,
            flash_duration_secs=flash_duration_secs,
            setup_status="SKIPPED",
            setup_duration_secs=0.0,
            pubkey="",
            log_path=str(log_path),
            error=error,
        )

    if erase_only:
        return SlotResult(
            slot=port.slot,
            port=port.device,
            location=port.location,
            status="PASS",
            flash_status="PASS",
            flash_returncode=flash_returncode,
            flash_duration_secs=flash_duration_secs,
            setup_status="SKIPPED",
            setup_duration_secs=0.0,
            pubkey="",
            log_path=str(log_path),
            error="",
        )

    if setup_pin:
        setup_status, setup_duration_secs, pubkey, setup_error = setup_pin_and_verify(
            port=port,
            setup_pin=setup_pin,
            baud=baud,
            read_timeout=read_timeout,
            action_timeout=action_timeout,
            cue_secs=cue_secs,
            log_path=log_path,
        )
        if setup_status != "PASS":
            return SlotResult(
                slot=port.slot,
                port=port.device,
                location=port.location,
                status="FAIL",
                flash_status="PASS",
                flash_returncode=flash_returncode,
                flash_duration_secs=flash_duration_secs,
                setup_status=setup_status,
                setup_duration_secs=setup_duration_secs,
                pubkey="",
                log_path=str(log_path),
                error=setup_error,
            )

    return SlotResult(
        slot=port.slot,
        port=port.device,
        location=port.location,
        status="PASS",
        flash_status="PASS",
        flash_returncode=flash_returncode,
        flash_duration_secs=flash_duration_secs,
        setup_status=setup_status,
        setup_duration_secs=setup_duration_secs,
        pubkey=pubkey,
        log_path=str(log_path),
        error="",
    )


def parse_args() -> argparse.Namespace:
    ap = argparse.ArgumentParser(
        description="Simple sequential hub flasher for the dedicated ESP32 dev-board USB-C hub.",
    )
    ap.add_argument("--profile", default="", help="Profile id from flash_profiles.json.")
    ap.add_argument("--config", default=str(DEFAULT_CONFIG_PATH), help="Path to flash_profiles.json.")
    ap.add_argument("--list", action="store_true", help="List detected hub slots and exit.")
    ap.add_argument("--all", action="store_true", help="Flash all detected hub slots.")
    ap.add_argument("--slots", default="", help="Comma or space separated slot list, for example: 1,3,4")
    ap.add_argument("--reverse", action="store_true", help="Process the selected slots in reverse order.")
    ap.add_argument("--batch-tag", default="", help="Batch tag to record in logs.")
    ap.add_argument(
        "--skip-build",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Reuse existing build artifacts. Defaults to the profile setting.",
    )
    ap.add_argument(
        "--skip-efuse-check",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Pass --skip-efuse-check through to provision_dev.sh. Defaults to the profile setting.",
    )
    ap.add_argument(
        "--setup-pin",
        default="",
        help="Optional six-digit PIN to set on each flashed board and verify over UART.",
    )
    ap.add_argument(
        "--erase-only",
        action="store_true",
        help="Just erase flash on each selected slot. No reflash, no UART verification.",
    )
    ap.add_argument(
        "--reset-only",
        action="store_true",
        help="Erase and reflash blank dev firmware only. This is the default when --setup-pin is omitted.",
    )
    ap.add_argument("--baud", type=int, default=115200, help="Baud rate for post-flash setup checks.")
    ap.add_argument("--read-timeout", type=float, default=4.0, help="Read timeout for normal UART commands.")
    ap.add_argument(
        "--action-timeout",
        type=float,
        default=35.0,
        help="Read timeout for button-confirmed UART commands like SET_MODE:PIN.",
    )
    ap.add_argument(
        "--flash-retries",
        type=int,
        default=2,
        help="Maximum flash attempts per slot. Default: 2.",
    )
    ap.add_argument(
        "--cue-seconds",
        type=float,
        default=DEFAULT_CUE_SECS,
        help="How long the non-blocking cue popup stays visible before each manual button step. Use 0 to disable.",
    )
    ap.add_argument(
        "--continue-on-fail",
        action="store_true",
        help="Continue to the next slot if one slot fails. Default is stop on first failure.",
    )
    return ap.parse_args()


def main() -> int:
    args = parse_args()
    if args.setup_pin and (len(args.setup_pin) != 6 or not args.setup_pin.isdigit()):
        raise SystemExit("--setup-pin must be exactly 6 digits")
    if args.erase_only and args.setup_pin:
        raise SystemExit("--erase-only cannot be used together with --setup-pin")
    if args.erase_only and args.reset_only:
        raise SystemExit("--erase-only cannot be used together with --reset-only")
    if args.reset_only and args.setup_pin:
        raise SystemExit("--reset-only cannot be used together with --setup-pin")
    if args.flash_retries < 1:
        raise SystemExit("--flash-retries must be >= 1")

    config_path = resolve_path(APP_ROOT, args.config)
    profile = load_profile(config_path, args.profile or None)
    ports = discover_hub_ports(profile)

    print_ports(profile, ports)
    if args.list or (not args.all and not args.slots):
        if not args.list:
            print()
            print("Use --all to flash every detected hub slot, or --slots 1,2 to target specific slots.")
        return 0

    slot_filter = parse_requested_slots(args.slots) if args.slots else []
    selected_ports = select_ports(ports, slot_filter, args.all, args.reverse)
    if not selected_ports:
        print("No matching slots selected.", file=sys.stderr)
        return 1

    project_root = resolve_path(APP_ROOT, str(profile.get("project_root") or "."))
    target_dir_raw = str(profile.get("target_dir") or "target")
    target_dir = resolve_path(project_root, target_dir_raw)
    batch_tag = args.batch_tag.strip() or default_batch_tag(str(profile.get("batch_tag_prefix") or "dev"))
    skip_build = bool(profile.get("default_skip_build", True)) if args.skip_build is None else bool(args.skip_build)
    skip_efuse_check = (
        bool(profile.get("default_skip_efuse_check", True))
        if args.skip_efuse_check is None
        else bool(args.skip_efuse_check)
    )

    if not args.erase_only:
        validate_artifact(project_root, target_dir, profile, skip_build=skip_build)

    run_dir = project_root / "logs" / "hub-runs" / f"{utc_timestamp(compact=True)}-{sanitize_label(batch_tag)}"
    run_dir.mkdir(parents=True, exist_ok=True)

    print()
    print(f"Batch tag:   {batch_tag}")
    print(f"Run dir:     {run_dir}")
    if args.erase_only:
        print("Mode:        sequential hub erase test")
    else:
        print("Mode:        sequential hub flashing")
    if not args.erase_only:
        print(f"Skip build:  {'yes' if skip_build else 'no'}")
        print(f"Skip eFuse:  {'yes' if skip_efuse_check else 'no'}")
    print(f"Flash retry: {args.flash_retries}")
    print(f"Cue popup:   {args.cue_seconds:.1f}s")
    print(f"Slots:       {', '.join(port.slot for port in selected_ports)}")
    if args.erase_only:
        print("Post-flash:  none (erase only)")
    elif args.setup_pin:
        print("Post-flash:  set PIN and verify app-ready UART flow")
    else:
        print("Post-flash:  blank reset only")

    results: list[SlotResult] = []
    for port in selected_ports:
        result = process_slot(
            project_root=project_root,
            target_dir=target_dir,
            profile=profile,
            port=port,
            batch_tag=batch_tag,
            skip_efuse_check=skip_efuse_check,
            run_dir=run_dir,
            flash_retries=args.flash_retries,
            setup_pin=args.setup_pin,
            baud=args.baud,
            read_timeout=args.read_timeout,
            action_timeout=args.action_timeout,
            cue_secs=args.cue_seconds,
            erase_only=args.erase_only,
        )
        results.append(result)
        if result.status != "PASS" and not args.continue_on_fail:
            break

    write_summary(run_dir, results)

    passed = sum(1 for result in results if result.status == "PASS")
    print()
    print(f"Summary: {passed}/{len(results)} slot(s) passed")
    if args.setup_pin:
        for result in results:
            if result.pubkey:
                print(f"  slot {result.slot}: pubkey {result.pubkey}")
    print(f"Run logs: {run_dir}")

    return 0 if passed == len(results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
