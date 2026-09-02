#!/usr/bin/env bash
# OpenEngine — verify all required dev tools are present.
# Usage: bash scripts/requirements.sh
set -euo pipefail

echo "==> Checking OpenEngine requirements..."

MISSING=0

# Rust
if command -v rustc >/dev/null 2>&1; then
    echo "  [OK] rustc $(rustc --version | awk '{print $2}')"
else
    echo "  [MISSING] rustc - install from https://rustup.rs/"
    MISSING=1
fi

# Cargo
if command -v cargo >/dev/null 2>&1; then
    echo "  [OK] cargo $(cargo --version | awk '{print $2}')"
else
    echo "  [MISSING] cargo"
    MISSING=1
fi

# wasm target
if rustup target list --installed 2>/dev/null | grep -q wasm32-unknown-unknown; then
    echo "  [OK] wasm32-unknown-unknown target"
else
    echo "  [MISSING] wasm32-unknown-unknown target"
    echo "    Install: rustup target add wasm32-unknown-unknown"
    MISSING=1
fi

# python3
if command -v python3 >/dev/null 2>&1; then
    echo "  [OK] python3 $(python3 --version | awk '{print $2}')"
else
    echo "  [MISSING] python3"
    MISSING=1
fi

# Docker (optional)
if command -v docker >/dev/null 2>&1; then
    echo "  [OK] docker $(docker --version | awk '{print $3}' | tr -d ',')"
else
    echo "  [OPTIONAL] docker - needed for reproducible builds"
fi

# wasm-tools (optional but recommended for structural purity checks)
if command -v wasm-tools >/dev/null 2>&1; then
    echo "  [OK] wasm-tools"
else
    echo "  [OPTIONAL] wasm-tools - recommended for purity verification"
    echo "    Install: cargo install wasm-tools"
fi

echo ""
if [ "$MISSING" -eq 0 ]; then
    echo "==> All required tools present."
else
    echo "==> Some required tools are missing. See above."
    exit 1
fi
