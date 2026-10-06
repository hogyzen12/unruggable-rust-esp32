#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import json
import os
import re
import shlex
import subprocess
import sys
import time
import traceback
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path

from serial.tools import list_ports

DEFAULT_PORT_PREFIXES = ("/dev/cu.usbserial", "/dev/cu.usbmodem")
DEFAULT_ARTIFACT_CHECK = "xtensa-esp32-espidf/release/esp32-solana-signer"
DEFAULT_BUILD_COMMAND = "cargo build --release --target xtensa-esp32-espidf"
FIELD_PATTERNS = {
    "mac_address": re.compile(r"^MAC address:\s*(.+)$", re.MULTILINE),
    "chip_type": re.compile(r"^Chip type:\s*(.+)$", re.MULTILINE),
    "flash_size": re.compile(r"^Flash size:\s*(.+)$", re.MULTILINE),
}


@dataclass
class FlashResult:
    port: str
    status: str
    returncode: int
    duration_secs: float
    log_path: str
    mac_address: str = ""
    chip_type: str = ""
    flash_size: str = ""
    error: str = ""


def utc_timestamp(compact: bool = False) -> str:
    fmt = "%Y%m%dT%H%M%SZ" if compact else "%Y-%m-%dT%H:%M:%SZ"
    return datetime.now(timezone.utc).strftime(fmt)


def sanitize_label(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]+", "-", value).strip("-") or "batch"


def sanitize_port(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]+", "_", value).strip("_") or "port"


def resolve_path(base: Path, raw_value: str) -> Path:
    path = Path(raw_value)
    return path.resolve() if path.is_absolute() else (base / path).resolve()


def discover_ports(prefixes: tuple[str, ...]) -> list[str]:
    devices = [port.device for port in list_ports.comports()]
    return sorted(device for device in devices if any(device.startswith(prefix) for prefix in prefixes))


def load_ports_file(path: Path) -> list[str]:
    ports: list[str] = []
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        ports.append(line)
    return ports


def resolve_ports(args: argparse.Namespace) -> list[str]:
    ports: list[str] = []
    if args.port:
        ports.extend(args.port)
    if args.ports_file:
        ports.extend(load_ports_file(Path(args.ports_file)))
    if args.auto_ports:
        prefixes = tuple(args.port_prefix or DEFAULT_PORT_PREFIXES)
        ports.extend(discover_ports(prefixes))

    unique_ports = sorted(dict.fromkeys(port.strip() for port in ports if port.strip()))
    if not unique_ports:
        raise SystemExit(
            "No ports selected. Use --port, --ports-file, or --auto-ports."
        )
    return unique_ports


def build_release(root: Path, target_dir: Path, build_command: str) -> None:
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(target_dir)
    env["ESP_IDF_SDKCONFIG_DEFAULTS"] = "sdkconfig.defaults"
    cmd = shlex.split(build_command.strip() or DEFAULT_BUILD_COMMAND)
    print(f"[build] {' '.join(cmd)}", flush=True)
    subprocess.run(cmd, cwd=root, env=env, check=True)


def extract_field(text: str, field: str) -> str:
    pattern = FIELD_PATTERNS[field]
    match = pattern.search(text)
    if not match:
        return ""
    return match.group(1).strip()


def tail_lines(path: Path, count: int) -> str:
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return ""
    return "\n".join(lines[-count:])


def run_flash(
    project_root: Path,
    provision_script: Path,
    port: str,
    target_dir: Path,
    batch_tag: str,
    log_csv: Path,
    run_dir: Path,
    skip_efuse_check: bool,
) -> FlashResult:
    port_slug = sanitize_port(port)
    port_log = run_dir / f"{port_slug}.log"
    cmd = [
        str(provision_script),
        "--skip-build",
        "--port",
        port,
        "--batch-tag",
        batch_tag,
        "--log-csv",
        str(log_csv),
    ]
    if skip_efuse_check:
        cmd.append("--skip-efuse-check")

    env = os.environ.copy()
    env["PROVISION_TARGET_DIR"] = str(target_dir)

    started_at = time.perf_counter()
    with port_log.open("w", encoding="utf-8") as log_handle:
        log_handle.write(f"# started_utc={utc_timestamp()}\n")
        log_handle.write(f"# command={' '.join(cmd)}\n\n")
        log_handle.flush()
        result = subprocess.run(
            cmd,
            cwd=project_root,
            env=env,
            stdout=log_handle,
            stderr=subprocess.STDOUT,
            text=True,
            check=False,
        )

    duration_secs = time.perf_counter() - started_at
    text = port_log.read_text(encoding="utf-8", errors="replace")
    status = "PASS" if result.returncode == 0 else "FAIL"
    return FlashResult(
        port=port,
        status=status,
        returncode=result.returncode,
        duration_secs=duration_secs,
        log_path=str(port_log),
        mac_address=extract_field(text, "mac_address"),
        chip_type=extract_field(text, "chip_type"),
        flash_size=extract_field(text, "flash_size"),
    )


def write_summary(run_dir: Path, results: list[FlashResult]) -> tuple[Path, Path]:
    summary_csv = run_dir / "summary.csv"
    summary_json = run_dir / "summary.json"
    with summary_csv.open("w", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(
            fh,
            fieldnames=[
                "port",
                "status",
                "returncode",
                "duration_secs",
                "mac_address",
                "chip_type",
                "flash_size",
                "log_path",
                "error",
            ],
        )
        writer.writeheader()
        for result in results:
            writer.writerow(asdict(result))

    with summary_json.open("w", encoding="utf-8") as fh:
        json.dump([asdict(result) for result in results], fh, indent=2)

    return summary_csv, summary_json


def main() -> int:
    ap = argparse.ArgumentParser(
        description=(
            "Build once and flash multiple dev boards in parallel using one "
            "provision script worker per serial port."
        )
    )
    ap.add_argument(
        "--project-root",
        default=".",
        help="Firmware project root. Relative paths resolve from this script's directory.",
    )
    ap.add_argument(
        "--provision-script",
        default="provision_dev.sh",
        help="Provisioning script path. Relative paths resolve from --project-root.",
    )
    ap.add_argument(
        "--port",
        action="append",
        default=[],
        help="Serial port to flash. Repeat for multiple ports.",
    )
    ap.add_argument(
        "--ports-file",
        default="",
        help="Optional text file with one serial port per line.",
    )
    ap.add_argument(
        "--auto-ports",
        action="store_true",
        help="Auto-discover matching serial ports on this machine.",
    )
    ap.add_argument(
        "--port-prefix",
        action="append",
        default=[],
        help=(
            "Prefix used by --auto-ports. Repeat to add multiple prefixes. "
            "Defaults to /dev/cu.usbserial and /dev/cu.usbmodem."
        ),
    )
    ap.add_argument(
        "--parallel",
        type=int,
        default=0,
        help="Maximum concurrent flash workers. Default: one worker per selected port.",
    )
    ap.add_argument(
        "--batch-tag",
        required=True,
        help="Traceability tag written to logs/dev_flash_log.csv and the run directory name.",
    )
    ap.add_argument(
        "--target-dir",
        default="",
        help=(
            "Artifact directory to reuse or build into. Default: "
            "PROVISION_TARGET_DIR if set, otherwise <project-root>/target."
        ),
    )
    ap.add_argument(
        "--build-command",
        default=DEFAULT_BUILD_COMMAND,
        help=(
            "Build command to run in --project-root when not using --skip-build. "
            "Must be a plain command string without shell operators."
        ),
    )
    ap.add_argument(
        "--artifact-check",
        default=DEFAULT_ARTIFACT_CHECK,
        help=(
            "Optional artifact path relative to --target-dir used for preflight validation. "
            "Pass an empty string to disable this check."
        ),
    )
    ap.add_argument(
        "--log-csv",
        default="",
        help="Shared CSV trace log. Default: <project-root>/logs/dev_flash_log.csv",
    )
    ap.add_argument(
        "--skip-build",
        action="store_true",
        help="Reuse the existing ELF instead of building once up front.",
    )
    ap.add_argument(
        "--skip-efuse-check",
        action="store_true",
        help="Pass --skip-efuse-check through to each worker.",
    )
    ap.add_argument(
        "--dry-run",
        action="store_true",
        help="Resolve ports and paths, print the plan, and exit without flashing.",
    )
    ap.add_argument(
        "--failure-tail-lines",
        type=int,
        default=20,
        help="How many log lines to print for each failed port at the end.",
    )
    args = ap.parse_args()

    script_root = Path(__file__).resolve().parent
    project_root = resolve_path(script_root, args.project_root)
    provision_script = resolve_path(project_root, args.provision_script)

    target_dir_raw = args.target_dir or os.environ.get("PROVISION_TARGET_DIR") or "target"
    target_dir = resolve_path(project_root, target_dir_raw)

    log_csv_raw = args.log_csv or "logs/dev_flash_log.csv"
    log_csv = resolve_path(project_root, log_csv_raw)

    ports = resolve_ports(args)
    parallelism = args.parallel or len(ports)
    if parallelism < 1:
        ap.error("--parallel must be >= 1")

    run_dir = project_root / "logs" / "batches" / f"{utc_timestamp(compact=True)}-{sanitize_label(args.batch_tag)}"

    print(f"[ports] {', '.join(ports)}", flush=True)
    print(f"[parallel] {min(parallelism, len(ports))} worker(s)", flush=True)
    print(f"[project-root] {project_root}", flush=True)
    print(f"[provision-script] {provision_script}", flush=True)
    print(f"[target-dir] {target_dir}", flush=True)
    print(f"[build-command] {args.build_command}", flush=True)
    print(f"[log-csv] {log_csv}", flush=True)
    print(f"[run-dir] {run_dir}", flush=True)

    if args.dry_run:
        return 0

    if not provision_script.exists():
        print(f"Missing provision script: {provision_script}", file=sys.stderr)
        return 1

    run_dir.mkdir(parents=True, exist_ok=True)

    if not args.skip_build:
        build_release(project_root, target_dir, args.build_command)

    artifact_check = args.artifact_check.strip()
    if artifact_check:
        artifact_path = resolve_path(target_dir, artifact_check)
        if not artifact_path.exists():
            print(f"Missing build artifact: {artifact_path}", file=sys.stderr)
            print(
                "Build once first, disable --artifact-check, or point --target-dir at the validated artifact directory.",
                file=sys.stderr,
            )
            return 1

    results: list[FlashResult] = []
    started_utc = utc_timestamp()
    started_at = time.perf_counter()

    with ThreadPoolExecutor(max_workers=min(parallelism, len(ports))) as executor:
        future_map = {}
        for port in ports:
            print(f"[start] {port}", flush=True)
            future = executor.submit(
                run_flash,
                project_root,
                provision_script,
                port,
                target_dir,
                args.batch_tag,
                log_csv,
                run_dir,
                args.skip_efuse_check,
            )
            future_map[future] = port

        for future in as_completed(future_map):
            port = future_map[future]
            try:
                result = future.result()
            except Exception as exc:  # noqa: BLE001
                port_log = run_dir / f"{sanitize_port(port)}.log"
                error_text = "".join(traceback.format_exception(exc)).strip()
                port_log.write_text(error_text + "\n", encoding="utf-8")
                result = FlashResult(
                    port=port,
                    status="FAIL",
                    returncode=99,
                    duration_secs=0.0,
                    log_path=str(port_log),
                    error=error_text.splitlines()[-1] if error_text else repr(exc),
                )
            results.append(result)
            print(
                f"[done] {port} {result.status} {result.duration_secs:.1f}s "
                f"log={result.log_path}",
                flush=True,
            )

    finished_utc = utc_timestamp()
    results.sort(key=lambda item: item.port)
    summary_csv, summary_json = write_summary(run_dir, results)

    manifest_path = run_dir / "run_manifest.json"
    manifest = {
        "batch_tag": args.batch_tag,
        "started_utc": started_utc,
        "finished_utc": finished_utc,
        "ports": ports,
        "parallelism": min(parallelism, len(ports)),
        "project_root": str(project_root),
        "provision_script": str(provision_script),
        "target_dir": str(target_dir),
        "build_command": args.build_command,
        "artifact_check": artifact_check,
        "log_csv": str(log_csv),
        "summary_csv": str(summary_csv),
        "summary_json": str(summary_json),
        "elapsed_secs": time.perf_counter() - started_at,
    }
    manifest_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")

    passed = [result for result in results if result.status == "PASS"]
    failed = [result for result in results if result.status != "PASS"]

    print()
    print(f"Batch tag: {args.batch_tag}")
    print(f"Passed: {len(passed)}")
    print(f"Failed: {len(failed)}")
    print(f"Summary CSV: {summary_csv}")
    print(f"Summary JSON: {summary_json}")
    print(f"Run manifest: {manifest_path}")

    for result in failed:
        print()
        print(f"[failure] {result.port} rc={result.returncode} log={result.log_path}")
        tail = tail_lines(Path(result.log_path), args.failure_tail_lines)
        if tail:
            print(tail)

    return 0 if not failed else 1


if __name__ == "__main__":
    raise SystemExit(main())
