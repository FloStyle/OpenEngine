#!/usr/bin/env bash
# OpenEngine — Domain B → Domain A build bridge.
#
# Compiles the pure logic to a no_std wasm cdylib and stages it where the
# `openengine-core` host binary reads it (`crates/core/assets/logic.wasm`).
#
# Usage:  bash scripts/build.sh
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# Cross-target for Domain B. Kept in sync with rust-toolchain.toml.
wasm_target="wasm32-unknown-unknown"
# The wasm cdylib that carries the #[no_mangle] ABI trampoline.
wasm_crate="openengine-logic-export"
out_asset="crates/core/assets/logic.wasm"

echo "==> Building ${wasm_crate} for ${wasm_target} (no_std + wasm-alloc)"
cargo build -p "${wasm_crate}" \
    --target "${wasm_target}" \
    --features wasm-alloc \
    --release

mkdir -p crates/core/assets
# Cargo cdylib artifact: package name "openengine-logic-export" becomes the
# lib name "openengine_logic_export" (hyphens -> underscores) + ".wasm".
lib_name="${wasm_crate//-/_}"
src="target/${wasm_target}/release/lib${lib_name}.wasm"
if [ ! -f "$src" ]; then
    src="target/${wasm_target}/release/${lib_name}.wasm"
fi
cp "$src" "$out_asset"
echo "==> Staged logic module: $out_asset"
echo "==> Next: cargo run -p openengine-core"
