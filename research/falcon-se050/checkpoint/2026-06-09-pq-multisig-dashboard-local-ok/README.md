# PQ Multisig Dashboard Local Smoke OK

Date: 2026-06-09

## Goal

Verify the thin operator dashboard layer before mainnet:

1. start the local read-only dashboard server
2. confirm the HTTP surface is healthy
3. confirm alias discovery works from the browser/backend path
4. confirm dashboard-to-CLI manifest handoff works through `--manifest-only`

## Commands

```bash
./scripts/run_pq_multisig_dashboard.sh
```

Health check:

```bash
curl -s http://127.0.0.1:8791/api/health
```

Alias discovery:

```bash
curl -s "http://127.0.0.1:8791/api/aliases?path=./docs/pq-multisig-ops.aliases.example.json"
```

Manifest handoff:

```bash
curl -s http://127.0.0.1:8791/api/ops \
  -H 'Content-Type: application/json' \
  -d '{
    "args": [
      "--rpc-url", "http://127.0.0.1:8899",
      "--vector-program-id", "FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU",
      "--vault-program-id", "9v16YaZYWXrMdVjGnhFQTE9yHerSchNJg18bqSCZUF52",
      "--aliases-file", "./docs/pq-multisig-ops.aliases.example.json",
      "--mode", "approve",
      "--vault-alias", "dev-vault",
      "--member-alias", "alice",
      "--proposal-id", "7",
      "--manifest-only"
    ],
    "parse_json": true
  }'
```

## Verified behavior

- dashboard server started locally
- `/api/health` returned `ok: true`
- `/api/aliases` loaded the checked-in alias example and exposed:
  - vault aliases: `dev-vault`, `mainnet-team`
  - member aliases: `alice`, `bob`, `carol`, `dave`
- `/` served the dashboard HTML shell successfully
- `/api/ops` successfully returned a parsed manifest JSON for a local approve handoff
- the dashboard stayed read-only and delegated actions back to `./scripts/run_pq_multisig_ops.sh`

## Notes

- The dashboard intentionally does not sign or submit transactions in-browser.
- It uses the same CLI surfaces already verified elsewhere:
  - `--json` for reads
  - `--manifest-only` for action handoff
