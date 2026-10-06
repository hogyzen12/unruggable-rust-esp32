#!/usr/bin/env python3
"""Verify both preserved firmware families without building or accessing hardware."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
ESP32_RELEASE = ROOT / "firmware/esp32-only/releases/v0.1.0"
ESP32_MANIFEST_SHA256 = "9ac6065626db259aa41c7d9b2ac85d1e8cf51eaf24a5b3b4ad2bfb199743b52f"
DUAL_COMMIT = "dd1f86d6633b66e203e8c78f5cd3130eb67ab98c"
DUAL_TAG = "firmware-v0.3.3-mlh-2026"
DUAL_MANIFEST_SHA256 = "10670a2ee95aa947c6ca6ac328fefd7502d621d8ea9b5b7b1fe610063fe1efe9"


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


def release_git(*args):
    result = subprocess.run(["git", "-C", str(ROOT), *args], capture_output=True)
    if result.returncode:
        raise ValueError("Original V2 history/tag unavailable; run git fetch origin --unshallow --tags "
                         "for a shallow clone, or git fetch origin --tags for a full clone")
    return result.stdout


def verify_dual(package):
    # Verify shipped V2 at its pinned commit; allow working V2 source to evolve.
    if release_git("rev-parse", f"refs/tags/{DUAL_TAG}^{{commit}}").decode().strip() != DUAL_COMMIT:
        raise ValueError("Original V2 release tag moved")
    manifest_path = "releases/mlh-2026/manifest.json"
    original = release_git("show", f"{DUAL_COMMIT}:{manifest_path}")
    if sha256(original) != DUAL_MANIFEST_SHA256:
        raise ValueError("Original V2 release manifest changed")
    if checked_path(ROOT, manifest_path).read_bytes() != original:
        raise ValueError("Preserved V2 manifest changed in the working tree")
    verifier_path = "releases/mlh-2026/verify.py"
    verifier = release_git("show", f"{DUAL_COMMIT}:{verifier_path}")
    if checked_path(ROOT, verifier_path).read_bytes() != verifier:
        raise ValueError("Preserved V2 verifier changed in the working tree")
    manifest = json.loads(original)
    with tempfile.TemporaryDirectory(prefix="unruggable-v033-") as temporary:
        snapshot = Path(temporary)
        for name in [*manifest["source_files"], manifest_path, verifier_path]:
            relative = Path(name)
            if relative.is_absolute() or ".." in relative.parts:
                raise ValueError(f"Unsafe release path: {name}")
            target = snapshot / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(release_git("show", f"{DUAL_COMMIT}:{name}"))
        command = [sys.executable, str(snapshot / verifier_path)]
        if package:
            command.extend(["--package", str(package.resolve())])
        subprocess.run(command, check=True)
    print("PASS: dual-element v0.3.3 — immutable release verified independently of current V2 source", flush=True)


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
