"""Claude Code skills and hooks auto-install.

Generates Claude Code agent skill files, hooks configuration, and
CLAUDE.md integration for seamless dagayn usage.
Also supports multi-platform MCP server installation and
Cursor hooks / OpenCode plugin generation.
"""

from __future__ import annotations

import json
import stat
from pathlib import Path

from ..atomic_write import write_text_atomic
from .platforms import _SESSION_PREPARE_BUDGET_SECONDS, SkillPayload, _embedding_hook_args, logger

_CURSOR_WORKTREE_SETUP_COMMAND = "dagayn session prepare --budget-seconds 45"
_CURSOR_SETUP_KEYS = ("setup-worktree-unix", "setup-worktree-windows")
_CURSOR_SETUP_FALLBACK_KEY = "setup-worktree"


def _merge_cursor_setup_commands(commands: list[object]) -> list[object]:
    """Replace dagayn-managed setup commands, preserving the user's own."""
    kept = [
        command
        for command in commands
        if not (
            isinstance(command, str)
            and ("dagayn worktree" in command or "dagayn session prepare" in command)
        )
    ]
    return kept + [_CURSOR_WORKTREE_SETUP_COMMAND]


def install_cursor_worktree_setup(repo_root: Path, dry_run: bool = False) -> str:
    """Register ``dagayn session prepare`` in ``.cursor/worktrees.json``.

    Cursor runs the commands in that file inside each new worktree it creates
    for a parallel agent. ``session prepare`` inherits the main checkout's
    graph (and MCP config) then catches up the branch diff within a short
    budget, which is what makes dagayn's tools available to the agent running
    there. The command is cross-platform, so the generic ``setup-worktree``
    key is enough unless the user already maintains OS-specific keys.

    Returns ``"created"``, ``"updated"``, ``"unchanged"``, or ``"manual"`` when
    the existing config points at a setup script this cannot safely edit.
    """
    config_path = repo_root / ".cursor" / "worktrees.json"
    existing: SkillPayload = {}
    if config_path.exists():
        try:
            loaded = json.loads(config_path.read_text(encoding="utf-8", errors="replace"))
            if isinstance(loaded, dict):
                existing = loaded
            else:
                logger.warning("Unexpected shape in %s; leaving it alone.", config_path)
                return "manual"
        except (json.JSONDecodeError, OSError) as exc:
            logger.warning("Could not read %s: %s", config_path, exc)
            return "manual"

    target_keys = [key for key in _CURSOR_SETUP_KEYS if key in existing]
    if not target_keys:
        target_keys = [_CURSOR_SETUP_FALLBACK_KEY]

    updated = dict(existing)
    script_keys: list[str] = []
    for key in target_keys:
        current = updated.get(key, [])
        if isinstance(current, str):
            # A script path: appending would corrupt it. Leave it to the user.
            script_keys.append(key)
            continue
        if not isinstance(current, list):
            current = []
        updated[key] = _merge_cursor_setup_commands(current)

    if script_keys and len(script_keys) == len(target_keys):
        logger.info(
            "%s delegates setup to a script (%s); add '%s' to it manually.",
            config_path,
            ", ".join(script_keys),
            _CURSOR_WORKTREE_SETUP_COMMAND,
        )
        return "manual"

    if updated == existing:
        return "unchanged"

    state = "updated" if config_path.exists() else "created"
    if not dry_run:
        config_path.parent.mkdir(parents=True, exist_ok=True)
        write_text_atomic(config_path, json.dumps(updated, indent=2) + "\n", encoding="utf-8")
    return state


# --- Cursor hooks ---


# Matcher for Cursor beforeShellExecution / similar git-commit detectors.
# Matches bare `git commit` and absolute/relative paths like
# `/usr/bin/git commit` or `.../nix-profile/bin/git commit`.
_GIT_COMMIT_COMMAND_MATCHER = r"(?:^|[/\\]|\s)git(?:\.exe)?\s+commit\b"
# HEAD-moving git commands that should re-prepare the graph mid-session.
_GIT_RELOCATE_COMMAND_MATCHER = (
    r"(?:^|[/\\]|\s)git(?:\.exe)?\s+"
    r"(?:checkout|switch|reset|pull|merge|rebase|cherry-pick)\b"
)


_CURSOR_EDIT_HOOK_TIMEOUT_SECONDS = 15
_CURSOR_SESSION_HOOK_TIMEOUT_SECONDS = 60
_CURSOR_COMMIT_HOOK_TIMEOUT_SECONDS = 120
_CURSOR_RELOCATE_HOOK_TIMEOUT_SECONDS = 60


def generate_cursor_hooks_config() -> SkillPayload:
    """Generate Cursor hooks.json configuration.

    Returns a dict conforming to the Cursor hooks schema (version 1) with
    hooks for afterFileEdit, sessionStart, beforeShellExecution (pre-commit),
    and afterShellExecution (HEAD-moving git relocate). Each hook points to a
    shell script in ~/.cursor/hooks/.

    Returns:
        Dict suitable for writing as ~/.cursor/hooks.json.
    """
    hooks_dir = str(Path.home() / ".cursor" / "hooks")
    return {
        "version": 1,
        "hooks": {
            "afterFileEdit": [
                {
                    "command": f"{hooks_dir}/crg-update.sh",
                    "timeout": _CURSOR_EDIT_HOOK_TIMEOUT_SECONDS,
                },
            ],
            "sessionStart": [
                {
                    "command": f"{hooks_dir}/crg-session-start.sh",
                    "timeout": _CURSOR_SESSION_HOOK_TIMEOUT_SECONDS,
                },
            ],
            "beforeShellExecution": [
                {
                    "matcher": _GIT_COMMIT_COMMAND_MATCHER,
                    "command": f"{hooks_dir}/crg-pre-commit.sh",
                    "timeout": _CURSOR_COMMIT_HOOK_TIMEOUT_SECONDS,
                },
            ],
            "afterShellExecution": [
                {
                    "matcher": _GIT_RELOCATE_COMMAND_MATCHER,
                    "command": f"{hooks_dir}/crg-relocate.sh",
                    "timeout": _CURSOR_RELOCATE_HOOK_TIMEOUT_SECONDS,
                },
            ],
        },
    }


# Shared prologue for every Cursor hook script.
#
# User-level hooks (``~/.cursor/hooks.json``) run with the working directory
# set to ``~/.cursor``, not the project — so the repository must be resolved
# from the hook payload on stdin. ``dagayn hook-repo`` reads that payload and
# resolves ``workspace_roots`` / ``file_path`` through
# ``git rev-parse --show-toplevel``, which lands on the worktree a parallel
# agent session is running in rather than the main checkout.
_CURSOR_HOOK_PROLOGUE = """\
set -uo pipefail

payload="$(cat 2>/dev/null || true)"

repo="$(printf '%s' "$payload" | dagayn hook-repo --no-cwd-fallback 2>/dev/null || true)"
if [ -z "$repo" ]; then
  repo="${CURSOR_PROJECT_DIR:-${CLAUDE_PROJECT_DIR:-}}"
fi
"""


def _cursor_hook_scripts(extra_update_args: list[str] | None = None) -> dict[str, str]:
    """Return a mapping of filename -> shell script content for Cursor hooks.

    Four scripts are generated:
    - crg-update.sh: enqueues a structure-only update (``dagayn queue add
      update``) after file edits; a detached worker drains the queue
    - crg-session-start.sh: runs ``dagayn session prepare`` and reports status
    - crg-pre-commit.sh: runs ``dagayn update --skip-flows`` and
      ``dagayn detect-changes --brief`` before git commit commands
    - crg-relocate.sh: re-prepares the graph after HEAD-moving git commands
      (wired to ``afterShellExecution`` so HEAD has already moved)

    All scripts:
    - Resolve the repository from the hook payload (see
      :data:`_CURSOR_HOOK_PROLOGUE`) and pass it as ``--repo``
    - Fail gracefully (exit 0) so they never block the editor
    - Emit the JSON the corresponding Cursor hook event expects (when any)

    Args:
        extra_update_args: Embedding flags from the install (e.g.
            ``["--local-embedding"]``), applied to ``session prepare`` only
            (see :func:`_embedding_hook_args`).
    """
    prepare_args = _embedding_hook_args(extra_update_args)

    update_script = f"""\
#!/usr/bin/env bash
# dagayn: auto-update graph after file edits (Cursor hook)
# Fails gracefully — never blocks the editor.
{_CURSOR_HOOK_PROLOGUE}
# afterFileEdit fires on every edit, so enqueue instead of running an update:
# a single detached worker drains the queue and edit bursts coalesce into one
# structure-only pass (see dagayn.task_queue). The worker applies the
# hook-update budget itself, so a runaway pass is still bounded.
# afterFileEdit is observational — no output schema to satisfy.
if [ -n "$repo" ]; then
  dagayn queue add update --repo "$repo" >/dev/null 2>&1 || true
fi

exit 0
"""

    session_start_script = f"""\
#!/usr/bin/env bash
# dagayn: prepare a usable+synced graph at session start (Cursor hook)
# Fails gracefully — never blocks the editor.
{_CURSOR_HOOK_PROLOGUE}
if [ -z "$repo" ]; then
  printf '{{}}\\n'
  exit 0
fi

output="$(DAGAYN_HOOK_UPDATE=1 dagayn session prepare \\
  --budget-seconds {_SESSION_PREPARE_BUDGET_SECONDS}{prepare_args} \\
  --repo "$repo" 2>&1)" \\
  || output="dagayn: session prepare failed — run 'dagayn session prepare'"

# sessionStart accepts {{"additional_context": "..."}}; feed status to the agent.
python3 -c 'import json, sys; print(json.dumps({{"additional_context": sys.stdin.read()}}))' \\
  <<< "$output" 2>/dev/null || printf '{{}}\\n'

exit 0
"""

    pre_commit_script = f"""\
#!/usr/bin/env bash
# dagayn: detect changes before git commit (Cursor hook)
# Fails gracefully — never blocks the commit.
{_CURSOR_HOOK_PROLOGUE}
if [ -z "$repo" ]; then
  printf '{{"permission":"allow"}}\\n'
  exit 0
fi

# Refresh the graph cheaply, then run detect-changes; swallow errors.
# Structure only, and budget-bounded: the commit waits on this hook.
DAGAYN_HOOK_UPDATE=1 dagayn update --skip-flows --repo "$repo" >/dev/null 2>&1 || true
output="$(dagayn detect-changes --brief --repo "$repo" 2>&1)" || output=""

# beforeShellExecution must return a permission decision; always allow and
# attach the analysis as a message for the agent.
python3 -c 'import json, sys
print(json.dumps({{"permission": "allow", "agent_message": sys.stdin.read()}}))' \\
  <<< "$output" 2>/dev/null || printf '{{"permission":"allow"}}\\n'

exit 0
"""

    relocate_script = f"""\
#!/usr/bin/env bash
# dagayn: re-prepare graph after HEAD-moving git commands (Cursor hook)
# Wired to afterShellExecution so checkout/switch/pull have already landed.
# Fails gracefully — never blocks the editor.
{_CURSOR_HOOK_PROLOGUE}
if [ -z "$repo" ]; then
  exit 0
fi

# afterShellExecution is observational — no permission JSON required.
DAGAYN_HOOK_UPDATE=1 dagayn session prepare \\
  --budget-seconds {_SESSION_PREPARE_BUDGET_SECONDS}{prepare_args} \\
  --repo "$repo" >/dev/null 2>&1 || true

exit 0
"""

    return {
        "crg-update.sh": update_script,
        "crg-session-start.sh": session_start_script,
        "crg-pre-commit.sh": pre_commit_script,
        "crg-relocate.sh": relocate_script,
    }


def install_cursor_hooks(extra_update_args: list[str] | None = None) -> Path:
    """Install Cursor hooks configuration and scripts at user level.

    Writes ``~/.cursor/hooks.json`` (merging dagayn hooks
    into any existing configuration) and creates executable shell scripts
    in ``~/.cursor/hooks/``.

    Args:
        extra_update_args: Additional CLI args appended to the hooks'
            ``dagayn update`` command.

    Returns:
        Path to the hooks.json file that was written.
    """
    cursor_dir = Path.home() / ".cursor"
    hooks_json_path = cursor_dir / "hooks.json"
    hooks_script_dir = cursor_dir / "hooks"

    # --- Merge hooks.json ---
    existing: SkillPayload = {}
    if hooks_json_path.exists():
        try:
            existing = json.loads(hooks_json_path.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, OSError) as exc:
            logger.warning("Could not read existing %s: %s", hooks_json_path, exc)

    new_config = generate_cursor_hooks_config()

    # Preserve version (use ours if absent)
    existing.setdefault("version", new_config["version"])

    # Merge hook arrays per event type
    existing_hooks = existing.get("hooks", {})
    if not isinstance(existing_hooks, dict):
        existing_hooks = {}

    for event, entries in new_config["hooks"].items():
        event_hooks = existing_hooks.get(event, [])
        if not isinstance(event_hooks, list):
            event_hooks = []

        def _hook_script_name(command: object) -> str:
            if not isinstance(command, str) or not command:
                return ""
            return Path(command).name

        # Replace existing dagayn/crg hook entries (same command path or
        # same script basename) so matcher/timeout updates take effect.
        for entry in entries:
            entry_cmd = entry.get("command", "")
            entry_name = _hook_script_name(entry_cmd)
            replaced = False
            for idx, existing_entry in enumerate(event_hooks):
                if not isinstance(existing_entry, dict):
                    continue
                existing_cmd = existing_entry.get("command", "")
                if existing_cmd == entry_cmd or (
                    entry_name and _hook_script_name(existing_cmd) == entry_name
                ):
                    event_hooks[idx] = entry
                    replaced = True
                    break
            if not replaced:
                event_hooks.append(entry)
        existing_hooks[event] = event_hooks

    # Relocate moved from beforeShellExecution -> afterShellExecution. Strip any
    # leftover managed relocate entry so prepare does not run before HEAD moves.
    before_hooks = existing_hooks.get("beforeShellExecution", [])
    if isinstance(before_hooks, list):
        existing_hooks["beforeShellExecution"] = [
            entry
            for entry in before_hooks
            if not (
                isinstance(entry, dict)
                and Path(str(entry.get("command", ""))).name == "crg-relocate.sh"
            )
        ]

    existing["hooks"] = existing_hooks

    cursor_dir.mkdir(parents=True, exist_ok=True)
    write_text_atomic(
        hooks_json_path,
        json.dumps(existing, indent=2) + "\n",
        encoding="utf-8",
    )
    logger.info("Wrote Cursor hooks config: %s", hooks_json_path)

    # --- Write hook scripts ---
    hooks_script_dir.mkdir(parents=True, exist_ok=True)
    scripts = _cursor_hook_scripts(extra_update_args)

    for filename, content in scripts.items():
        script_path = hooks_script_dir / filename
        write_text_atomic(script_path, content, encoding="utf-8")
        # Make executable (owner rwx, group rx, other rx)
        script_path.chmod(stat.S_IRWXU | stat.S_IRGRP | stat.S_IXGRP | stat.S_IROTH | stat.S_IXOTH)
        logger.info("Wrote Cursor hook script: %s", script_path)

    return hooks_json_path
