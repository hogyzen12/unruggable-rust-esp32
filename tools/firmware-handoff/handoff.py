#!/usr/bin/env python3
"""Preview/export committed V2 source from the app monorepo; verify its provenance."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
OWNED = (
    "firmware/iphone-ble-signer",
    "tools/firmware-host-tests",
    "tools/firmware-production",
    "tools/firmware-lot",
)
RECORD = "firmware/v2-source.json"
REPOSITORY = "hogyzen12/unruggable-app-pure-UX"


def git(root, *args):
    result = subprocess.run(["git", "-C", str(root), *args], capture_output=True)
    if result.returncode:
        raise ValueError(result.stderr.decode().strip())
    return result.stdout


def digest(data):
    return hashlib.sha256(data).hexdigest()


def checked_path(root, name):
    relative = PurePosixPath(name)
    if (relative.is_absolute() or ".." in relative.parts or str(relative) != name
            or any(ord(c) < 32 for c in name) or "\\" in name):
        raise ValueError(f"unsafe path: {name}")
    path = root
    for part in relative.parts:
        path = path / part
        if path.is_symlink():
            raise ValueError(f"symlink is not an export destination: {name}")
        if path != root / relative and path.exists() and not path.is_dir():
            raise ValueError(f"parent is not a directory: {name}")
    return path


def owned(name):
    return any(name.startswith(prefix + "/") for prefix in OWNED)


def clean(root, *paths):
    status = git(root, "status", "--porcelain", "--untracked-files=all", "--", *paths)
    if status:
        raise ValueError(f"uncommitted work in {root}:\n{status.decode().strip()}")


def snapshot(source, revision):
    if not re.fullmatch(r"[0-9a-fA-F]{40}", revision):
        raise ValueError("--revision must be a full 40-character commit SHA (git rev-parse HEAD)")
    commit = git(source, "rev-parse", "--verify", revision + "^{commit}").decode().strip()
    clean(source, *OWNED)
    files = {}
    entries = git(source, "ls-tree", "-rz", commit, "--", *OWNED)
    for entry in entries.split(b"\0"):
        if not entry:
            continue
        metadata, raw_name = entry.split(b"\t", 1)
        mode, kind, oid = metadata.decode().split()
        name = raw_name.decode()
        checked_path(source, name)
        if not owned(name) or kind != "blob" or mode not in ("100644", "100755"):
            raise ValueError(f"only regular tracked files may be exported: {name}")
        files[name] = (git(source, "cat-file", "blob", oid), mode)
    for prefix in OWNED:
        if not any(name.startswith(prefix + "/") for name in files):
            raise ValueError(f"selected commit is missing required tree: {prefix}")
    return commit, files


def record_for(commit, files):
    return {
        "schema": 1,
        "source_repository": REPOSITORY,
        "source_commit": commit,
        "owned_paths": list(OWNED),
        "status": "source-snapshot-not-hardware-qualification",
        "files": {name: {"sha256": digest(data), "mode": mode}
                  for name, (data, mode) in sorted(files.items())},
    }


def export(source, destination, revision, apply=False):
    source, destination = source.resolve(), destination.resolve()
    if source == destination:
        raise ValueError("source and destination must be different repositories")
    commit, files = snapshot(source, revision)
    if apply:
        clean(destination)
    previous = {p.decode() for p in git(destination, "ls-files", "-z", "--", *OWNED).split(b"\0") if p}
    deleted = previous - files.keys()
    changed = []
    for name in sorted(previous | files.keys()):
        path = checked_path(destination, name)
        if name in deleted:
            changed.append(("DELETE", name))
        else:
            data, mode = files[name]
            if path.exists() and not path.is_file():
                raise ValueError(f"destination is not a regular file: {name}")
            # Never overwrite an ignored/untracked file hidden from git status.
            if apply and name not in previous and path.exists():
                raise ValueError(f"untracked destination would be overwritten: {name}")
            actual_mode = "100755" if path.exists() and path.stat().st_mode & 0o111 else "100644"
            if not path.exists() or path.read_bytes() != data or actual_mode != mode:
                changed.append(("UPDATE" if name in previous else "ADD", name))
    manifest = checked_path(destination, RECORD)
    tracked_record = bool(git(destination, "ls-files", "--", RECORD))
    if apply and manifest.exists() and not tracked_record:
        raise ValueError(f"untracked provenance would be overwritten: {RECORD}")
    if manifest.exists() and not manifest.is_file():
        raise ValueError(f"provenance is not a regular file: {RECORD}")
    raw = (json.dumps(record_for(commit, files), indent=2, sort_keys=True) + "\n").encode()
    print(f"Source: {REPOSITORY}@{commit}; {len(files)} committed files")
    for action, name in changed:
        print(f"{action:6} {name}")
    record_changed = not manifest.exists() or manifest.read_bytes() != raw
    print(f"{len(changed)} source changes; provenance {'updates' if record_changed else 'unchanged'}")
    if not apply:
        print("Preview only. Review the commit/diff, then repeat with --apply on a clean destination.")
        return
    for action, name in changed:
        path = checked_path(destination, name)
        if action == "DELETE":
            path.unlink()
        else:
            data, mode = files[name]
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            path.chmod(int(mode[-3:], 8))
    if record_changed:
        manifest.parent.mkdir(parents=True, exist_ok=True)
        manifest.write_bytes(raw)
    print("Applied locally. Review git diff, run tools/check-firmware.sh, then commit/push explicitly.")


def verify(root):
    record = json.loads(checked_path(root, RECORD).read_text())
    if (record.get("schema") != 1 or record.get("source_repository") != REPOSITORY
            or record.get("owned_paths") != list(OWNED)
            or record.get("status") != "source-snapshot-not-hardware-qualification"
            or not re.fullmatch(r"[0-9a-f]{40}", record.get("source_commit", ""))):
        raise ValueError("invalid source provenance")
    files = record["files"]
    actual = {p.decode() for p in git(root, "ls-files", "-co", "--exclude-standard", "-z", "--", *OWNED).split(b"\0") if p}
    if actual != files.keys():
        raise ValueError(f"exported file list changed: {sorted(actual ^ files.keys())}")
    for prefix in OWNED:
        if not any(name.startswith(prefix + "/") for name in files):
            raise ValueError(f"provenance is missing required tree: {prefix}")
    for name, meta in files.items():
        if not owned(name):
            raise ValueError(f"unexpected provenance path: {name}")
        path = checked_path(root, name)
        mode = "100755" if path.stat().st_mode & 0o111 else "100644"
        if not path.is_file() or digest(path.read_bytes()) != meta["sha256"] or mode != meta["mode"]:
            raise ValueError(f"exported source changed: {name}; export a new committed monorepo revision")
    print(f"PASS: {len(files)} exported files match recorded monorepo commit {record['source_commit']}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    command = commands.add_parser("export", help="preview by default; never builds, flashes, commits or pushes")
    command.add_argument("--source", type=Path, required=True)
    command.add_argument("--revision", required=True)
    command.add_argument("--apply", action="store_true")
    commands.add_parser("verify", help="check current source against the recorded export")
    args = parser.parse_args()
    try:
        if args.command == "export":
            export(args.source, ROOT, args.revision, args.apply)
        else:
            verify(ROOT)
    except (ValueError, OSError, KeyError, TypeError) as exc:
        parser.exit(1, f"error: {exc}\n")


if __name__ == "__main__":
    main()
