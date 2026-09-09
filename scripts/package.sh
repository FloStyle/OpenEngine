#!/usr/bin/env bash
# OpenEngine — cook & package a game into a runnable distributable (spec 50).
#
# A "game" = a scene file + optional pure logic (logic.wasm) + the headless
# runner. Staged into dist/<game>/ with a launcher.
#
# Modes:
#   logic   (default)  run the Domain-B wasm gameplay (WASD/jump/NPC).
#   physics             run the deterministic Domain-B physics (no wasm needed).
#
# Usage:
#   bash scripts/package.sh <game> <scene.json> [physics]
# e.g. bash scripts/package.sh demo examples/demo-chase.json
#      bash scripts/package.sh phys examples/demo-physics.json physics
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

game="${1:-demo}"
scene="${2:-examples/demo-chase.json}"
mode="${3:-logic}"
out="dist/${game}"

if [ ! -f "$scene" ]; then
  echo "scene not found: $scene" >&2; exit 2
fi
if [ "$mode" != "logic" ] && [ "$mode" != "physics" ]; then
  echo "mode must be 'logic' or 'physics'" >&2; exit 2
fi

echo "==> cook: rebuild the pure logic module"
bash scripts/build.sh >/dev/null

echo "==> package: build release runner"
cargo build --release -p openengine-harness --bin openengine-runner 2>&1 | tail -1

mkdir -p "$out"

# Guard: a distributable must never carry local secrets (.env or real configs).
if ls "$out" >/dev/null 2>&1 && find "$out" -name '.env' -o -name '.env.*' 2>/dev/null | grep -v '\.example$' | grep -q .; then
  echo "SECRETS: refusing to package a .env into dist/ — remove it first." >&2
  exit 3
fi

# Stage the runnable: binary + scene + launcher (logic.wasm only in logic mode).
cp target/release/openengine-runner "$out/openengine-game"
cp "$scene" "$out/scene.json"

if [ "$mode" = "physics" ]; then
  cat > "$out/run.sh" <<EOF
#!/usr/bin/env bash
# Deterministic Domain-B physics replay of the scene (no wasm).
cd "\$(dirname "\$0")"
./openengine-game --scene scene.json --physics --frames "\${FRAMES:-400}"
EOF
else
  cp crates/core/assets/logic.wasm "$out/logic.wasm"
  cat > "$out/run.sh" <<EOF
#!/usr/bin/env bash
# Deterministic replay of the scene + wasm gameplay logic.
# FRAMES = ticks; FORWARD = hold the player's forward for the first N ticks.
cd "\$(dirname "\$0")"
./openengine-game --scene scene.json --wasm logic.wasm \
  --frames "\${FRAMES:-120}" --forward "\${FORWARD:-0}"
EOF
fi
chmod +x "$out/run.sh" "$out/openengine-game"

echo "==> packaged to $out (mode: $mode)"
echo "    run:  $out/run.sh  (FRAMES=600 $out/run.sh)"
