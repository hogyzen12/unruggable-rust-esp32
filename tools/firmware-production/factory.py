#!/usr/bin/env python3
"""Verify a package, flash an existing wallet, or run a new-unit PCB/blank-state check."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time
import uuid

REQUIRED_FILES = {"signer.elf", "diagnostic.elf", "bootloader.bin", "partition-table.bin",
                  "partitions.csv", "sdkconfig", "signer.bin", "diagnostic.bin"}
FACTORY_COMPONENTS = {"PCB", "ESP32", "SE050", "DISPLAY", "BUTTON", "TROPIC"}
MAC_PATTERN = re.compile(r"(?:[0-9A-F]{2}:){5}[0-9A-F]{2}\Z")


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify_package(package):
    manifest = json.loads((package / "manifest.json").read_text())
    expected = {"schema": 1, "board": "dual-element-se050", "chip": "esp32s3", "profile": "release",
                "flash_size": "4mb", "flash_mode": "dio", "flash_freq": "80mhz", "esp_idf": "v5.2.3"}
    for key, value in expected.items():
        if manifest.get(key) != value:
            raise ValueError(f"unexpected package {key}: {manifest.get(key)!r}")
    files = manifest.get("files", {})
    if not REQUIRED_FILES.issubset(files):
        raise ValueError("incomplete firmware package")
    for name, metadata in files.items():
        if Path(name).name != name or name in (".", ".."):
            raise ValueError("package file must be a plain filename")
        path = package / name
        if path.is_symlink() or not path.is_file():
            raise ValueError(f"missing/non-regular package artifact: {name}")
        if path.stat().st_size != metadata["bytes"] or sha256(path) != metadata["sha256"]:
            raise ValueError(f"package artifact changed: {name}")
    return manifest


def flash_command(package, port, image):
    return ["espflash", "flash", "--skip-update-check", "--chip", "esp32s3", "--port", port,
            "--before", "default-reset", "--after", "hard-reset", "--flash-size", "4mb",
            "--flash-mode", "dio", "--flash-freq", "80mhz", "--bootloader", str(package / "bootloader.bin"),
            "--partition-table", str(package / "partitions.csv"), str(package / f"{image}.elf")]


def validate_action(manifest, action, allow_candidate):
    qualification = manifest.get("qualification_commands", False) or manifest["status"] == "qualification-only"
    if action == "run" and qualification:
        raise ValueError("qualification firmware cannot be used for the production station run")
    if (manifest["status"] == "candidate" or qualification) and not allow_candidate:
        raise ValueError("use --allow-candidate for a development-board candidate")


def parse_factory_result(line):
    if not line.startswith("FACTORY_RESULT "):
        raise ValueError("missing factory result")
    fields = dict(part.split("=", 1) for part in line.split()[1:])
    if set(fields) != FACTORY_COMPONENTS or any(value != "PASS" for value in fields.values()):
        raise ValueError(f"PCB did not pass every fitted component: {fields}")
    return fields


def parse_info(line):
    if not line.startswith("INFO;"):
        raise ValueError("missing INFO response")
    return dict(part.split("=", 1) for part in line.split(";")[1:] if "=" in part)


def validate_info(info, build_id, require_blank):
    expected = {"BUILD": build_id, "BOARD": "dual-element-se050", "PROTOCOL": "2",
                "BLE_SECURITY": "SC_MITM_BOND", "SESSION_BOUND": "1", "TX_V1": "1",
                "MAX_SIGN_BYTES": "4096", "PIN_BACKEND": "SE050", "SOLANA_BACKEND": "TROPIC",
                "PQC_SUPPORTED": "0", "PQC_BACKEND": "DISABLED",
                "ACTIVE_TRANSPORT": "USB", "WIPE_PENDING": "0"}
    if require_blank:
        expected.update(AUTH_MODE="UNSET", FINALIZED="0", LOCKED="0", RETRIES_LEFT="10",
                        KEY_STATE="UNINITIALIZED", PQC_STATE="UNINITIALIZED", TROPIC_ACCESS="MISSING",
                        AUTH_STORE="EMPTY", QUALIFICATION="0", PIN_ATTEMPT_LIMIT="10",
                        RESET_SUPPORTED="0", RESET_POLICY="PERMANENT_LOCKOUT", BLE_BONDS="0")
    for key, value in expected.items():
        if info.get(key) != value:
            raise ValueError(f"status {key}: expected {value}, got {info.get(key)!r}")
    if info.get("KEY_STATE") == "FAULT" or info.get("PQC_STATE") == "FAULT":
        raise ValueError("secure-element/PQC fault")
    mac = info.get("DEVICE_MAC", "")
    if not MAC_PATTERN.fullmatch(mac) or mac in ("00:00:00:00:00:00", "FF:FF:FF:FF:FF:FF"):
        raise ValueError("missing/invalid factory MAC from firmware")


def open_serial(port):
    import serial
    connection = serial.Serial(port=None, baudrate=115200, timeout=0.2, write_timeout=2)
    connection.dtr = False
    connection.rts = False
    connection.port = port
    connection.open()
    return connection


def lines_until(connection, deadline):
    pending = bytearray()
    while time.monotonic() < deadline:
        chunk = connection.read(1)
        if not chunk:
            continue
        if chunk == b"\n":
            yield pending.decode("utf-8", errors="replace").strip()
            pending.clear()
        else:
            pending.extend(chunk)
            if len(pending) > 8192:
                raise ValueError("serial line exceeded limit")


def command(connection, text, expected_prefix, timeout=8):
    connection.reset_input_buffer()
    connection.write((text + "\n").encode("ascii"))
    connection.flush()
    for line in lines_until(connection, time.monotonic() + timeout):
        if line.startswith(expected_prefix):
            return line
        if line.startswith("ERROR:"):
            raise ValueError(f"{text} failed: {line}")
    raise TimeoutError(f"no reply to {text}")


def smoke_check(port, build_id, require_blank, expected_mac=None):
    with open_serial(port) as connection:
        time.sleep(2)  # Secure-element startup after reset.
        if command(connection, "PING", "PONG") != "PONG":
            raise ValueError("invalid PING response")
        info = parse_info(command(connection, "GET_INFO", "INFO;"))
        validate_info(info, build_id, require_blank)
        if expected_mac is not None and info["DEVICE_MAC"] != expected_mac:
            raise ValueError("firmware MAC does not match the ROM MAC")
        if command(connection, "SE050_STATUS", "SE050_OK") != "SE050_OK":
            raise ValueError("SE050 status failed")
        # Public data only; never create a factory PIN or wallet key.
        public_key = command(connection, "GET_PUBKEY", "ERROR:WALLET_NOT_INITIALIZED" if require_blank else ("PUBKEY:" if info["KEY_STATE"] == "READY" else "ERROR:WALLET_NOT_INITIALIZED"))
        if require_blank and public_key != "ERROR:WALLET_NOT_INITIALIZED":
            raise ValueError("factory unit already has a wallet")
        audit = None
        if require_blank:
            line = command(connection, "TROPIC_AUDIT", "TROPIC_AUDIT;", timeout=30)
            audit = dict(part.split("=", 1) for part in line.split(";")[1:])
            validate_blank_permissions(audit)
        if command(connection, "LOCK", "LOCKED") != "LOCKED":
            raise ValueError("LOCK failed")
        return {"info": info, "public_key_response": public_key, "lock": "PASS", "permissions": audit}


def validate_blank_permissions(audit):
    if audit.get("SLOT0") != "OCCUPIED" or any(audit.get("SLOT" + str(slot)) != "EMPTY" for slot in (1, 2, 3)):
        raise ValueError("factory board has used or invalid customer pairing slots")
    for name in ("ECC_GENERATE", "ECC_STORE", "ECC_ERASE", "EDDSA_SIGN"):
        if int(audit.get("I_" + name, "0"), 16) & 0xf != 0xf:
            raise ValueError("factory board already has permanent wallet restrictions")


def run_command(command_args, record):
    result = subprocess.run(command_args, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    record.setdefault("tool_output", []).append(result.stdout)
    print(result.stdout, end="", flush=True)
    if result.returncode:
        raise RuntimeError(f"{command_args[1]} failed with exit {result.returncode}")
    return result.stdout


def board_identity(port, record):
    result = run_command(["espflash", "board-info", "--skip-update-check", "--port", port], record)
    mac = re.search(r"MAC address:\s*([0-9a-fA-F:]{17})", result)
    flash = re.search(r"Flash size:\s*(\d+)\s*MB", result, re.I)
    if not re.search(r"Chip type:\s*esp32s3", result, re.I) or not mac or not flash:
        raise ValueError("could not identify ESP32-S3 MAC/flash size")
    if int(flash[1]) < 4:
        raise ValueError("physical flash is smaller than the pinned 4 MB layout")
    return {"mac": mac[1].upper(), "physical_flash_mb": int(flash[1])}


def compact_record(record):
    saved = {name: record[name] for name in (
        "schema", "action", "lot", "mac", "build_id", "manifest_sha256", "result"
    ) if name in record}
    saved["at_utc"] = record["finished_utc"]
    if record.get("result") == "PASS" and record.get("action") == "run":
        saved["checks"] = {"pcb": "PASS", "blank_wallet": "PASS",
                           "no_phone_bonds": "PASS", "device_identity": "PASS"}
    elif "smoke" in record:
        # Update/check records retain public before/after evidence for owner devices.
        saved["smoke"] = record["smoke"]
    if record.get("result") != "PASS":
        for name in ("stage", "error", "tool_output", "diagnostic_log"):
            if name in record:
                saved[name] = record[name]
    return saved


def save_record(directory, record):
    directory.mkdir(parents=True, exist_ok=True)
    mac = record.get("mac", "UNKNOWN").replace(":", "")
    path = directory / (mac + "-" + datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S") + "-" + uuid.uuid4().hex[:8] + ".json")
    with path.open("x") as file:
        json.dump(compact_record(record), file, indent=2)
        file.write("\n")
        file.flush()
        os.fsync(file.fileno())
    print(f"Record: {path}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("verify", "flash", "check", "run"))
    parser.add_argument("--package", type=Path, default=Path(__file__).resolve().parent)
    parser.add_argument("--port")
    parser.add_argument("--lot", help="defaults to the records folder name; no per-board data entry")
    parser.add_argument("--expect-mac")
    parser.add_argument("--records", type=Path, default=Path("factory-records"))
    parser.add_argument("--allow-candidate", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    package = args.package.resolve()
    manifest = verify_package(package)
    print(f"Package verified: {manifest['build_id']} ({manifest['status']})")
    if args.action == "verify":
        return
    if not args.port:
        parser.error("--port is required; automatic selection is disabled")
    validate_action(manifest, args.action, args.allow_candidate)
    if args.dry_run:
        images = ("diagnostic", "signer") if args.action == "run" else (("signer",) if args.action == "flash" else ())
        for image in images:
            print(json.dumps(flash_command(package, args.port, image)))
        print("No device opened. No erase operation. run additionally requires full PCB pass and blank signer state.")
        return
    actual_version = subprocess.check_output(["espflash", "--version"], text=True).strip()
    if actual_version != manifest["espflash"]:
        raise ValueError(f"install {manifest['espflash']} before using this package")
    record = {"schema": 2, "started_utc": datetime.now(timezone.utc).isoformat(),
              "action": args.action, "lot": args.lot or args.records.resolve().name,
              "port": args.port, "build_id": manifest["build_id"],
              "manifest_sha256": sha256(package / "manifest.json"), "package_status": manifest["status"],
              "result": "FAIL", "shipping_approved": False}
    try:
        # check deliberately does not reset the board: inspect its current state.
        record["stage"] = "identify"
        if args.action != "check":
            record.update(board_identity(args.port, record))
            if args.expect_mac and record["mac"] != args.expect_mac.upper():
                raise ValueError("connected MAC does not match --expect-mac")
        elif args.expect_mac:
            raise ValueError("--expect-mac requires flash/run; check does not reset or query the ROM")
        if args.action == "run":
            record["stage"] = "diagnostic_flash"
            run_command(flash_command(package, args.port, "diagnostic"), record)
            record["stage"] = "diagnostic_test"
            print("Inspect the display's colors/text/border. Press and release the board button ONLY if correct.", flush=True)
            with open_serial(args.port) as connection:
                for line in lines_until(connection, time.monotonic() + 75):
                    print(line, flush=True)
                    record.setdefault("diagnostic_log", []).append(line)
                    if line.startswith("FACTORY_RESULT "):
                        record["pcb"] = parse_factory_result(line)
                        break
                else:
                    raise TimeoutError("diagnostic result not received; do not pass this unit")
        if args.action in ("flash", "run"):
            record["stage"] = "signer_flash"
            verify_package(package)  # Recheck before the second flash as well.
            run_command(flash_command(package, args.port, "signer"), record)
        record["stage"] = "blank_check" if args.action == "run" else "status_check"
        record["smoke"] = smoke_check(args.port, manifest["build_id"], args.action == "run", record.get("mac"))
        record["mac"] = record["smoke"]["info"]["DEVICE_MAC"]
        record["result"] = "PASS"
        print(f"PASS {record['mac']} — {manifest['build_id']}")
    except (Exception, KeyboardInterrupt) as error:
        record["error"] = str(error)
        raise
    finally:
        record["finished_utc"] = datetime.now(timezone.utc).isoformat()
        save_record(args.records, record)


if __name__ == "__main__":
    main()
