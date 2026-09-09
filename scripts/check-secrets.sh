#!/usr/bin/env bash
# OpenEngine — guard that no secrets are tracked.
#
# Fails if:
#   1. Any tracked path matches .env (e.g. `foo/.env`, `.env.local`) EXCEPT the
#      committed template `.env.example`.
#   2. A real `.env` at the workspace root is NOT git-ignored (would be leak risk).
#   3. Ripgrep's ignore list exists but fails to exclude .env (informational).
#
# Usage: bash scripts/check-secrets.sh   (exit 0 = clean, non-zero = leak found)
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

leak=0

# 1) Any tracked .env (excluding .env.example)?
bad=$(git ls-files | grep -E '(^|/)\.env($|\.)' | grep -v '\.env\.example$' || true)
if [ -n "$bad" ]; then
  echo "SECRETS LEAK: tracked files match .env (excluding .env.example):" >&2
  echo "$bad" >&2
  leak=1
fi

# 2) Real .env must be git-ignored.
if [ -f .env ]; then
  if ! git check-ignore -q .env; then
    echo "SECRETS LEAK: .env exists but is NOT git-ignored (will be committed)." >&2
    leak=1
  else
    echo "ok: .env is present and git-ignored (not tracked)."
  fi
else
  echo "ok: no .env present (fine)."
fi

# 3) Sanity: .env.example must be tracked (the committed template).
if ! git ls-files --error-unmatch .env.example >/dev/null 2>&1; then
  echo "WARN: .env.example is not tracked — commit the placeholder template." >&2
fi

# 4) .rgignore must exclude .env so agent searches never read it.
if [ -f .rgignore ] && grep -q '^\.env$' .rgignore; then
  echo "ok: .rgignore excludes .env."
else
  echo "WARN: .rgignore does not exclude .env — agent ripgrep may read it." >&2
fi

if [ "$leak" = "1" ]; then
  echo "check-secrets: FAIL — a secret may be tracked." >&2
  exit 1
fi
echo "check-secrets: PASS (no tracked secrets)."
