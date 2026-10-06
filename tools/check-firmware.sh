#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
export PYTHONDONTWRITEBYTECODE=1

python3 tools/firmware-handoff/handoff.py verify
cargo +1.92.0 test --locked --manifest-path tools/firmware-host-tests/Cargo.toml
python3 -m unittest discover -s tools/firmware-production -v
python3 -m unittest discover -s tools/firmware-lot -v
python3 -m unittest discover -s tools/firmware-handoff -v
python3 firmware/verify.py
