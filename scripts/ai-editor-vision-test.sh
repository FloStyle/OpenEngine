#!/usr/bin/env bash
# OpenEngine — does your vision model (unsloth) see the EDITOR? Live test.
#
# Two things must be true:
#   1. A vision-capable model is configured (config/ai-vision.json by default).
#   2. The editor has captured a frame: in the editor, click "📷 Send to AI",
#      which writes .editor/frame.png of the REAL editor window (viewport,
#      gizmos, panels). You must grab it while the editor is running.
#
# Guarded by OPENENGINE_AI_LIVE=1 (offline CI never runs this).
#
# Usage:
#   OPENENGINE_AI_LIVE=1 bash scripts/ai-editor-vision-test.sh
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "${OPENENGINE_AI_LIVE:-0}" != "1" ]]; then
  echo "SKIP: ai-editor-vision-test is gated by OPENENGINE_AI_LIVE=1 (offline CI)."
  exit 0
fi

CONFIG="${OPENENGINE_AI_CONFIG:-config/ai-vision.json}"
FRAME=".editor/frame.png"

if [[ ! -f "$CONFIG" ]]; then
  echo "no vision config at $CONFIG — set OPENENGINE_AI_CONFIG." >&2
  exit 1
fi

echo "== model config =="
cargo run -q -p openengine-ai -- describe --config "$CONFIG"

echo "== editor frame =="
if [[ ! -f "$FRAME" ]]; then
  echo "NO editor frame yet at $FRAME." >&2
  echo "Open the editor (bash scripts/editor.sh), build your scene, then click" >&2
  echo "the '📷 Send to AI' toolbar button; that writes $FRAME. Re-run me." >&2
  exit 2
fi
echo "present: $FRAME ($(wc -c < "$FRAME") bytes)"

echo "== ask the vision model what it sees in the EDITOR =="
cargo run -q -p openengine-ai -- see --editor --config "$CONFIG" \
  "You are looking at a screenshot of the OpenEngine editor UI. Describe what you see — the viewport, any actors/objects, panels, and tools — in 2-3 sentences."

echo
echo "== verdict =="
echo "If the model described the editor layout (panels/viewport/gizmos, not just a bare scene),"
echo "VISION-OK. If it described only spheres/ground, it saw the frame but confirm it is the"
echo "real editor window (click 📷 in the editor, don't reuse an old /frame)."
echo "== done =="
