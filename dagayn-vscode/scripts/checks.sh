#!/usr/bin/env bash
# Runs dagayn-vscode fmt/lint/test/test:compile. Intended for pre-commit (prek)
# and local use. Exits non-zero on any failure.
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$DIR"

echo "[dagayn-vscode] fmt:check"
pnpm fmt:check
echo "[dagayn-vscode] lint"
pnpm lint
echo "[dagayn-vscode] test"
pnpm test
echo "[dagayn-vscode] test:compile"
pnpm test:compile
