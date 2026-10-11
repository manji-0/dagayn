#!/bin/bash
# Loads the flake's devShell into Claude Code cloud sessions: exports its
# environment through $CLAUDE_ENV_FILE so every Bash call sees the same tools
# as `nix develop`, then installs the Python and dagayn-vscode dependencies.
set -euo pipefail

if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

cd "$CLAUDE_PROJECT_DIR"

if ! command -v nix >/dev/null 2>&1; then
  echo "session-start: nix not found; skipping devShell setup" >&2
  exit 0
fi

# Only the tool-facing part of the devShell: PATH, the cc/bintools wrappers
# configuration, and the variables flake.nix sets. The rest describes the
# build sandbox (TMPDIR, NIX_BUILD_TOP, PYTHONHASHSEED, ...) and would leak
# into every command.
dev_env="$(nix print-dev-env --json)"
exports="$(python3 -I -c '
import json, re, shlex, sys
keep = re.compile(
    r"(CC|CXX|AR|AS|LD|NM|OBJCOPY|OBJDUMP|RANLIB|READELF|SIZE|STRINGS|STRIP)"
    r"|NIX_(CC|BINTOOLS|CFLAGS_COMPILE|LDFLAGS|HARDENING_ENABLE|ENFORCE_NO_NATIVE)\w*"
    r"|NIX_\w+_WRAPPER_TARGET_\w+"
    r"|PKG_CONFIG(_PATH)?|UV_\w+|PYO3_\w+|COREPACK_\w+"
)
for key, var in json.load(sys.stdin)["variables"].items():
    if var["type"] != "exported":
        continue
    if key == "PATH":
        print("export PATH=" + shlex.quote(var["value"]) + ":\"$PATH\"")
    elif keep.fullmatch(key):
        print("export " + key + "=" + shlex.quote(var["value"]))
' <<<"$dev_env")"

if [ -n "${CLAUDE_ENV_FILE:-}" ]; then
  printf '%s\n' "$exports" >>"$CLAUDE_ENV_FILE"
fi
eval "$exports"

# Builds the PyO3 extension (dev-fast profile) and vendors the grammars, which
# downloads from codeload.github.com and lindera.dev. Where the network policy
# blocks those, install the dependencies alone so ruff and pyrefly still run.
if ! uv sync --extra dev; then
  echo "session-start: building dagayn failed; installing its dependencies only" >&2
  uv sync --extra dev --no-install-project
fi
# corepack resolves pnpm from the cwd, so run it inside the package.
(cd dagayn-vscode && pnpm install --frozen-lockfile)
