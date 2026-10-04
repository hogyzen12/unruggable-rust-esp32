#!/usr/bin/env python3
"""Build once and package the V2 signer and diagnostic for repeatable flashing."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import sys
from datetime import datetime, timezone

ROOT = Path(__file__).resolve().parents[2]
FIRMWARE = ROOT / "firmware/iphone-ble-signer"
TARGET = "xtensa-esp32s3-espidf"
sys.path.insert(0, str(FIRMWARE / "scripts"))
from prepare_bootloader import with_integrity_digest


def output(*args, cwd=ROOT):
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_files():
    names = output("git", "ls-files", "--cached", "--others", "--exclude-standard", "--",
                   "firmware/iphone-ble-signer", "tools/firmware-production").splitlines()
    return sorted(set(name for name in names if (ROOT / name).is_file()))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="new package directory")
    parser.add_argument("--allow-dirty", action="store_true", help="package an explicitly marked candidate")
    parser.add_argument("--qualification", action="store_true", help="enable direct chip probes; NEVER a shipping image")
    args = parser.parse_args()
    destination = args.output.resolve()
    if destination.exists():
        parser.error("output already exists; use a new directory to preserve earlier artifacts")
    # Reject inherited pin overrides rather than silently manufacturing another board variant.
    overrides = [key for key in os.environ if key.startswith("SIGNER_")]
    if overrides:
        parser.error("unset inherited firmware overrides first: " + ", ".join(overrides))
    version = output("espflash", "--version")
    if version != "espflash 3.3.0":
        parser.error("this workflow is qualified for espflash 3.3.0; install that version")
    dirty = bool(output("git", "status", "--porcelain", "--untracked-files=all", "--",
                        "firmware/iphone-ble-signer", "tools/firmware-production"))
    if dirty and not args.allow_dirty:
        parser.error("firmware/tooling has uncommitted changes; commit or use --allow-dirty for a candidate")
    records = {name: digest(ROOT / name) for name in source_files()}
    source_hash = hashlib.sha256(json.dumps(records, sort_keys=True).encode()).hexdigest()
    build_id = "v2-" + source_hash[:16] + ("-qual" if args.qualification else "")
    destination.mkdir(parents=True)
    env = dict(os.environ, SIGNER_BOARD_PROFILE="dual-element-se050", SIGNER_BUILD_ID=build_id,
               ESP_IDF_VERSION="v5.2.3")
    command = ["cargo", "+esp", "build", "--locked", "--release", "--message-format=json-render-diagnostics",
               "--bin", "esp32-solana-signer", "--bin", "peripheral_probe"]
    if args.qualification:
        command += ["--features", "qualification"]
    # Use Cargo's build-script result, not a glob that might choose an obsolete SDK build.
    sdk_out = None
    with (destination / "build.log").open("w") as log:
        process = subprocess.Popen(command, cwd=FIRMWARE, env=env, stdout=subprocess.PIPE,
                                   stderr=log, text=True)
        for line in process.stdout:
            log.write(line)
            try:
                event = json.loads(line)
            except ValueError:
                continue
            if event.get("reason") == "build-script-executed" and "esp-idf-sys" in event.get("package_id", ""):
                sdk_out = Path(event["out_dir"])
        if process.wait() != 0:
            raise RuntimeError(f"build failed; see {destination / 'build.log'}")
    if sdk_out is None:
        raise RuntimeError("Cargo did not identify the SDK build output")
    description = json.loads((sdk_out / "build/project_description.json").read_text())
    version_header = (Path(description["idf_path"]) / "components/esp_common/include/esp_idf_version.h").read_text()
    for part, value in (("MAJOR", 5), ("MINOR", 2), ("PATCH", 3)):
        if not re.search(rf"^#define ESP_IDF_VERSION_{part}\s+{value}\s*$", version_header, re.M):
            raise RuntimeError("actual ESP-IDF checkout does not match v5.2.3")
    after = {name: digest(ROOT / name) for name in source_files()}
    if after != records:
        raise RuntimeError("source changed during build; rerun into a new package directory")
    copies = {
        "signer.elf": FIRMWARE / "target" / TARGET / "release/esp32-solana-signer",
        "diagnostic.elf": FIRMWARE / "target" / TARGET / "release/peripheral_probe",
        "bootloader-idf.bin": sdk_out / "build/bootloader/bootloader.bin",
        "partition-table.bin": sdk_out / "build/partition_table/partition-table.bin",
        "sdkconfig": sdk_out / "sdkconfig",
        "partitions.csv": FIRMWARE / "partitions/unified_wallet_4mb.csv",
        "Cargo.lock": FIRMWARE / "Cargo.lock",
        "factory.py": Path(__file__).with_name("factory.py"),
        "README.md": Path(__file__).with_name("README.md"),
    }
    for name, source in copies.items():
        shutil.copyfile(source, destination / name)
    (destination / "bootloader.bin").write_bytes(
        with_integrity_digest((destination / "bootloader-idf.bin").read_bytes()))
    config = (destination / "sdkconfig").read_text()
    for required in ['CONFIG_IDF_TARGET="esp32s3"', 'CONFIG_ESPTOOLPY_FLASHSIZE="4MB"',
                     'CONFIG_BT_ENABLED=y', 'CONFIG_BT_BLUEDROID_ENABLED=y']:
        if required not in config.splitlines():
            raise RuntimeError(f"unexpected generated configuration: missing {required}")
    if re.search(r"^CONFIG_(SECURE_BOOT|SECURE_FLASH_ENC_ENABLED|NVS_ENCRYPTION)=y$", config, re.M):
        raise RuntimeError("package must preserve the agreed open-reflash configuration")
    # Standalone app binaries are useful for fixtures/programmers. No merged image:
    # padding one across 0x9000–0xffff would overwrite an existing wallet's NVS.
    for image in ("signer", "diagnostic"):
        subprocess.run(["espflash", "save-image", "--skip-update-check", "--chip", "esp32s3",
                        "--flash-size", "4mb", "--flash-mode", "dio", "--flash-freq", "80mhz",
                        "--bootloader", str(destination / "bootloader.bin"),
                        "--partition-table", str(destination / "partitions.csv"),
                        str(destination / f"{image}.elf"), str(destination / f"{image}.bin")], check=True)
    with tarfile.open(destination / "sources.tar.gz", "w:gz") as archive:
        for name in records:
            archive.add(ROOT / name, arcname=name)
    files = {path.name: {"sha256": digest(path), "bytes": path.stat().st_size}
             for path in sorted(destination.iterdir()) if path.is_file()}
    manifest = {
        "schema": 1, "build_id": build_id, "board": "dual-element-se050", "chip": "esp32s3",
        "status": "qualification-only" if args.qualification else ("candidate" if dirty else "built-unqualified"),
        "source_dirty": dirty, "qualification_commands": args.qualification,
        "release_blockers": ["Chip-enforced theft protection and permanent PIN lockout qualification required",
                             "Full app, new-unit station and interrupted-setup qualification required"],
        "git_revision": output("git", "rev-parse", "HEAD"), "source_sha256": source_hash,
        "source_files": records, "created_utc": datetime.now(timezone.utc).isoformat(),
        "espflash": version, "rustc": output("rustc", "+esp", "--version"),
        "esp_idf": "v5.2.3", "profile": "release", "flash_size": "4mb",
        "release_scope": "200-unit open-hardware hackathon prize order",
        "capabilities": {"solana_ed25519": True, "usb": True, "ble": True,
                         "falcon": False, "open_reflash": True, "reset": False,
                         "pin_attempt_limit": 10, "lockout_recovery": False},
        "flash_mode": "dio", "flash_freq": "80mhz", "minimum_physical_flash_bytes": 4194304,
        "files": files,
    }
    (destination / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Packaged {build_id}: {destination}\nStatus: {manifest['status']}; hardware acceptance is still required.")


if __name__ == "__main__":
    main()
