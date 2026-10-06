#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import queue
import re
import shlex
import signal
import subprocess
import sys
import threading
from dataclasses import asdict, dataclass, field
from datetime import datetime
from pathlib import Path
import tkinter as tk
from tkinter import filedialog, messagebox, simpledialog, ttk
from typing import Callable

from serial.tools import list_ports

from provision_dev_batch import DEFAULT_ARTIFACT_CHECK, DEFAULT_BUILD_COMMAND, DEFAULT_PORT_PREFIXES


APP_ROOT = Path(__file__).resolve().parent
DEFAULT_CONFIG_PATH = APP_ROOT / "flash_profiles.json"
PROFILE_ID_RE = re.compile(r"^[A-Za-z0-9._-]+$")

START_RE = re.compile(r"^\[start\] (.+)$")
DONE_RE = re.compile(r"^\[done\] (.+?) (PASS|FAIL) ([0-9.]+)s log=(.+)$")
RUN_DIR_RE = re.compile(r"^\[run-dir\] (.+)$")
SUMMARY_JSON_RE = re.compile(r"^Summary JSON:\s+(.+)$")
ROUND_TAG_RE = re.compile(r"^(.*-r)(\d+)$")


def default_batch_tag(prefix: str = "dev") -> str:
    clean_prefix = prefix.strip() or "dev"
    return f"{clean_prefix}-{datetime.now().strftime('%Y-%m-%d')}-r1"


def bump_batch_tag(tag: str) -> str:
    match = ROUND_TAG_RE.fullmatch(tag.strip())
    if not match:
        return f"{tag.strip()}-r2" if tag.strip() else default_batch_tag()
    prefix, round_num = match.groups()
    return f"{prefix}{int(round_num) + 1}"


def shlex_join(parts: list[str]) -> str:
    return " ".join(shlex.quote(part) for part in parts)


def open_path(path: Path) -> None:
    if sys.platform == "darwin":
        subprocess.Popen(["open", str(path)])
    elif os.name == "nt":
        os.startfile(path)  # type: ignore[attr-defined]
    else:
        subprocess.Popen(["xdg-open", str(path)])


def resolve_against(base: Path, raw_value: str) -> Path:
    path = Path(raw_value)
    return path.resolve() if path.is_absolute() else (base / path).resolve()


def serialize_relatively(base: Path, raw_value: str) -> str:
    try:
        resolved = resolve_against(base, raw_value)
        return os.path.relpath(resolved, base)
    except OSError:
        return raw_value


def parse_prefixes(raw_value: str) -> list[str]:
    prefixes = [item.strip() for item in raw_value.split(",") if item.strip()]
    return prefixes or list(DEFAULT_PORT_PREFIXES)


def parse_manual_ports(raw_value: str) -> list[str]:
    items = re.split(r"[\s,]+", raw_value.strip())
    return [item for item in items if item]


def natural_sort_key(value: str) -> list[object]:
    return [int(part) if part.isdigit() else part.lower() for part in re.split(r"(\d+)", value or "")]


def extract_location(port: object) -> str:
    location = str(getattr(port, "location", "") or "").strip()
    if location:
        return location
    hwid = str(getattr(port, "hwid", "") or "")
    match = re.search(r"LOCATION=([^\s]+)", hwid)
    return match.group(1).strip() if match else ""


def parent_location(location: str) -> str:
    location = location.strip()
    if not location:
        return ""
    if "." in location:
        return location.rsplit(".", 1)[0]
    if "-" in location:
        return location.rsplit("-", 1)[0]
    return location


def shared_location_prefix(locations: list[str]) -> str:
    cleaned = sorted({location.strip() for location in locations if location.strip()})
    if not cleaned:
        return ""
    if len(cleaned) == 1:
        return parent_location(cleaned[0])

    prefix = os.path.commonprefix(cleaned).rstrip(".-")
    while prefix:
        if all(
            location == prefix
            or (location.startswith(prefix) and location[len(prefix):len(prefix) + 1] in {".", "-"})
            for location in cleaned
        ):
            return prefix
        prefix = prefix[:-1].rstrip(".-")
    return ""


def suggested_hub_prefix(locations: list[str]) -> str:
    cleaned = sorted({location.strip() for location in locations if location.strip()})
    if not cleaned:
        return ""
    if len(cleaned) == 1:
        return parent_location(cleaned[0])

    ancestor_counts: dict[str, int] = {}
    for location in cleaned:
        seen: set[str] = set()
        cursor = parent_location(location)
        while cursor and cursor not in seen:
            ancestor_counts[cursor] = ancestor_counts.get(cursor, 0) + 1
            seen.add(cursor)
            next_cursor = parent_location(cursor)
            if next_cursor == cursor:
                break
            cursor = next_cursor

    repeated = [ancestor for ancestor, count in ancestor_counts.items() if count >= 2]
    if repeated:
        return max(repeated, key=lambda value: (value.count(".") + value.count("-"), len(value)))

    return shared_location_prefix(cleaned)


def location_matches_prefix(location: str, hub_location_prefix: str) -> bool:
    location = location.strip()
    hub_location_prefix = hub_location_prefix.strip().rstrip(".-")
    if not location or not hub_location_prefix:
        return False
    if location == hub_location_prefix:
        return True
    return location.startswith(hub_location_prefix) and location[len(hub_location_prefix):len(hub_location_prefix) + 1] in {
        ".",
        "-",
    }


def derive_hub_slot(location: str, hub_location_prefix: str) -> str:
    location = location.strip()
    hub_location_prefix = hub_location_prefix.strip().rstrip(".-")
    if not location or not hub_location_prefix:
        return ""
    if location == hub_location_prefix:
        return "root"
    if not location_matches_prefix(location, hub_location_prefix):
        return ""
    remainder = location[len(hub_location_prefix):].lstrip(".-")
    return remainder or "root"


@dataclass
class FlashProfile:
    profile_id: str
    name: str
    description: str = ""
    hub_location_prefix: str = ""
    project_root: str = "."
    provision_script: str = "provision_dev.sh"
    target_dir: str = "target"
    build_command: str = DEFAULT_BUILD_COMMAND
    artifact_check: str = DEFAULT_ARTIFACT_CHECK
    log_csv: str = "logs/dev_flash_log.csv"
    default_parallel: int = 4
    default_skip_build: bool = True
    default_skip_efuse_check: bool = True
    port_prefixes: list[str] = field(default_factory=lambda: list(DEFAULT_PORT_PREFIXES))
    batch_tag_prefix: str = "dev"

    @classmethod
    def from_dict(cls, raw: dict[str, object]) -> "FlashProfile":
        profile_id = str(raw.get("profile_id") or raw.get("id") or "").strip()
        name = str(raw.get("name") or profile_id or "Unnamed Profile").strip()
        return cls(
            profile_id=profile_id,
            name=name,
            description=str(raw.get("description") or "").strip(),
            hub_location_prefix=str(raw.get("hub_location_prefix") or "").strip(),
            project_root=str(raw.get("project_root") or ".").strip() or ".",
            provision_script=str(raw.get("provision_script") or "provision_dev.sh").strip() or "provision_dev.sh",
            target_dir=str(raw.get("target_dir") or "target").strip() or "target",
            build_command=str(raw.get("build_command") or DEFAULT_BUILD_COMMAND).strip() or DEFAULT_BUILD_COMMAND,
            artifact_check=str(raw.get("artifact_check") or DEFAULT_ARTIFACT_CHECK),
            log_csv=str(raw.get("log_csv") or "logs/dev_flash_log.csv").strip() or "logs/dev_flash_log.csv",
            default_parallel=max(1, int(raw.get("default_parallel") or 4)),
            default_skip_build=bool(raw.get("default_skip_build", True)),
            default_skip_efuse_check=bool(raw.get("default_skip_efuse_check", True)),
            port_prefixes=list(raw.get("port_prefixes") or list(DEFAULT_PORT_PREFIXES)),
            batch_tag_prefix=str(raw.get("batch_tag_prefix") or "dev").strip() or "dev",
        )

    def to_dict(self) -> dict[str, object]:
        payload = asdict(self)
        payload["id"] = self.profile_id
        return payload


@dataclass
class PortInfo:
    device: str
    description: str
    hwid: str
    location: str
    hub_slot: str


def default_profiles_config() -> dict[str, object]:
    profile = FlashProfile(
        profile_id="dev_boards",
        name="Current Dev Boards Hub Station",
        description="Current UART dev-board firmware on the dedicated USB-C flashing hub.",
        hub_location_prefix="",
        project_root=".",
        provision_script="provision_dev.sh",
        target_dir="target",
        build_command=DEFAULT_BUILD_COMMAND,
        artifact_check=DEFAULT_ARTIFACT_CHECK,
        log_csv="logs/dev_flash_log.csv",
        default_parallel=1,
        default_skip_build=True,
        default_skip_efuse_check=True,
        port_prefixes=list(DEFAULT_PORT_PREFIXES),
        batch_tag_prefix="dev",
    )
    return {
        "default_profile_id": profile.profile_id,
        "profiles": [profile.to_dict()],
    }


def ensure_profiles_config(config_path: Path) -> None:
    if config_path.exists():
        return
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text(json.dumps(default_profiles_config(), indent=2) + "\n", encoding="utf-8")


def load_profiles_config(config_path: Path) -> tuple[dict[str, FlashProfile], list[str], str]:
    ensure_profiles_config(config_path)
    raw = json.loads(config_path.read_text(encoding="utf-8"))
    profile_items = raw.get("profiles", [])
    profiles: dict[str, FlashProfile] = {}
    order: list[str] = []
    for item in profile_items:
        if not isinstance(item, dict):
            continue
        profile = FlashProfile.from_dict(item)
        if not profile.profile_id:
            continue
        profiles[profile.profile_id] = profile
        order.append(profile.profile_id)

    if not profiles:
        defaults = default_profiles_config()
        raw["profiles"] = defaults["profiles"]
        raw["default_profile_id"] = defaults["default_profile_id"]
        config_path.write_text(json.dumps(raw, indent=2) + "\n", encoding="utf-8")
        return load_profiles_config(config_path)

    default_profile_id = str(raw.get("default_profile_id") or order[0])
    if default_profile_id not in profiles:
        default_profile_id = order[0]
    return profiles, order, default_profile_id


def save_profiles_config(
    config_path: Path,
    profiles: dict[str, FlashProfile],
    order: list[str],
    default_profile_id: str,
) -> None:
    payload = {
        "default_profile_id": default_profile_id,
        "profiles": [profiles[profile_id].to_dict() for profile_id in order if profile_id in profiles],
    }
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


class PortRow:
    def __init__(
        self,
        parent: tk.Misc,
        port: PortInfo,
        selected: bool = True,
        on_toggle: Callable[[], None] | None = None,
    ) -> None:
        self.port = port
        self.selected = tk.BooleanVar(value=selected)
        self.status = tk.StringVar(value="IDLE")
        self.duration = tk.StringVar(value="-")
        self.mac = tk.StringVar(value="-")
        self.log_path = ""

        self.frame = ttk.Frame(parent, padding=(0, 2))
        self.check = ttk.Checkbutton(self.frame, variable=self.selected, command=on_toggle)
        self.check.grid(row=0, column=0, padx=(0, 8), sticky="w")

        self.slot_label = ttk.Label(self.frame, text=port.hub_slot or "-", width=10)
        self.slot_label.grid(row=0, column=1, padx=(0, 8), sticky="w")

        self.device_label = ttk.Label(self.frame, text=port.device, width=24)
        self.device_label.grid(row=0, column=2, padx=(0, 8), sticky="w")

        self.location_label = ttk.Label(self.frame, text=port.location or "-", width=14)
        self.location_label.grid(row=0, column=3, padx=(0, 8), sticky="w")

        self.desc_label = ttk.Label(self.frame, text=port.description or port.hwid or "-", width=30)
        self.desc_label.grid(row=0, column=4, padx=(0, 8), sticky="w")

        self.status_label = tk.Label(self.frame, textvariable=self.status, width=12, anchor="w", fg="#6b7280")
        self.status_label.grid(row=0, column=5, padx=(0, 8), sticky="w")

        self.duration_label = ttk.Label(self.frame, textvariable=self.duration, width=9)
        self.duration_label.grid(row=0, column=6, padx=(0, 8), sticky="w")

        self.mac_label = ttk.Label(self.frame, textvariable=self.mac, width=22)
        self.mac_label.grid(row=0, column=7, sticky="w")

    def set_enabled(self, enabled: bool) -> None:
        self.check.configure(state="normal" if enabled else "disabled")

    def set_idle(self) -> None:
        self.status.set("IDLE")
        self.duration.set("-")
        self.log_path = ""
        self.status_label.configure(fg="#6b7280")

    def set_pending(self) -> None:
        self.status.set("QUEUED")
        self.duration.set("-")
        self.log_path = ""
        self.status_label.configure(fg="#9a6700")

    def set_running(self) -> None:
        self.status.set("RUNNING")
        self.status_label.configure(fg="#1d4ed8")

    def set_result(self, status: str, duration_secs: float | None = None, log_path: str = "") -> None:
        self.status.set(status)
        self.log_path = log_path
        if duration_secs is not None:
            self.duration.set(f"{duration_secs:.1f}s")
        if status == "PASS":
            self.status_label.configure(fg="#15803d")
        elif status == "FAIL":
            self.status_label.configure(fg="#b91c1c")
        elif status == "STOPPED":
            self.status_label.configure(fg="#6b7280")
        else:
            self.status_label.configure(fg="#6b7280")

    def set_mac(self, mac_address: str) -> None:
        self.mac.set(mac_address or "-")


class DevFlashGui:
    def __init__(self, root: tk.Tk, args: argparse.Namespace) -> None:
        self.root = root
        self.config_path = resolve_against(APP_ROOT, args.profiles_file or str(DEFAULT_CONFIG_PATH))
        self.profiles, self.profile_order, self.default_profile_id = load_profiles_config(self.config_path)
        self.current_profile_id: str | None = None
        self.loading_profile = False

        self.profile_selector_var = tk.StringVar()
        self.profile_id_var = tk.StringVar()
        self.profile_name_var = tk.StringVar()
        self.profile_description_var = tk.StringVar()
        self.profile_summary_var = tk.StringVar(value="Choose a firmware target.")
        self.profile_runtime_var = tk.StringVar(value="")
        self.operator_hint_var = tk.StringVar(
            value="Choose a target, refresh/select ports, then click Flash Selected."
        )
        self.hub_location_prefix_var = tk.StringVar()
        self.project_root_var = tk.StringVar()
        self.provision_script_var = tk.StringVar()
        self.target_dir_var = tk.StringVar()
        self.build_command_var = tk.StringVar()
        self.artifact_check_var = tk.StringVar()
        self.log_csv_var = tk.StringVar()
        self.port_prefixes_var = tk.StringVar()
        self.manual_ports_var = tk.StringVar()
        self.batch_tag_prefix_var = tk.StringVar()
        self.batch_tag_var = tk.StringVar()
        self.parallel_var = tk.IntVar(value=max(1, args.parallel or 4))
        self.skip_build_var = tk.BooleanVar(value=args.skip_build)
        self.skip_efuse_var = tk.BooleanVar(value=args.skip_efuse_check)
        self.show_all_ports_var = tk.BooleanVar(value=args.show_all_ports)
        self.advanced_button_text = tk.StringVar(value="Show Advanced")
        self.status_var = tk.StringVar(value="Choose a target and refresh ports.")
        self.selection_summary_var = tk.StringVar(value="Detected 0 ports. Selected 0.")

        self.rows: dict[str, PortRow] = {}
        self.process: subprocess.Popen[str] | None = None
        self.process_thread: threading.Thread | None = None
        self.output_queue: queue.Queue[tuple[str, object]] = queue.Queue()
        self.last_run_dir: Path | None = None
        self.last_summary_json: Path | None = None
        self.is_running = False
        self.current_dry_run = False
        self.advanced_visible = False
        self.advanced_frame: ttk.LabelFrame | None = None

        self._build_ui()
        self._refresh_profile_selector()

        selected_profile = args.profile or self.default_profile_id
        if selected_profile not in self.profiles and self.profile_order:
            selected_profile = self.profile_order[0]
        if selected_profile in self.profiles:
            self._select_profile(selected_profile, reset_batch_tag=True)
        else:
            self._new_profile()

        if args.skip_build:
            self.skip_build_var.set(True)
        if args.skip_efuse_check:
            self.skip_efuse_var.set(True)
        if args.show_all_ports:
            self.show_all_ports_var.set(True)
            self.refresh_ports()
        if args.parallel:
            self.parallel_var.set(max(1, args.parallel))
        self._update_profile_summary()

        self._poll_queue()

    def _build_ui(self) -> None:
        self.root.title("ESP32 Board Flasher")
        self.root.geometry("1420x920")
        self.root.minsize(1220, 780)

        main = ttk.Frame(self.root, padding=14)
        main.pack(fill="both", expand=True)

        profile_frame = ttk.LabelFrame(main, text="1. Firmware Target", padding=12)
        profile_frame.pack(fill="x")

        ttk.Label(profile_frame, text="Target").grid(row=0, column=0, sticky="w")
        self.profile_combo = ttk.Combobox(
            profile_frame,
            textvariable=self.profile_selector_var,
            state="readonly",
            width=42,
        )
        self.profile_combo.grid(row=0, column=1, padx=(8, 8), sticky="ew")
        self.profile_combo.bind("<<ComboboxSelected>>", self._on_profile_selected)

        self.reload_profiles_button = ttk.Button(profile_frame, text="Reload Targets", command=self._reload_profiles)
        self.reload_profiles_button.grid(row=0, column=2, padx=(0, 8), sticky="w")
        self.advanced_toggle_button = ttk.Button(
            profile_frame,
            textvariable=self.advanced_button_text,
            command=self._toggle_advanced,
        )
        self.advanced_toggle_button.grid(row=0, column=3, sticky="w")

        ttk.Label(profile_frame, textvariable=self.profile_summary_var, wraplength=1100).grid(
            row=1, column=0, columnspan=4, pady=(10, 0), sticky="w"
        )
        ttk.Label(profile_frame, textvariable=self.profile_runtime_var, wraplength=1100).grid(
            row=2, column=0, columnspan=4, pady=(6, 0), sticky="w"
        )
        ttk.Label(profile_frame, textvariable=self.operator_hint_var, wraplength=1100).grid(
            row=3, column=0, columnspan=4, pady=(6, 0), sticky="w"
        )
        profile_frame.columnconfigure(1, weight=1)

        controls = ttk.LabelFrame(main, text="2. Batch", padding=12)
        controls.pack(fill="x", pady=(12, 0))

        ttk.Label(controls, text="Batch Tag").grid(row=0, column=0, sticky="w")
        self.batch_tag_entry = ttk.Entry(controls, textvariable=self.batch_tag_var, width=32)
        self.batch_tag_entry.grid(
            row=0, column=1, padx=(8, 8), sticky="w"
        )
        self.batch_tag_next_button = ttk.Button(controls, text="Next Round", command=self._bump_batch_tag)
        self.batch_tag_next_button.grid(
            row=0, column=2, padx=(0, 12), sticky="w"
        )
        ttk.Label(
            controls,
            text="Use one batch tag per tray or round. It increments after a successful flash run.",
        ).grid(row=1, column=0, columnspan=3, pady=(8, 0), sticky="w")
        controls.columnconfigure(1, weight=1)

        port_frame = ttk.LabelFrame(main, text="3. Ports", padding=12)
        port_frame.pack(fill="x", pady=(12, 0))

        summary_row = ttk.Frame(port_frame)
        summary_row.pack(fill="x")
        ttk.Label(summary_row, textvariable=self.selection_summary_var).pack(side="left")

        button_row = ttk.Frame(port_frame)
        button_row.pack(fill="x", pady=(8, 8))
        self.refresh_button = ttk.Button(button_row, text="Refresh Ports", command=self.refresh_ports)
        self.refresh_button.pack(side="left")
        self.select_all_button = ttk.Button(button_row, text="Select All", command=lambda: self._set_all_selected(True))
        self.select_all_button.pack(side="left", padx=(8, 0))
        self.clear_button = ttk.Button(button_row, text="Clear Selection", command=lambda: self._set_all_selected(False))
        self.clear_button.pack(side="left", padx=(8, 0))

        manual_row = ttk.Frame(port_frame)
        manual_row.pack(fill="x", pady=(0, 8))
        ttk.Label(manual_row, text="Missing Port").pack(side="left")
        self.manual_ports_entry = ttk.Entry(manual_row, textvariable=self.manual_ports_var, width=72)
        self.manual_ports_entry.pack(side="left", padx=(8, 8), fill="x", expand=True)
        self.apply_ports_button = ttk.Button(manual_row, text="Apply", command=self.refresh_ports)
        self.apply_ports_button.pack(side="left")
        ttk.Label(manual_row, text="Paste /dev/... values, separated by commas or spaces.").pack(
            side="left", padx=(8, 0)
        )

        header = ttk.Frame(port_frame)
        header.pack(fill="x", pady=(0, 4))
        headers = ["", "Hub Slot", "Port", "Location", "Description", "Status", "Duration", "MAC Address"]
        widths = [4, 10, 24, 14, 30, 12, 9, 22]
        for idx, (text, width) in enumerate(zip(headers, widths)):
            ttk.Label(header, text=text, width=width).grid(row=0, column=idx, sticky="w", padx=(0, 8))

        canvas_frame = ttk.Frame(port_frame)
        canvas_frame.pack(fill="both", expand=False)
        self.port_canvas = tk.Canvas(canvas_frame, height=230, highlightthickness=0)
        self.port_canvas.pack(side="left", fill="both", expand=True)
        port_scroll = ttk.Scrollbar(canvas_frame, orient="vertical", command=self.port_canvas.yview)
        port_scroll.pack(side="right", fill="y")
        self.port_canvas.configure(yscrollcommand=port_scroll.set)
        self.port_inner = ttk.Frame(self.port_canvas)
        self.port_canvas.create_window((0, 0), window=self.port_inner, anchor="nw")
        self.port_inner.bind(
            "<Configure>",
            lambda _event: self.port_canvas.configure(scrollregion=self.port_canvas.bbox("all")),
        )

        action_frame = ttk.LabelFrame(main, text="4. Flash", padding=12)
        action_frame.pack(fill="x", pady=(12, 0))
        action_row = ttk.Frame(action_frame)
        action_row.pack(fill="x")
        self.dry_run_button = ttk.Button(action_row, text="Dry Run", command=lambda: self._start_batch(dry_run=True))
        self.dry_run_button.pack(side="left")
        self.flash_button = ttk.Button(action_row, text="Flash Selected", command=lambda: self._start_batch(dry_run=False))
        self.flash_button.pack(side="left", padx=(8, 0))
        self.stop_button = ttk.Button(action_row, text="Stop", command=self._stop_batch, state="disabled")
        self.stop_button.pack(side="left", padx=(8, 0))
        self.open_dir_button = ttk.Button(action_row, text="Open Last Run Dir", command=self._open_last_run_dir)
        self.open_dir_button.pack(side="left", padx=(8, 0))
        ttk.Button(action_row, text="Clear Log", command=self._clear_log).pack(side="right")
        ttk.Label(action_frame, textvariable=self.status_var, wraplength=1100).pack(fill="x", pady=(10, 0), anchor="w")

        self.advanced_frame = ttk.LabelFrame(main, text="Advanced Target Setup", padding=12)

        ttk.Label(
            self.advanced_frame,
            text="Only use this section when setting up or editing firmware targets. Daily flashing should not require it.",
            wraplength=1100,
        ).pack(fill="x")

        manage_row = ttk.Frame(self.advanced_frame)
        manage_row.pack(fill="x", pady=(10, 0))
        self.new_profile_button = ttk.Button(manage_row, text="New Target", command=self._new_profile)
        self.new_profile_button.pack(side="left")
        self.save_profile_button = ttk.Button(manage_row, text="Save Target", command=self._save_profile)
        self.save_profile_button.pack(side="left", padx=(8, 0))
        self.open_config_button = ttk.Button(manage_row, text="Open Targets JSON", command=self._open_profiles_config)
        self.open_config_button.pack(side="left", padx=(8, 0))
        ttk.Button(manage_row, text="Open Project Root", command=self._open_project_root).pack(side="left", padx=(8, 0))
        ttk.Label(manage_row, text=f"Config: {self.config_path}").pack(side="right")

        config_frame = ttk.Frame(self.advanced_frame)
        config_frame.pack(fill="x", pady=(10, 0))

        ttk.Label(config_frame, text="Target ID").grid(row=0, column=0, sticky="w")
        ttk.Entry(config_frame, textvariable=self.profile_id_var, width=24).grid(
            row=0, column=1, padx=(8, 12), sticky="w"
        )
        ttk.Label(config_frame, text="Name").grid(row=0, column=2, sticky="e")
        ttk.Entry(config_frame, textvariable=self.profile_name_var, width=34).grid(
            row=0, column=3, padx=(8, 12), sticky="ew"
        )
        ttk.Label(config_frame, text="Batch Tag Prefix").grid(row=0, column=4, sticky="e")
        ttk.Entry(config_frame, textvariable=self.batch_tag_prefix_var, width=18).grid(
            row=0, column=5, padx=(8, 0), sticky="w"
        )

        ttk.Label(config_frame, text="Description").grid(row=1, column=0, sticky="w", pady=(10, 0))
        ttk.Entry(config_frame, textvariable=self.profile_description_var, width=96).grid(
            row=1, column=1, columnspan=5, padx=(8, 0), pady=(10, 0), sticky="ew"
        )

        ttk.Label(config_frame, text="Hub Location Prefix").grid(row=2, column=0, sticky="w", pady=(10, 0))
        ttk.Entry(config_frame, textvariable=self.hub_location_prefix_var, width=96).grid(
            row=2, column=1, columnspan=4, padx=(8, 8), pady=(10, 0), sticky="ew"
        )
        self.capture_hub_button = ttk.Button(
            config_frame,
            text="Capture From Visible Ports",
            command=self._capture_hub_prefix,
        )
        self.capture_hub_button.grid(row=2, column=5, pady=(10, 0), sticky="ew")

        ttk.Label(
            config_frame,
            text="When set, the operator view only shows serial devices whose USB LOCATION lives under this hub path.",
        ).grid(row=3, column=1, columnspan=5, pady=(6, 0), sticky="w")

        ttk.Label(config_frame, text="Project Root").grid(row=4, column=0, sticky="w", pady=(10, 0))
        ttk.Entry(config_frame, textvariable=self.project_root_var, width=96).grid(
            row=4, column=1, columnspan=4, padx=(8, 8), pady=(10, 0), sticky="ew"
        )
        ttk.Button(config_frame, text="Browse", command=self._browse_project_root).grid(
            row=4, column=5, pady=(10, 0), sticky="ew"
        )

        ttk.Label(config_frame, text="Provision Script").grid(row=5, column=0, sticky="w", pady=(10, 0))
        ttk.Entry(config_frame, textvariable=self.provision_script_var, width=96).grid(
            row=5, column=1, columnspan=4, padx=(8, 8), pady=(10, 0), sticky="ew"
        )
        ttk.Button(config_frame, text="Browse", command=self._browse_provision_script).grid(
            row=5, column=5, pady=(10, 0), sticky="ew"
        )

        ttk.Label(config_frame, text="Target Dir").grid(row=6, column=0, sticky="w", pady=(10, 0))
        ttk.Entry(config_frame, textvariable=self.target_dir_var, width=96).grid(
            row=6, column=1, columnspan=4, padx=(8, 8), pady=(10, 0), sticky="ew"
        )
        ttk.Button(config_frame, text="Browse", command=self._browse_target_dir).grid(
            row=6, column=5, pady=(10, 0), sticky="ew"
        )

        ttk.Label(config_frame, text="Build Command").grid(row=7, column=0, sticky="w", pady=(10, 0))
        ttk.Entry(config_frame, textvariable=self.build_command_var, width=96).grid(
            row=7, column=1, columnspan=5, padx=(8, 0), pady=(10, 0), sticky="ew"
        )

        ttk.Label(config_frame, text="Artifact Check").grid(row=8, column=0, sticky="w", pady=(10, 0))
        ttk.Entry(config_frame, textvariable=self.artifact_check_var, width=96).grid(
            row=8, column=1, columnspan=5, padx=(8, 0), pady=(10, 0), sticky="ew"
        )

        ttk.Label(config_frame, text="Trace CSV").grid(row=9, column=0, sticky="w", pady=(10, 0))
        ttk.Entry(config_frame, textvariable=self.log_csv_var, width=96).grid(
            row=9, column=1, columnspan=4, padx=(8, 8), pady=(10, 0), sticky="ew"
        )
        ttk.Button(config_frame, text="Browse", command=self._browse_log_csv).grid(
            row=9, column=5, pady=(10, 0), sticky="ew"
        )

        ttk.Label(config_frame, text="Port Prefixes").grid(row=10, column=0, sticky="w", pady=(10, 0))
        ttk.Entry(config_frame, textvariable=self.port_prefixes_var, width=96).grid(
            row=10, column=1, columnspan=5, padx=(8, 0), pady=(10, 0), sticky="ew"
        )

        ttk.Label(
            config_frame,
            text="Port prefixes are comma-separated and only used when Show All Serial Ports is off.",
        ).grid(row=11, column=1, columnspan=5, pady=(6, 0), sticky="w")

        ttk.Label(config_frame, text="Parallel").grid(row=12, column=0, sticky="w", pady=(12, 0))
        self.parallel_spinbox = ttk.Spinbox(config_frame, from_=1, to=32, textvariable=self.parallel_var, width=6)
        self.parallel_spinbox.grid(row=12, column=1, padx=(8, 8), pady=(12, 0), sticky="w")
        self.skip_build_check = ttk.Checkbutton(config_frame, text="Skip Build", variable=self.skip_build_var)
        self.skip_build_check.grid(row=12, column=2, padx=(0, 8), pady=(12, 0), sticky="w")
        self.skip_efuse_check = ttk.Checkbutton(
            config_frame, text="Skip eFuse Check", variable=self.skip_efuse_var
        )
        self.skip_efuse_check.grid(row=12, column=3, padx=(0, 8), pady=(12, 0), sticky="w")
        self.show_all_check = ttk.Checkbutton(
            config_frame,
            text="Show All Serial Ports",
            variable=self.show_all_ports_var,
            command=self.refresh_ports,
        )
        self.show_all_check.grid(row=12, column=4, columnspan=2, pady=(12, 0), sticky="w")

        for column in range(6):
            config_frame.columnconfigure(column, weight=1 if column in (1, 3, 4) else 0)

        self.log_frame = ttk.LabelFrame(main, text="Output", padding=12)
        self.log_frame.pack(fill="both", expand=True, pady=(12, 0))
        self.log_text = tk.Text(self.log_frame, wrap="word", height=22, font=("Menlo", 11))
        self.log_text.pack(side="left", fill="both", expand=True)
        log_scroll = ttk.Scrollbar(self.log_frame, orient="vertical", command=self.log_text.yview)
        log_scroll.pack(side="right", fill="y")
        self.log_text.configure(yscrollcommand=log_scroll.set)

    def _toggle_advanced(self) -> None:
        if self.advanced_visible:
            self._hide_advanced()
        else:
            self._show_advanced()

    def _show_advanced(self) -> None:
        if self.advanced_frame is None or self.advanced_visible:
            return
        self.advanced_frame.pack(fill="x", pady=(12, 0), before=self.log_frame)
        self.advanced_visible = True
        self.advanced_button_text.set("Hide Advanced")

    def _hide_advanced(self) -> None:
        if self.advanced_frame is None or not self.advanced_visible:
            return
        self.advanced_frame.pack_forget()
        self.advanced_visible = False
        self.advanced_button_text.set("Show Advanced")

    def _update_profile_summary(self) -> None:
        profile = self._selected_profile()
        if profile is None:
            self.profile_summary_var.set("Unsaved target profile.")
            self.profile_runtime_var.set(
                "Open Advanced to set the hub location prefix, project root, provision script, build directory, and defaults."
            )
            self.operator_hint_var.set(
                "Save the target once, then operators only need the target dropdown, ports list, and Flash button."
            )
            return

        description = profile.description.strip() or f"Firmware target '{profile.name}'."
        self.profile_summary_var.set(description)
        artifact_path = str(Path(profile.target_dir) / profile.artifact_check)
        hub_prefix = self.hub_location_prefix_var.get().strip()
        hub_text = f"   Hub prefix: {hub_prefix}" if hub_prefix else ""
        self.profile_runtime_var.set(
            "Project root: "
            f"{profile.project_root}   Script: {profile.provision_script}   Artifact: {artifact_path}{hub_text}"
        )
        defaults = (
            f"Current run: parallel={max(1, int(self.parallel_var.get()))}, "
            f"skip build={'yes' if self.skip_build_var.get() else 'no'}, "
            f"skip eFuse={'yes' if self.skip_efuse_var.get() else 'no'}."
        )
        if hub_prefix and not self.show_all_ports_var.get():
            defaults += " Operator list is filtered to this hub. If the hub moves to another Mac USB-C port, recapture the hub prefix in Advanced."
        self.operator_hint_var.set(
            f"{defaults} Pick ports below, then run Dry Run or Flash Selected."
        )

    def _capture_hub_prefix(self) -> None:
        selected_ports = set(self._selected_ports())
        candidate_locations: list[str] = []
        prefixes = parse_prefixes(self.port_prefixes_var.get())

        for port in list_ports.comports():
            device = str(getattr(port, "device", "") or "")
            if selected_ports and device not in selected_ports:
                continue
            if not selected_ports and not self.show_all_ports_var.get():
                if not any(device.startswith(prefix) for prefix in prefixes):
                    continue
            location = extract_location(port)
            if location:
                candidate_locations.append(location)

        hub_prefix = suggested_hub_prefix(candidate_locations)
        if not hub_prefix:
            messagebox.showerror(
                "Hub Prefix Not Found",
                "Select only the boards on the hub, then retry capture. The tool could not find a shared USB LOCATION ancestor.",
            )
            return

        self.hub_location_prefix_var.set(hub_prefix)
        self.show_all_ports_var.set(False)
        self._update_profile_summary()
        self.refresh_ports()
        self.status_var.set(
            f"Captured hub prefix '{hub_prefix}' from {len(candidate_locations)} visible port(s)."
        )

    def _port_sort_key(self, port: PortInfo) -> tuple[object, ...]:
        if port.hub_slot:
            return (0, natural_sort_key(port.hub_slot), natural_sort_key(port.location), natural_sort_key(port.device))
        if port.location:
            return (1, natural_sort_key(port.location), natural_sort_key(port.device))
        return (2, natural_sort_key(port.device))

    def _profile_choice_label(self, profile_id: str) -> str:
        profile = self.profiles[profile_id]
        return f"{profile.name} ({profile_id})"

    def _profile_display_values(self) -> list[str]:
        return [self._profile_choice_label(profile_id) for profile_id in self.profile_order]

    def _profile_id_from_choice(self, raw_value: str) -> str:
        raw_value = raw_value.strip()
        for profile_id in self.profile_order:
            if self._profile_choice_label(profile_id) == raw_value:
                return profile_id
        if raw_value in self.profiles:
            return raw_value
        return raw_value.split("|", 1)[0].strip()

    def _refresh_profile_selector(self) -> None:
        self.profile_combo["values"] = self._profile_display_values()
        if self.current_profile_id and self.current_profile_id in self.profiles:
            self.profile_selector_var.set(self._profile_choice_label(self.current_profile_id))
        elif self.profile_order:
            profile_id = self.profile_order[0]
            self.profile_selector_var.set(self._profile_choice_label(profile_id))
        else:
            self.profile_selector_var.set("")

    def _selected_profile(self) -> FlashProfile | None:
        if self.current_profile_id and self.current_profile_id in self.profiles:
            return self.profiles[self.current_profile_id]
        return None

    def _select_profile(self, profile_id: str, reset_batch_tag: bool) -> None:
        if profile_id not in self.profiles:
            return
        self.current_profile_id = profile_id
        profile = self.profiles[profile_id]
        self.loading_profile = True
        self.profile_selector_var.set(self._profile_choice_label(profile.profile_id))
        self.profile_id_var.set(profile.profile_id)
        self.profile_name_var.set(profile.name)
        self.profile_description_var.set(profile.description)
        self.hub_location_prefix_var.set(profile.hub_location_prefix)
        self.project_root_var.set(profile.project_root)
        self.provision_script_var.set(profile.provision_script)
        self.target_dir_var.set(profile.target_dir)
        self.build_command_var.set(profile.build_command)
        self.artifact_check_var.set(profile.artifact_check)
        self.log_csv_var.set(profile.log_csv)
        self.port_prefixes_var.set(", ".join(profile.port_prefixes))
        self.batch_tag_prefix_var.set(profile.batch_tag_prefix)
        self.parallel_var.set(profile.default_parallel)
        self.skip_build_var.set(profile.default_skip_build)
        self.skip_efuse_var.set(profile.default_skip_efuse_check)
        if reset_batch_tag or not self.batch_tag_var.get().strip():
            self.batch_tag_var.set(default_batch_tag(profile.batch_tag_prefix))
        self.loading_profile = False
        self._update_profile_summary()
        self.refresh_ports()

    def _on_profile_selected(self, _event: object = None) -> None:
        profile_id = self._profile_id_from_choice(self.profile_selector_var.get())
        if profile_id:
            self._select_profile(profile_id, reset_batch_tag=True)

    def _open_profiles_config(self) -> None:
        open_path(self.config_path)

    def _reload_profiles(self) -> None:
        selected = self.current_profile_id or self.default_profile_id
        self.profiles, self.profile_order, self.default_profile_id = load_profiles_config(self.config_path)
        self._refresh_profile_selector()
        if selected in self.profiles:
            self._select_profile(selected, reset_batch_tag=False)
        elif self.profile_order:
            self._select_profile(self.profile_order[0], reset_batch_tag=True)

    def _new_profile(self) -> None:
        source = self._selected_profile()
        base = source if source is not None else FlashProfile(profile_id="", name="")
        self.current_profile_id = None
        self.loading_profile = True
        self.profile_selector_var.set("<unsaved>")
        self.profile_id_var.set("")
        self.profile_name_var.set("")
        self.profile_description_var.set(base.description)
        self.hub_location_prefix_var.set(base.hub_location_prefix)
        self.project_root_var.set(base.project_root or ".")
        self.provision_script_var.set(base.provision_script or "provision_dev.sh")
        self.target_dir_var.set(base.target_dir or "target")
        self.build_command_var.set(base.build_command or DEFAULT_BUILD_COMMAND)
        self.artifact_check_var.set(base.artifact_check or DEFAULT_ARTIFACT_CHECK)
        self.log_csv_var.set(base.log_csv or "logs/dev_flash_log.csv")
        self.port_prefixes_var.set(", ".join(base.port_prefixes or list(DEFAULT_PORT_PREFIXES)))
        self.batch_tag_prefix_var.set(base.batch_tag_prefix or "dev")
        self.parallel_var.set(base.default_parallel or 4)
        self.skip_build_var.set(base.default_skip_build)
        self.skip_efuse_var.set(base.default_skip_efuse_check)
        self.batch_tag_var.set(default_batch_tag(self.batch_tag_prefix_var.get()))
        self.loading_profile = False
        self._update_profile_summary()
        self._show_advanced()
        self.refresh_ports()
        self.status_var.set("New unsaved profile.")

    def _profile_from_form(self) -> FlashProfile:
        return FlashProfile(
            profile_id=self.profile_id_var.get().strip(),
            name=self.profile_name_var.get().strip(),
            description=self.profile_description_var.get().strip(),
            hub_location_prefix=self.hub_location_prefix_var.get().strip(),
            project_root=self.project_root_var.get().strip() or ".",
            provision_script=self.provision_script_var.get().strip() or "provision_dev.sh",
            target_dir=self.target_dir_var.get().strip() or "target",
            build_command=self.build_command_var.get().strip() or DEFAULT_BUILD_COMMAND,
            artifact_check=self.artifact_check_var.get(),
            log_csv=self.log_csv_var.get().strip() or "logs/dev_flash_log.csv",
            default_parallel=max(1, int(self.parallel_var.get())),
            default_skip_build=bool(self.skip_build_var.get()),
            default_skip_efuse_check=bool(self.skip_efuse_var.get()),
            port_prefixes=parse_prefixes(self.port_prefixes_var.get()),
            batch_tag_prefix=self.batch_tag_prefix_var.get().strip() or "dev",
        )

    def _save_profile(self) -> None:
        try:
            profile = self._profile_from_form()
        except (tk.TclError, ValueError) as exc:
            messagebox.showerror("Invalid Profile", str(exc))
            return

        if not profile.profile_id:
            suggestion = simpledialog.askstring(
                "Profile ID",
                "Enter a profile id like dev_boards or se050_proto:",
                parent=self.root,
            )
            if suggestion is None:
                return
            self.profile_id_var.set(suggestion.strip())
            profile.profile_id = suggestion.strip()

        if not PROFILE_ID_RE.fullmatch(profile.profile_id):
            messagebox.showerror(
                "Invalid Profile ID",
                "Profile id may only use letters, numbers, dots, underscores, and dashes.",
            )
            return

        if not profile.name:
            messagebox.showerror("Missing Name", "Enter a human-readable profile name.")
            return

        old_profile_id = self.current_profile_id
        if old_profile_id and old_profile_id != profile.profile_id and profile.profile_id in self.profiles:
            messagebox.showerror("Duplicate Profile ID", f"Profile id '{profile.profile_id}' already exists.")
            return

        if old_profile_id and old_profile_id != profile.profile_id and old_profile_id in self.profiles:
            del self.profiles[old_profile_id]
            self.profile_order = [profile.profile_id if item == old_profile_id else item for item in self.profile_order]
        elif profile.profile_id not in self.profiles:
            self.profile_order.append(profile.profile_id)

        self.profiles[profile.profile_id] = profile
        self.current_profile_id = profile.profile_id
        self.default_profile_id = profile.profile_id
        save_profiles_config(self.config_path, self.profiles, self.profile_order, self.default_profile_id)
        self._refresh_profile_selector()
        self._select_profile(profile.profile_id, reset_batch_tag=False)
        self.status_var.set(f"Saved profile '{profile.profile_id}'.")

    def _browse_project_root(self) -> None:
        current = self.project_root_var.get().strip() or str(APP_ROOT)
        selected = filedialog.askdirectory(initialdir=str(resolve_against(APP_ROOT, current)))
        if selected:
            self.project_root_var.set(selected)

    def _browse_provision_script(self) -> None:
        base = resolve_against(APP_ROOT, self.project_root_var.get().strip() or ".")
        current = self.provision_script_var.get().strip() or "provision_dev.sh"
        initial_dir = str(resolve_against(base, current).parent if current else base)
        selected = filedialog.askopenfilename(initialdir=initial_dir, filetypes=[("Scripts", "*.sh *.py"), ("All files", "*.*")])
        if selected:
            self.provision_script_var.set(selected)

    def _browse_target_dir(self) -> None:
        base = resolve_against(APP_ROOT, self.project_root_var.get().strip() or ".")
        current = self.target_dir_var.get().strip() or "target"
        selected = filedialog.askdirectory(initialdir=str(resolve_against(base, current)))
        if selected:
            self.target_dir_var.set(selected)

    def _browse_log_csv(self) -> None:
        base = resolve_against(APP_ROOT, self.project_root_var.get().strip() or ".")
        current = self.log_csv_var.get().strip() or "logs/dev_flash_log.csv"
        selected = filedialog.asksaveasfilename(
            initialdir=str(resolve_against(base, current).parent),
            initialfile=Path(current).name,
            defaultextension=".csv",
            filetypes=[("CSV files", "*.csv"), ("All files", "*.*")],
        )
        if selected:
            self.log_csv_var.set(selected)

    def _bump_batch_tag(self) -> None:
        self.batch_tag_var.set(bump_batch_tag(self.batch_tag_var.get()))

    def _append_log(self, text: str) -> None:
        self.log_text.insert("end", text)
        self.log_text.see("end")

    def _clear_log(self) -> None:
        self.log_text.delete("1.0", "end")

    def _set_controls_enabled(self, enabled: bool) -> None:
        state = "normal" if enabled else "disabled"
        for widget in [
            self.advanced_toggle_button,
            self.new_profile_button,
            self.save_profile_button,
            self.reload_profiles_button,
            self.open_config_button,
            self.capture_hub_button,
            self.refresh_button,
            self.select_all_button,
            self.clear_button,
            self.apply_ports_button,
            self.batch_tag_next_button,
            self.dry_run_button,
            self.flash_button,
            self.skip_build_check,
            self.skip_efuse_check,
            self.show_all_check,
        ]:
            widget.configure(state=state)
        for widget in [
            self.batch_tag_entry,
            self.manual_ports_entry,
            self.parallel_spinbox,
        ]:
            widget.configure(state="normal" if enabled else "disabled")
        self.profile_combo.configure(state="readonly" if enabled else "disabled")
        self.stop_button.configure(state="disabled" if enabled else "normal")
        for row in self.rows.values():
            row.set_enabled(enabled)

    def _selected_ports(self) -> list[str]:
        return [device for device, row in self.rows.items() if row.selected.get()]

    def _set_all_selected(self, selected: bool) -> None:
        for row in self.rows.values():
            row.selected.set(selected)
        self._update_selection_summary()

    def _update_selection_summary(self) -> None:
        detected = len(self.rows)
        selected = len(self._selected_ports())
        hub_prefix = self.hub_location_prefix_var.get().strip()
        if hub_prefix and not self.show_all_ports_var.get():
            self.selection_summary_var.set(
                f"Detected {detected} hub ports under {hub_prefix}. Selected {selected}."
            )
        else:
            self.selection_summary_var.set(f"Detected {detected} ports. Selected {selected}.")

    def _list_ports(self) -> list[PortInfo]:
        prefixes = parse_prefixes(self.port_prefixes_var.get())
        show_all = self.show_all_ports_var.get()
        hub_prefix = self.hub_location_prefix_var.get().strip().rstrip(".-")
        ports_by_device: dict[str, PortInfo] = {}
        for port in list_ports.comports():
            device = str(getattr(port, "device", "") or "")
            if not show_all and not any(device.startswith(prefix) for prefix in prefixes):
                continue
            location = extract_location(port)
            if not show_all and hub_prefix and not location_matches_prefix(location, hub_prefix):
                continue
            description = str(getattr(port, "description", "") or getattr(port, "product", "") or "").strip()
            hwid = str(getattr(port, "hwid", "") or "")
            ports_by_device[device] = PortInfo(
                device=device,
                description=description,
                hwid=hwid,
                location=location,
                hub_slot=derive_hub_slot(location, hub_prefix),
            )
        for device in parse_manual_ports(self.manual_ports_var.get()):
            ports_by_device.setdefault(
                device,
                PortInfo(device=device, description="Manual", hwid="", location="", hub_slot=""),
            )
        return sorted(ports_by_device.values(), key=self._port_sort_key)

    def refresh_ports(self) -> None:
        if self.is_running:
            return
        self._update_profile_summary()
        previous_selection = {device: row.selected.get() for device, row in self.rows.items()}
        for child in self.port_inner.winfo_children():
            child.destroy()
        self.rows.clear()

        ports = self._list_ports()
        for idx, port in enumerate(ports):
            row = PortRow(
                self.port_inner,
                port,
                selected=previous_selection.get(port.device, True),
                on_toggle=self._update_selection_summary,
            )
            row.frame.grid(row=idx, column=0, sticky="ew")
            self.rows[port.device] = row
        self.port_inner.columnconfigure(0, weight=1)
        self._update_selection_summary()

    def _build_command(self, ports: list[str], dry_run: bool) -> list[str]:
        cmd = [
            sys.executable,
            "-u",
            str(APP_ROOT / "provision_dev_batch.py"),
            "--project-root",
            self.project_root_var.get().strip() or ".",
            "--provision-script",
            self.provision_script_var.get().strip() or "provision_dev.sh",
            "--batch-tag",
            self.batch_tag_var.get().strip(),
            "--parallel",
            str(max(1, int(self.parallel_var.get()))),
            "--target-dir",
            self.target_dir_var.get().strip() or "target",
            "--build-command",
            self.build_command_var.get().strip() or DEFAULT_BUILD_COMMAND,
            "--artifact-check",
            self.artifact_check_var.get(),
            "--log-csv",
            self.log_csv_var.get().strip() or "logs/dev_flash_log.csv",
        ]
        if self.skip_build_var.get():
            cmd.append("--skip-build")
        if self.skip_efuse_var.get():
            cmd.append("--skip-efuse-check")
        if dry_run:
            cmd.append("--dry-run")
        for port in ports:
            cmd.extend(["--port", port])
        return cmd

    def _start_batch(self, dry_run: bool) -> None:
        if self.is_running:
            return

        ports = self._selected_ports()
        if not ports:
            messagebox.showerror("No Ports Selected", "Select at least one serial port.")
            return

        batch_tag = self.batch_tag_var.get().strip()
        if not batch_tag:
            messagebox.showerror("Missing Batch Tag", "Enter a batch tag before flashing.")
            return

        project_root = self.project_root_var.get().strip()
        if not project_root:
            messagebox.showerror("Missing Project Root", "Set the firmware project root.")
            return

        provision_script = self.provision_script_var.get().strip()
        if not provision_script:
            messagebox.showerror("Missing Provision Script", "Set the provision script path.")
            return

        cmd = self._build_command(ports, dry_run=dry_run)
        self.last_run_dir = None
        self.last_summary_json = None
        self._clear_log()
        self._append_log(f"$ {shlex_join(cmd)}\n\n")
        self.status_var.set("Starting batch...")

        for device, row in self.rows.items():
            if device in ports:
                row.set_pending()
                row.set_mac("-")
            else:
                row.set_idle()

        try:
            self.process = subprocess.Popen(
                cmd,
                cwd=APP_ROOT,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                bufsize=1,
                start_new_session=True,
            )
        except OSError as exc:
            messagebox.showerror("Failed to Start", str(exc))
            self.status_var.set("Failed to start batch.")
            return

        self.is_running = True
        self.current_dry_run = dry_run
        self._set_controls_enabled(False)
        self.process_thread = threading.Thread(target=self._read_process_output, daemon=True)
        self.process_thread.start()

    def _read_process_output(self) -> None:
        assert self.process is not None
        if self.process.stdout is not None:
            for line in self.process.stdout:
                self.output_queue.put(("line", line))
        returncode = self.process.wait()
        self.output_queue.put(("exit", returncode))

    def _handle_output_line(self, line: str) -> None:
        self._append_log(line)
        stripped = line.strip()

        match = RUN_DIR_RE.match(stripped)
        if match:
            self.last_run_dir = Path(match.group(1))
            self.status_var.set(f"Run dir: {self.last_run_dir}")
            return

        match = SUMMARY_JSON_RE.match(stripped)
        if match:
            self.last_summary_json = Path(match.group(1))
            return

        match = START_RE.match(stripped)
        if match:
            port = match.group(1)
            if port in self.rows:
                self.rows[port].set_running()
                self.status_var.set(f"Flashing {port}...")
            return

        match = DONE_RE.match(stripped)
        if match:
            port, status, duration, log_path = match.groups()
            if port in self.rows:
                self.rows[port].set_result(status, float(duration), log_path=log_path)
                self.status_var.set(f"{port} {status}")

    def _load_summary(self) -> None:
        summary_path = self.last_summary_json
        if summary_path is None and self.last_run_dir is not None:
            summary_path = self.last_run_dir / "summary.json"
        if summary_path is None or not summary_path.exists():
            return

        try:
            data = json.loads(summary_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return

        for item in data:
            port = item.get("port")
            if port not in self.rows:
                continue
            row = self.rows[port]
            row.set_result(
                item.get("status", "UNKNOWN"),
                duration_secs=float(item.get("duration_secs") or 0.0),
                log_path=item.get("log_path", ""),
            )
            row.set_mac(item.get("mac_address", "") or "-")

    def _finish_batch(self, returncode: int) -> None:
        self.is_running = False
        self._set_controls_enabled(True)
        self._load_summary()

        for row in self.rows.values():
            if row.status.get() in {"QUEUED", "RUNNING"}:
                if self.current_dry_run:
                    row.set_idle()
                else:
                    row.set_result("STOPPED")

        if returncode == 0:
            if self.current_dry_run:
                self.status_var.set("Dry run completed successfully.")
            else:
                self.status_var.set("Batch completed successfully.")
                self.batch_tag_var.set(bump_batch_tag(self.batch_tag_var.get()))
        else:
            self.status_var.set(f"Batch finished with errors (rc={returncode}).")

        self.process = None
        self.process_thread = None
        self.current_dry_run = False
        self._update_selection_summary()

    def _poll_queue(self) -> None:
        try:
            while True:
                kind, payload = self.output_queue.get_nowait()
                if kind == "line":
                    self._handle_output_line(str(payload))
                elif kind == "exit":
                    self._finish_batch(int(payload))
        except queue.Empty:
            pass
        finally:
            self.root.after(120, self._poll_queue)

    def _stop_batch(self) -> None:
        if not self.is_running or self.process is None:
            return
        self.status_var.set("Stopping batch...")
        try:
            if os.name == "posix":
                os.killpg(os.getpgid(self.process.pid), signal.SIGTERM)
            else:
                self.process.terminate()
        except ProcessLookupError:
            pass

    def _open_last_run_dir(self) -> None:
        if self.last_run_dir is None:
            messagebox.showinfo("No Run Directory", "Run a batch first.")
            return
        open_path(self.last_run_dir)

    def _open_project_root(self) -> None:
        project_root = self.project_root_var.get().strip()
        if not project_root:
            messagebox.showinfo("No Project Root", "Set a project root first.")
            return
        open_path(resolve_against(APP_ROOT, project_root))


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Local GUI for multi-port ESP32 board flashing.")
    parser.add_argument(
        "--profiles-file",
        default=str(DEFAULT_CONFIG_PATH),
        help="Profile config JSON path. Defaults to flash_profiles.json in this repo.",
    )
    parser.add_argument(
        "--profile",
        default="",
        help="Target profile id to load on startup.",
    )
    parser.add_argument(
        "--parallel",
        type=int,
        default=0,
        help="Default parallel worker count override.",
    )
    parser.add_argument("--skip-build", action="store_true", help="Default Skip Build to enabled.")
    parser.add_argument(
        "--skip-efuse-check",
        action="store_true",
        help="Default Skip eFuse Check to enabled.",
    )
    parser.add_argument("--show-all-ports", action="store_true", help="Show all serial ports.")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    root = tk.Tk()
    style = ttk.Style(root)
    try:
        style.theme_use("clam")
    except tk.TclError:
        pass
    DevFlashGui(root, args)
    root.mainloop()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
