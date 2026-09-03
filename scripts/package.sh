#!/usr/bin/env bash
# OpenEngine — cook & package a game into a runnable distributable (spec 50).
#
# A "game" = a scene file + the pure logic (logic.wasm) + the headless runner.
# This script builds the release artifacts and stages them into a self-contained
# dist/<game>/ folder with a launcher, so the game can be run and distributed.
#
# Usage:
#   bash scripts/package.sh <game> <scene.json>
# e.g. bash scripts/package.sh demo examples/demo-chase.json
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

game="${1:-demo}"
scene="${2:-examples/demo-chase.json}"
out="dist/${game}"

if [ ! -f "$scene" ]; then
  echo "scene not found: $scene" >&2; exit 2
fi

echo "==> cook: rebuild the pure logic module"
bash scripts/build.sh >/dev/null

echo "==> package: build release runner"
cargo build --release -p openengine-harness --bin openengine-runner 2>&1 | tail -1

mkdir -p "$out"
# Stage a runnable game: binary + logic + scene + launcher.
cp target/release/openengine-runner "$out/openengine-game"
cp crates/core/assets/logic.wasm "$out/logic.wasm"
cp "$scene" "$out/scene.json"

cat > "$out/run.sh" <<EOF
#!/usr/bin/env bash
# Run the packaged game (headless). Deterministic replay of the scene+logic.
cd "\$(dirname "\$0")"
./openengine-game --scene scene.json --wasm logic.wasm --frames "\${FRAMES:-120}"
EOF
chmod +x "$out/run.sh" "$out/openengine-game"

echo "==> packaged to $out"
echo "    run:  $out/run.sh  (or FRAMES=600 $out/run.sh)"
