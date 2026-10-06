#!/usr/bin/env python3
"""Verify both preserved firmware families without building or accessing hardware."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
ESP32_RELEASE = ROOT / "firmware/esp32-only/releases/v0.1.0"
ESP32_MANIFEST_SHA256 = "9ac6065626db259aa41c7d9b2ac85d1e8cf51eaf24a5b3b4ad2bfb199743b52f"


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def checked_path(root, name):
    relative = Path(name)
    if relative.is_absolute() or ".." in relative.parts:
        raise ValueError(f"Unsafe manifest path: {name}")
    path = root / relative
    if path.is_symlink() or not path.is_file() or not path.resolve().is_relative_to(root.resolve()):
        raise ValueError(f"Missing/non-regular release file: {name}")
    return path


def verify_esp32():
    raw = checked_path(ESP32_RELEASE, "manifest.json").read_bytes()
    if sha256(raw) != ESP32_MANIFEST_SHA256:
        raise ValueError("ESP32-only preorder manifest changed")
    manifest = json.loads(raw)
    checksums = {"manifest.json": ESP32_MANIFEST_SHA256}
    for name, meta in manifest["files"].items():
        data = checked_path(ESP32_RELEASE, name).read_bytes()
        if len(data) != meta["bytes"] or sha256(data) != meta["sha256"]:
            raise ValueError(f"ESP32-only preorder file changed: {name}")
        checksums[name] = meta["sha256"]
    expected = "".join(f"{digest}  {name}\n" for name, digest in sorted(checksums.items()))
    if checked_path(ESP32_RELEASE, "SHA256SUMS").read_text() != expected:
        raise ValueError("ESP32-only SHA256SUMS differs from its pinned manifest")
    print(f"PASS: ESP32-only v0.1.0 — {len(manifest['files'])} archived files unchanged", flush=True)


def verify_dual(package):
    command = [sys.executable, str(ROOT / "releases/mlh-2026/verify.py")]
    if package:
        command.extend(["--package", str(package.resolve())])
    subprocess.run(command, check=True)
    print("PASS: dual-element v0.3.3 — original MLH release verified", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--family", choices=["all", "esp32-only", "dual-element"], default="all")
    parser.add_argument("--dual-package", type=Path, help="optional extracted V2 package firmware/ directory")
    args = parser.parse_args()
    if args.family == "esp32-only" and args.dual_package:
        parser.error("--dual-package requires --family all or dual-element")
    if args.family in ("all", "esp32-only"):
        verify_esp32()
    if args.family in ("all", "dual-element"):
        verify_dual(args.dual_package)


if __name__ == "__main__":
    main()
