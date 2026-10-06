#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HOST="127.0.0.1"
PORT="8787"

print_usage() {
  cat <<'EOF'
Usage:
  ./scripts/run_pq_multisig_dashboard.sh [options]

Options:
  --host <host>   Host to bind. Default: 127.0.0.1
  --port <port>   Port to bind. Default: 8787
  -h, --help      Show this help text
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --host)
      HOST="$2"
      shift 2
      ;;
    --port)
      PORT="$2"
      shift 2
      ;;
    -h|--help)
      print_usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

exec python3 "$ROOT_DIR/tools/pq-multisig-dashboard/server.py" --host "$HOST" --port "$PORT"
