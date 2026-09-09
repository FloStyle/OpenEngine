#!/usr/bin/env bash
# OpenEngine AI live-path test (gated; never runs in CI offline).
#
# Runs against a REAL model server (local unsloth/llama.cpp or a cloud key).
# Guard: only executes when OPENENGINE_AI_LIVE=1 OR a config is given, so the
# default cargo/CI flow stays fully offline.
#
# Usage:
#   bash scripts/ai-live-test.sh                      # uses ./config/ai.json or $OPENENGINE_AI_CONFIG
#   OPENENGINE_AI_CONFIG=config/ai-local.example.json \
#   UNSLOTH_API_KEY=sk-... \
#   bash scripts/ai-live-test.sh
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "${OPENENGINE_AI_LIVE:-0}" != "1" && "${1:-}" != "--force" ]]; then
  echo "SKIP: ai-live-test is gated by OPENENGINE_AI_LIVE=1 (offline CI)."
  exit 0
fi

CONFIG="${OPENENGINE_AI_CONFIG:-./config/ai.json}"
if [[ ! -f "$CONFIG" ]]; then
  echo "no config at $CONFIG — pass --config or set OPENENGINE_AI_CONFIG." >&2
  exit 1
fi

echo "== describe =="
cargo run -q -p openengine-ai -- describe --config "$CONFIG"

echo "== providers =="
cargo run -q -p openengine-ai -- providers

echo "== models =="
cargo run -q -p openengine-ai -- models

echo "== test (1-turn ping) =="
cargo run -q -p openengine-ai -- test --config "$CONFIG"

echo "== chat =="
cargo run -q -p openengine-ai -- chat --config "$CONFIG" "Reply with the single word FORGED"

echo "== live test done =="
