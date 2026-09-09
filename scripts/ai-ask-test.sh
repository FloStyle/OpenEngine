#!/usr/bin/env bash
# OpenEngine /ask live test (gated; never runs in offline CI).
#
# Requires a running harness (with config) + a reachable model server.
# Guard: only runs when OPENENGINE_AI_LIVE=1.
#
# Usage:
#   OPENENGINE_AI_LIVE=1 OPENENGINE_AI_CONFIG=config/ai-vision.json \
#     HARNESS_URL=http://127.0.0.1:8090 bash scripts/ai-ask-test.sh
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "${OPENENGINE_AI_LIVE:-0}" != "1" ]]; then
  echo "SKIP: ai-ask-test is gated by OPENENGINE_AI_LIVE=1 (offline CI)."
  exit 0
fi

H="${HARNESS_URL:-http://127.0.0.1:8090}"

echo "== /ask chat =="
curl -sS -X POST "$H/ask" -H 'Content-Type: application/json' \
  -d '{"message":"Reply with the single word PONG"}' | python3 -m json.tool

echo "== /ask propose (spawn a blue entity at [1,0,0]) =="
curl -sS -X POST "$H/ask" -H 'Content-Type: application/json' \
  -d '{"message":"Add one entity at [1,0,0] colored blue [0,0,255,255]. Reply with ONLY the JSON proposal batch.","propose":true}' | python3 -m json.tool

echo "== observe (entity should now exist) =="
curl -sS "$H/observe" | python3 -c 'import sys,json;d=json.load(sys.stdin);print("entities",d["entity_count"])'

echo "== /ask vision:true (model sees the scene) =="
curl -sS -X POST "$H/ask" -H 'Content-Type: application/json' \
  -d '{"message":"What do you SEE in the scene? Describe in one sentence.","vision":true}' | python3 -m json.tool
echo "== ai-ask-test done =="
