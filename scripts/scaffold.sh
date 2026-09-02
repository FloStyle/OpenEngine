#!/usr/bin/env bash
# OpenEngine — repository scaffold.
# Idempotent: safe to run more than once. Creates the canonical directory tree
# only; file *contents* are authored by the AI-agent onboarding (AGENTS.md).
#
# Usage:  bash scripts/scaffold.sh   (from the repository root)
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

mkdir -p \
  .github/workflows \
  .github/ISSUE_TEMPLATE \
  .ai/lock \
  .ai/sessions \
  contracts/src \
  crates/core/src \
  crates/ecs/src \
  crates/editor/src \
  crates/logic-sandbox/src \
  crates/math/src \
  brain/docs \
  brain/rag \
  docs/specs \
  docs/abi \
  scripts

# Touch .gitkeep in dirs that would otherwise be empty once scaffolded fresh.
touch .ai/lock/.gitkeep .ai/sessions/.gitkeep brain/rag/.gitkeep

echo "OpenEngine scaffold ready at $root"
echo "Members: contracts, crates/{core,ecs,editor,logic-sandbox,math}, brain, docs"
