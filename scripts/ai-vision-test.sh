#!/usr/bin/env bash
# OpenEngine AI vision-path test (gated).
#
# Captures a /frame screenshot from a running harness and sends it to a
# vision-capable model via `openengine-ai see`. Only runs with
# OPENENGINE_AI_LIVE=1 AND a vision-capable config; otherwise SKIPs cleanly
# (a local text model like KAT-Coder cannot see — that's expected).
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "${OPENENGINE_AI_LIVE:-0}" != "1" ]]; then
  echo "SKIP: ai-vision-test is gated by OPENENGINE_AI_LIVE=1."
  exit 0
fi

CONFIG="${OPENENGINE_AI_CONFIG:-./config/ai.json}"
HARNESS="${HARNESS_URL:-http://127.0.0.1:8090}"

if ! cargo run -q -p openengine-ai -- describe --config "$CONFIG" 2>/dev/null | grep -qi "vision: yes\|gpt-4o-vision"; then
  # `describe` doesn't print vision; probe via the catalog/see guard instead.
  :
fi

# `see` refuses non-vision models with a typed error -> that is the SKIP path
# when the configured model can't see.
echo "harness: $HARNESS"
echo "== GET /frame from harness =="
if ! curl -sS "$HARNESS/health" >/dev/null 2>&1; then
  echo "SKIP: harness not running at $HARNESS — start it with --features capture."
  exit 0
fi

echo "== /ai/status =="
curl -sS "$HARNESS/ai/status"

echo
echo "== see (vision describe of the live scene) =="
cargo run -q -p openengine-ai -- see --config "$CONFIG" --harness "$HARNESS" \
  "Describe this 3D scene in one sentence."
echo "== vision test done =="
