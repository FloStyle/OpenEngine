#!/usr/bin/env bash
# OpenEngine — launch the interactive editor (double-click friendly).
#
# Usage:
#   bash scripts/editor.sh            # build (if needed) + open the editor
#   bash scripts/editor.sh --release  # force a release build
#   bash scripts/editor.sh --play scene.json   # open the editor already playing a scene
#
# Optional scene to auto-load: OPENENGINE_SCENE=path/to/scene.json
set -euo pipefail

# Repo root = parent of scripts/.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

PROFILE="debug"
EXTRA_ARGS=()
AUTO_SCENE="${OPENENGINE_SCENE:-}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --release) PROFILE="release" ;;
    --play) shift; AUTO_SCENE="${1:-}" ;;
    *) EXTRA_ARGS+=("$1") ;;
  esac
  shift || true
done

# 1) Friendly GPU/display check before we waste a build on a headless box.
if [[ -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
  echo "OpenEngine: no graphical session detected (\$DISPLAY / \$WAYLAND_DISPLAY empty)."
  echo "  The editor is a windowed app — run it from your desktop session,"
  echo "  or start the headless harness instead:  bash scripts/harness.sh health"
  echo "  (start the server with:  cargo run -p openengine-harness)"
  exit 1
fi

# 2) Build if the binary is missing (or --release requested).
BIN="target/${PROFILE}/openengine-editor-shell"
NEEDS_BUILD=0
if [[ "$PROFILE" == "release" || ! -x "$BIN" ]]; then
  NEEDS_BUILD=1
fi
if [[ "$NEEDS_BUILD" == "1" ]]; then
  echo "==> building editor ($PROFILE)… (first launch may take a while)"
  if [[ "$PROFILE" == "release" ]]; then
    cargo build --release -p openengine-editor-shell
  else
    cargo build -p openengine-editor-shell
  fi
fi

echo "==> launching OpenEngine editor ($PROFILE)"
echo "    (WASD/orbit: LMB drag · Q select · W move · E rotate · R scale ·"
echo "     + Add Actor · Ctrl+D duplicate · Delete · ▶ Play runs the wasm logic)"
if [[ -n "$AUTO_SCENE" ]]; then
  exec "$ROOT/$BIN" --play "$ROOT/$AUTO_SCENE" "${EXTRA_ARGS[@]}"
else
  exec "$ROOT/$BIN" "${EXTRA_ARGS[@]}"
fi
