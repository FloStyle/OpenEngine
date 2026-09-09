#!/usr/bin/env bash
# OpenEngine harness — thin curl wrapper over the headless API.
#
# Usage:
#   bash scripts/harness.sh <subcommand> [json]
#   subcommands: health | spec | observe | hash | schema
#                spawn <json> | despawn <json> | set <json> | tick <json> |
#                physics <json> | load-wasm <json> | reload <json> |
#                prove <json> | tx <json> | save <json> | load-scene <json> |
#                verify | snapshot | restore <json>
#
# Defaults to http://127.0.0.1:8080 ; override with OPENENGINE_HARNESS_URL.
set -euo pipefail

URL="${OPENENGINE_HARNESS_URL:-http://127.0.0.1:8080}"
cmd="${1:-}"; shift || true
json="${1:-}"

post() { curl -sS -X POST -H 'Content-Type: application/json' -d "$json" "$URL$1"; }

case "$cmd" in
  health)      curl -sS "$URL/health" ;;
  spec)        curl -sS "$URL/spec" ;;
  observe)     curl -sS "$URL/observe" ;;
  hash)        curl -sS "$URL/hash" ;;
  schema)      curl -sS "$URL/schema" ;;
  spawn)       post /spawn ;;
  despawn)     post /despawn ;;
  set)         post /set ;;
  tick)        post /tick ;;
  physics)     post /physics ;;
  load-wasm)   post /load_wasm ;;
  reload)      post /reload_logic ;;
  prove)       post /prove ;;
  tx)          post /transaction ;;
  save)        post /save ;;
  load-scene)  post /load ;;
  verify)      curl -sS "$URL/verify" ;;
  snapshot)    curl -sS "$URL/snapshot" ;;
  restore)     post /restore ;;
  *) echo "usage: $0 health|spec|observe|hash|schema|spawn|despawn|set|tick|physics|load-wasm|reload|prove|tx|save|load-scene|verify|snapshot|restore [json]" >&2; exit 2 ;;
esac
echo
