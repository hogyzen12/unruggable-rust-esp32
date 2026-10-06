#!/usr/bin/env python3
"""Verify the recovered research source; does not build, flash or contact a chain."""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
MANIFEST_SHA256 = "500c5c2f7c4acef4e46c97a173f945a680dea73637ce098d9168b033318c24a2"

def main():
    raw = (ROOT / "source-provenance.json").read_bytes()
    if hashlib.sha256(raw).hexdigest() != MANIFEST_SHA256:
        raise ValueError("Research provenance manifest changed")
    files = json.loads(raw)["files"]
    for name, meta in files.items():
        relative = Path(name)
        path = ROOT / relative
        if relative.is_absolute() or ".." in relative.parts or path.is_symlink() or not path.is_file() or not path.resolve().is_relative_to(ROOT.resolve()):
            raise ValueError(f"Missing or unsafe snapshot file: {name}")
        data = path.read_bytes()
        if len(data) != meta["bytes"] or hashlib.sha256(data).hexdigest() != meta["sha256"]:
            raise ValueError(f"Research source differs from recovery snapshot: {name}")
    print(f"PASS: {len(files)} recovered Falcon/SE050 research files match")

if __name__ == "__main__":
    main()
