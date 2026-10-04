#!/usr/bin/env python3
"""Reconcile factory records for the pinned 200-unit candidate; never opens a device."""
import argparse
import csv
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import runpy

MANIFEST_SHA256 = "10670a2ee95aa947c6ca6ac328fefd7502d621d8ea9b5b7b1fe610063fe1efe9"
BUILD_ID = "v2-ff212385a4d13c18"
MAC = re.compile(r"(?:[0-9A-F]{2}:){5}[0-9A-F]{2}\Z")


def load_factory(package):
    manifest_path = package / "manifest.json"
    if hashlib.sha256(manifest_path.read_bytes()).hexdigest() != MANIFEST_SHA256:
        raise ValueError("wrong manifest: this handoff is pinned to the v0.3.3 signing-flow candidate")
    manifest = json.loads(manifest_path.read_text())
    factory_path = package / "factory.py"
    if hashlib.sha256(factory_path.read_bytes()).hexdigest() != manifest["files"]["factory.py"]["sha256"]:
        raise ValueError("factory.py changed")
    factory = runpy.run_path(str(factory_path))
    factory["verify_package"](package)
    factory["validate_action"](manifest, "run", True)
    return factory


def reconcile(records, lot, expected):
    """Count each factory MAC once; a later failed attempt replaces an earlier pass."""
    by_mac, issues, unidentified = {}, [], []
    for path, record in records:
        if record.get("lot") != lot:
            continue
        if record.get("action") != "run":
            issues.append(f"{path}: non-factory action in recipient lot")
            continue
        mac = str(record.get("mac", "")).upper()
        if not MAC.fullmatch(mac) or mac in ("00:00:00:00:00:00", "FF:FF:FF:FF:FF:FF"):
            if record.get("result") == "FAIL" and not mac:
                unidentified.append(str(path))
            else:
                issues.append(f"{path}: missing/invalid MAC")
            continue
        errors = []
        try:
            at = datetime.fromisoformat(record["at_utc"])
            if at.tzinfo is None:
                raise ValueError("timezone missing")
        except (KeyError, TypeError, ValueError) as error:
            issues.append(f"{path}: invalid timestamp ({error})")
            errors.append("invalid timestamp")
            at = datetime.max.replace(tzinfo=timezone.utc)
        if record.get("schema") != 2 or record.get("build_id") != BUILD_ID:
            errors.append("wrong schema/build")
        if record.get("manifest_sha256") != MANIFEST_SHA256:
            errors.append("wrong manifest")
        if record.get("result") != "PASS":
            errors.append(record.get("error") or "station did not pass")
        elif record.get("checks") != {"pcb": "PASS", "blank_wallet": "PASS",
                                      "no_phone_bonds": "PASS", "device_identity": "PASS"}:
            errors.append("incomplete station checks")
        by_mac.setdefault(mac, []).append((at, str(path), record, errors))

    rows = []
    for mac, attempts in sorted(by_mac.items()):
        attempts.sort(key=lambda attempt: (attempt[0], attempt[1]))
        at, path, record, errors = attempts[-1]
        errors = list(errors)
        if len(attempts) > 1 and attempts[-2][0] == at:
            errors.append("ambiguous latest attempts: matching timestamps")
        rows.append({"mac": mac, "at_utc": record.get("at_utc", ""),
                     "build_id": record.get("build_id", ""),
                     "result": "FAIL" if errors else "PASS", "error": "; ".join(errors)})
    passed = sum(row["result"] == "PASS" for row in rows)
    return {"lot": lot, "build_id": BUILD_ID, "expected": expected,
            "seen": len(rows), "passed": passed, "failed": len(rows) - passed,
            "missing": max(0, expected - passed), "issues": issues, "units": rows,
            "unidentified_failures": unidentified,
            "station_records_complete": passed == expected and not issues,
            "shipping_approved": False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--records", type=Path, required=True)
    parser.add_argument("--lot", required=True)
    parser.add_argument("--expected", type=int, default=200)
    parser.add_argument("--output", type=Path, required=True, help="new report directory, outside raw records")
    args = parser.parse_args()
    if args.expected < 1 or not args.records.is_dir():
        parser.error("positive expected count and existing records directory required")
    if args.output.resolve().is_relative_to(args.records.resolve()):
        parser.error("keep report output outside the raw records directory")
    load_factory(args.package.resolve())
    records = []
    for path in sorted(args.records.rglob("*.json")):
        record = json.loads(path.read_text())
        if not isinstance(record, dict):
            raise ValueError(f"{path}: malformed station record")
        records.append((path, record))
    report = reconcile(records, args.lot, args.expected)
    args.output.mkdir(parents=True, exist_ok=False)
    (args.output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
    with (args.output / "units.csv").open("w", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=("mac", "at_utc", "build_id", "result", "error"))
        writer.writeheader()
        writer.writerows(report["units"])
    print(f"{report['passed']}/{args.expected} PASS; {report['failed']} failed; {report['missing']} missing")
    print("Station records complete." if report["station_records_complete"] else "Station records incomplete or inconsistent.")
    print("Physical qualification and packing checks remain separate; this report never grants shipping approval.")
    if report["unidentified_failures"]:
        print(f"{len(report['unidentified_failures'])} failed attempts before MAC identification; no unit counted for them.")
    for issue in report["issues"]:
        print(issue)
    raise SystemExit(0 if report["station_records_complete"] else 1)


if __name__ == "__main__":
    main()
