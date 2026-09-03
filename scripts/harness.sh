#!/usr/bin/env bash
# OpenEngine harness — thin curl wrapper over the headless API.
#
# Usage:
#   bash scripts/harness.sh <subcommand> [json]
#   subcommands: health | observe | spec | hash
#                spawn <json> | despawn <json> | set <json> | tick <json> |
#                load <json> | prove <json> | tx <json>
#
# Defaults to http://127.0.0.1:8080 ; override with OPENENGINE_HARNESS_URL.
set -euo pipefail

URL="${OPENENGINE_HARNESS_URL:-http://127.0.0.1:8080}"
cmd="${1:-}"; shift || true
json="${1:-}"

post() { curl -sS -X POST -H 'Content-Type: application/json' -d "$json" "$URL$1"; }

case "$cmd" in
  health)    curl -sS "$URL/health" ;;
  spec)      curl -sS "$URL/spec" ;;
  observe)   curl -sS "$URL/observe" ;;
  hash)      curl -sS "$URL/hash" ;;
  spawn)     post /spawn ;;
  despawn)   post /despawn ;;
  set)       post /set ;;
  tick)      post /tick ;;
  load)      post /load_wasm ;;
  prove)     post /prove ;;
  tx)        post /transaction ;;
  *) echo "usage: $0 health|spec|observe|hash|spawn|despawn|set|tick|load|prove|tx [json]" >&2; exit 2 ;;
esac
echo
