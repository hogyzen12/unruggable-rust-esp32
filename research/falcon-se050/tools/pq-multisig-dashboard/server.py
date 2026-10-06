#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import shlex
import subprocess
import sys
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, unquote, urlparse


ROOT_DIR = Path(__file__).resolve().parents[2]
STATIC_DIR = Path(__file__).resolve().parent / "static"
OPS_BINARY = ROOT_DIR / "tools" / "pq-multisig-ops" / "target" / "aarch64-apple-darwin" / "debug" / "pq-multisig-ops"
OPS_SCRIPT = ROOT_DIR / "scripts" / "run_pq_multisig_ops.sh"


def resolve_ops_command() -> list[str]:
    custom = os.environ.get("PQ_MULTISIG_OPS_CMD")
    if custom:
        return shlex.split(custom)
    if OPS_BINARY.exists():
        return [str(OPS_BINARY)]
    return [str(OPS_SCRIPT)]


def run_ops(args: list[str], parse_json: bool) -> dict:
    cmd = resolve_ops_command() + args
    completed = subprocess.run(
        cmd,
        cwd=ROOT_DIR,
        capture_output=True,
        text=True,
        check=False,
    )
    stdout = completed.stdout.strip()
    stderr = completed.stderr.strip()
    payload: dict[str, object] = {
        "ok": completed.returncode == 0,
        "command": cmd,
        "stdout": stdout,
        "stderr": stderr,
        "returncode": completed.returncode,
    }
    if completed.returncode != 0:
        return payload
    if parse_json:
        payload["json"] = json.loads(stdout) if stdout else None
    return payload


def load_aliases(path_value: str) -> dict:
    path = Path(unquote(path_value)).expanduser()
    with path.open("r", encoding="utf-8") as fh:
        data = json.load(fh)
    return {
        "path": str(path),
        "vault_aliases": sorted((data.get("vaults") or {}).keys()),
        "member_aliases": sorted((data.get("members") or {}).keys()),
        "raw": data,
    }


class DashboardHandler(BaseHTTPRequestHandler):
    server_version = "PQMultisigDashboard/0.1"

    def log_message(self, format: str, *args) -> None:
        return

    def _send_json(self, status: int, payload: dict) -> None:
        body = json.dumps(payload, indent=2).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _send_text(self, status: int, text: str, content_type: str) -> None:
        body = text.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", f"{content_type}; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        parsed = urlparse(self.path)
        if parsed.path == "/api/health":
            cmd = resolve_ops_command()
            self._send_json(
                HTTPStatus.OK,
                {
                    "ok": True,
                    "root_dir": str(ROOT_DIR),
                    "static_dir": str(STATIC_DIR),
                    "ops_command": cmd,
                    "ops_path_exists": Path(cmd[0]).exists(),
                },
            )
            return

        if parsed.path == "/api/aliases":
            query = parse_qs(parsed.query)
            path_value = query.get("path", [""])[0]
            if not path_value:
                self._send_json(HTTPStatus.BAD_REQUEST, {"ok": False, "error": "missing aliases path"})
                return
            try:
                payload = load_aliases(path_value)
            except Exception as exc:  # noqa: BLE001
                self._send_json(HTTPStatus.BAD_REQUEST, {"ok": False, "error": str(exc)})
                return
            self._send_json(HTTPStatus.OK, {"ok": True, **payload})
            return

        if parsed.path == "/" or parsed.path == "/index.html":
            target = STATIC_DIR / "index.html"
            self._send_text(HTTPStatus.OK, target.read_text(encoding="utf-8"), "text/html")
            return

        if parsed.path == "/app.js":
            target = STATIC_DIR / "app.js"
            self._send_text(HTTPStatus.OK, target.read_text(encoding="utf-8"), "application/javascript")
            return

        if parsed.path == "/styles.css":
            target = STATIC_DIR / "styles.css"
            self._send_text(HTTPStatus.OK, target.read_text(encoding="utf-8"), "text/css")
            return

        self._send_json(HTTPStatus.NOT_FOUND, {"ok": False, "error": "not found"})

    def do_POST(self) -> None:
        parsed = urlparse(self.path)
        if parsed.path != "/api/ops":
            self._send_json(HTTPStatus.NOT_FOUND, {"ok": False, "error": "not found"})
            return

        try:
            content_length = int(self.headers.get("Content-Length", "0"))
            body = self.rfile.read(content_length).decode("utf-8") if content_length else "{}"
            payload = json.loads(body)
        except Exception as exc:  # noqa: BLE001
            self._send_json(HTTPStatus.BAD_REQUEST, {"ok": False, "error": f"invalid JSON body: {exc}"})
            return

        args = payload.get("args")
        parse_json = bool(payload.get("parse_json", False))
        if not isinstance(args, list) or not all(isinstance(item, str) for item in args):
            self._send_json(HTTPStatus.BAD_REQUEST, {"ok": False, "error": "args must be a string array"})
            return

        try:
            result = run_ops(args, parse_json=parse_json)
        except Exception as exc:  # noqa: BLE001
            self._send_json(HTTPStatus.INTERNAL_SERVER_ERROR, {"ok": False, "error": str(exc)})
            return

        status = HTTPStatus.OK if result.get("ok") else HTTPStatus.BAD_REQUEST
        self._send_json(status, result)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Local read-only PQ multisig dashboard")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8787)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    server = ThreadingHTTPServer((args.host, args.port), DashboardHandler)
    print("============================================================")
    print("PQ Multisig Dashboard")
    print("============================================================")
    print(f"URL:                  http://{args.host}:{args.port}")
    print(f"Root dir:             {ROOT_DIR}")
    print(f"Ops command:          {' '.join(resolve_ops_command())}")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print()
        print("Stopping dashboard...")
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
