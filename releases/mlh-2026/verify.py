#!/usr/bin/env python3
"""Read-only verification of the exact MLH firmware source and optional package."""
import argparse
import hashlib
import json
from pathlib import Path
import tarfile

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = Path(__file__).with_name("manifest.json")
EXPECTED_MANIFEST = "10670a2ee95aa947c6ca6ac328fefd7502d621d8ea9b5b7b1fe610063fe1efe9"
EXPECTED_BUILD = "v2-ff212385a4d13c18"


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def checked_path(root, name):
    relative = Path(name)
    if relative.is_absolute() or ".." in relative.parts:
        raise ValueError(f"unsafe manifest path: {name}")
    path = root / relative
    if path.is_symlink() or not path.is_file() or not path.resolve().is_relative_to(root.resolve()):
        raise ValueError(f"missing/non-regular file: {name}")
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", type=Path, help="optional extracted firmware/ directory")
    args = parser.parse_args()
    original = MANIFEST.read_bytes()
    if sha256(original) != EXPECTED_MANIFEST:
        raise ValueError("MLH manifest changed")
    manifest = json.loads(original)
    if manifest["build_id"] != EXPECTED_BUILD or len(manifest["source_files"]) != 78:
        raise ValueError("unexpected MLH source identity")
    source_hash = sha256(json.dumps(manifest["source_files"], sort_keys=True).encode())
    if source_hash != manifest["source_sha256"] or "v2-" + source_hash[:16] != EXPECTED_BUILD:
        raise ValueError("source hash/build identity mismatch")
    for name, expected in manifest["source_files"].items():
        if sha256(checked_path(ROOT, name).read_bytes()) != expected:
            raise ValueError(f"source changed: {name}")
    print(f"PASS: all 78 source files match {EXPECTED_BUILD}")
    if args.package:
        package = args.package.resolve()
        if checked_path(package, "manifest.json").read_bytes() != original:
            raise ValueError("package manifest differs from the shipped manifest")
        for name, meta in manifest["files"].items():
            data = checked_path(package, name).read_bytes()
            if len(data) != meta["bytes"] or sha256(data) != meta["sha256"]:
                raise ValueError(f"package artifact changed: {name}")
        with tarfile.open(package / "sources.tar.gz", "r:gz") as archive:
            members = archive.getmembers()
            if len(members) != 78 or any(not member.isfile() for member in members):
                raise ValueError("unexpected source archive contents")
            entries = {member.name: member for member in members}
            if set(entries) != set(manifest["source_files"]):
                raise ValueError("source archive file list differs")
            for name, expected in manifest["source_files"].items():
                if sha256(archive.extractfile(entries[name]).read()) != expected:
                    raise ValueError(f"archived source changed: {name}")
        print(f"PASS: all {len(manifest['files'])} package artifacts and 78 archived sources match")


if __name__ == "__main__":
    main()
