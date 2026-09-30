"""Claude Code skills and hooks auto-install.

Generates Claude Code agent skill files, hooks configuration, and
CLAUDE.md integration for seamless dagayn usage.
Also supports multi-platform MCP server installation and
Cursor hooks / OpenCode plugin generation.
"""

from __future__ import annotations

import json
import re
import shutil
import stat
from pathlib import Path
from typing import Any

import yaml

from ..atomic_write import write_text_atomic
from .platforms import (
    _SESSION_PREPARE_BUDGET_SECONDS,
    _STATUS_HOOK_TIMEOUT_SECONDS,
    _UPDATE_HOOK_TIMEOUT_SECONDS,
    SkillPayload,
    _embedding_hook_args,
    _merge_hermes_hook_entries,
    _merge_pi_hook_entries,
    logger,
    normalize_platform_target,
)

#: Narrow ``$repo`` to a jj workspace nested inside it. Such a workspace
#: (``track`` / ``jj workspace add``) has no ``.git``, so ``git rev-parse``
#: answers with the enclosing main checkout.
_SHELL_JJ_WORKSPACE_NARROWING = (
    'jj_ws="$(jj workspace root 2>/dev/null || true)"; '
    'case "$jj_ws" in "$repo"/?*) repo="$jj_ws" ;; esac'
)


def _dagayn_hook_scripts(extra_update_args: list[str] | None = None) -> dict[str, str]:
    """Return shell scripts shared by hook integrations that expect JSON stdout."""
    prepare_args = _embedding_hook_args(extra_update_args)
    return {
        "dagayn-update.sh": f"""#!/usr/bin/env bash
# dagayn: auto-update graph after agent file/tool activity
# Enqueues a structure-only update; a single detached worker drains the
# queue so edit bursts coalesce (see dagayn.task_queue). Embeddings are
# refreshed by the session-start hook.
set -u
cat >/dev/null || true
repo="$(git rev-parse --show-toplevel 2>/dev/null || true)"
{_SHELL_JJ_WORKSPACE_NARROWING}
if [ -n "$repo" ]; then
  dagayn queue add update --repo "$repo" >/dev/null 2>&1 || true
fi
printf '{{}}\\n'
""",
        "dagayn-status.sh": f"""#!/usr/bin/env bash
# dagayn: prepare a usable+synced graph at session start
set -u
cat >/dev/null || true
repo="$(git rev-parse --show-toplevel 2>/dev/null || true)"
{_SHELL_JJ_WORKSPACE_NARROWING}
if [ -n "$repo" ]; then
  DAGAYN_HOOK_UPDATE=1 dagayn session prepare \\
    --budget-seconds {_SESSION_PREPARE_BUDGET_SECONDS}{prepare_args} \\
    --repo "$repo" >/dev/null 2>&1 || true
fi
printf '{{}}\\n'
""",
    }


def _write_hook_scripts(hooks_dir: Path, scripts: dict[str, str]) -> None:
    hooks_dir.mkdir(parents=True, exist_ok=True)
    for filename, content in scripts.items():
        script_path = hooks_dir / filename
        write_text_atomic(script_path, content, encoding="utf-8")
        script_path.chmod(stat.S_IRWXU | stat.S_IRGRP | stat.S_IXGRP | stat.S_IROTH | stat.S_IXOTH)


def generate_hermes_hooks_config() -> SkillPayload:
    """Generate Hermes Agent shell hook entries for dagayn refreshes."""
    hooks_dir = str(Path.home() / ".hermes" / "agent-hooks")
    return {
        "post_tool_call": [
            {
                "matcher": "terminal|write_file|patch",
                "command": f"{hooks_dir}/dagayn-update.sh",
                "timeout": _UPDATE_HOOK_TIMEOUT_SECONDS,
            }
        ],
        "on_session_start": [
            {
                "command": f"{hooks_dir}/dagayn-status.sh",
                "timeout": _STATUS_HOOK_TIMEOUT_SECONDS,
            }
        ],
    }


def install_hermes_hooks(
    extra_update_args: list[str] | None = None,
) -> Path:
    """Install Hermes Agent shell hooks in ``~/.hermes/config.yaml``."""
    hermes_dir = Path.home() / ".hermes"
    config_path = hermes_dir / "config.yaml"
    existing: SkillPayload = {}
    if config_path.exists():
        try:
            loaded = yaml.safe_load(config_path.read_text(encoding="utf-8", errors="replace"))
            if isinstance(loaded, dict):
                existing = loaded
            elif loaded is not None:
                logger.warning("Invalid YAML shape in %s, will overwrite.", config_path)
        except (yaml.YAMLError, OSError) as exc:
            logger.warning("Could not read existing %s: %s", config_path, exc)
    if config_path.exists():
        backup_path = hermes_dir / "config.yaml.bak"
        shutil.copy2(config_path, backup_path)
        logger.info("Backed up existing Hermes config to %s", backup_path)

    _write_hook_scripts(hermes_dir / "agent-hooks", _dagayn_hook_scripts(extra_update_args))
    existing_hooks = existing.get("hooks", {})
    if not isinstance(existing_hooks, dict):
        existing_hooks = {}
    existing["hooks"] = _merge_hermes_hook_entries(existing_hooks, generate_hermes_hooks_config())

    config_path.parent.mkdir(parents=True, exist_ok=True)
    write_text_atomic(config_path, yaml.safe_dump(existing, sort_keys=False), encoding="utf-8")
    return config_path


def generate_pi_hooks_config() -> list[SkillPayload]:
    """Generate pi-yaml-hooks entries for dagayn refreshes."""
    hooks_dir = str(Path.home() / ".pi" / "agent" / "hook")
    return [
        {
            "event": "file.changed",
            "actions": [{"bash": f"{hooks_dir}/dagayn-update.sh"}],
        },
        {
            "event": "session.created",
            "actions": [{"bash": f"{hooks_dir}/dagayn-status.sh"}],
        },
    ]


def install_pi_hooks(
    extra_update_args: list[str] | None = None,
) -> Path:
    """Install pi-yaml-hooks config and scripts for dagayn refreshes.

    Pi loads this only when the ``pi-yaml-hooks`` extension is installed.
    """
    hook_dir = Path.home() / ".pi" / "agent" / "hook"
    hooks_path = hook_dir / "hooks.yaml"
    existing: SkillPayload = {}
    if hooks_path.exists():
        try:
            loaded = yaml.safe_load(hooks_path.read_text(encoding="utf-8", errors="replace"))
            if isinstance(loaded, dict):
                existing = loaded
            elif loaded is not None:
                logger.warning("Invalid YAML shape in %s, will overwrite.", hooks_path)
        except (yaml.YAMLError, OSError) as exc:
            logger.warning("Could not read existing %s: %s", hooks_path, exc)
    if hooks_path.exists():
        backup_path = hook_dir / "hooks.yaml.bak"
        shutil.copy2(hooks_path, backup_path)
        logger.info("Backed up existing Pi hooks to %s", backup_path)

    _write_hook_scripts(hook_dir, _dagayn_hook_scripts(extra_update_args))
    existing_hooks = existing.get("hooks", [])
    if not isinstance(existing_hooks, list):
        existing_hooks = []
    existing["hooks"] = _merge_pi_hook_entries(existing_hooks, generate_pi_hooks_config())

    hooks_path.parent.mkdir(parents=True, exist_ok=True)
    write_text_atomic(hooks_path, yaml.safe_dump(existing, sort_keys=False), encoding="utf-8")
    return hooks_path


def generate_hooks_config(
    repo_root: Path,
    extra_update_args: list[str] | None = None,
    *,
    worktree_hook: bool = True,
) -> SkillPayload:
    """Generate Claude Code hooks configuration.

    Hooks use the v1.x+ schema: each entry needs a ``matcher`` and a nested
    ``hooks`` array. Timeouts are in seconds. ``PreCommit`` is not a valid
    Claude Code event — pre-commit checks are handled by ``install_git_hook``.

    Args:
        repo_root: Unused; hooks resolve the active repository at runtime.
        extra_update_args: Embedding flags from the install, applied to
            ``session prepare`` only (see :func:`_embedding_hook_args`).
        worktree_hook: Include the ``EnterWorktree`` / ``ExitWorktree``
            ``PostToolUse`` entry. Disable for hosts without those tools
            (Codex), where the entry would never match.
    """
    del repo_root  # Hooks are global; resolve the active repository at runtime.
    prepare_args = _embedding_hook_args(extra_update_args)
    # ``git rev-parse`` first: hooks run in the session's working directory, so
    # in a worktree session it resolves to that worktree rather than the main
    # checkout. ``CLAUDE_PROJECT_DIR`` covers a cwd outside the repository.
    repo_expr = (
        'repo="$(git rev-parse --show-toplevel 2>/dev/null)"'
        ' || repo="${CLAUDE_PROJECT_DIR:-}"; '
        f'{_SHELL_JJ_WORKSPACE_NARROWING}; [ -n "$repo" ]'
    )
    post_tool_use: list[SkillPayload] = [
        {
            # Edit/Write only: Bash fires on every shell command, including the
            # majority that touch no tracked file, so the graph was re-diffed
            # constantly for nothing. Commits are covered by the pre-commit hook
            # and HEAD moves by the relocate/session-prepare hooks.
            "matcher": "Edit|Write",
            "hooks": [
                {
                    "type": "command",
                    # Enqueue a structure-only update instead of running one
                    # inline: a burst of edits coalesces into a single task
                    # drained by one detached worker (dagayn.task_queue), so
                    # the graph no longer re-diffs per keystroke batch.
                    # Embeddings stay out of this path entirely.
                    "command": (f'{repo_expr} && dagayn queue add update --repo "$repo" || true'),
                    "timeout": _UPDATE_HOOK_TIMEOUT_SECONDS,
                },
            ],
        },
    ]
    if worktree_hook:
        post_tool_use.append(
            {
                # Entering a worktree switches the session to a fresh checkout
                # with no .dagayn/ — prepare inherits the main checkout's graph
                # and catches up on the branch diff within a short budget.
                "matcher": "EnterWorktree|ExitWorktree",
                "hooks": [
                    {
                        "type": "command",
                        "command": (
                            f"DAGAYN_HOOK_UPDATE=1 dagayn session prepare --from-hook"
                            f" --budget-seconds {_SESSION_PREPARE_BUDGET_SECONDS}"
                            f"{prepare_args} || true"
                        ),
                        "timeout": _UPDATE_HOOK_TIMEOUT_SECONDS,
                    },
                ],
            }
        )
    return {
        "hooks": {
            "PostToolUse": post_tool_use,
            "SessionStart": [
                {
                    "matcher": "",
                    "hooks": [
                        {
                            "type": "command",
                            "command": (
                                f"{repo_expr}"
                                f" && DAGAYN_HOOK_UPDATE=1 dagayn session prepare"
                                f" --budget-seconds {_SESSION_PREPARE_BUDGET_SECONDS}"
                                f"{prepare_args}"
                                ' --repo "$repo"'
                                " || echo 'Not a git repo, skipping'"
                            ),
                            "timeout": _STATUS_HOOK_TIMEOUT_SECONDS,
                        },
                    ],
                },
            ],
        }
    }


#: Command fragments that identify a hook entry as dagayn-generated, so a
#: re-install replaces it instead of appending a duplicate. Entries written by
#: older dagayn versions are listed too: a session that still runs the legacy
#: ``dagayn status`` session-start hook only *seeds* a worktree graph and never
#: catches up the branch diff, so the stale entry must be removed rather than
#: left running alongside ``dagayn session prepare``.
_DAGAYN_HOOK_NEEDLES: dict[str, tuple[str, ...]] = {
    "PostToolUse": (
        "dagayn queue add update",
        "dagayn update --skip-flows",
        "dagayn session prepare",
        # <= 4.8.2 wrote this for EnterWorktree/ExitWorktree.
        "dagayn worktree sync",
    ),
    "SessionStart": (
        "dagayn session prepare",
        # <= 4.8.2 wrote a seed-only status call here.
        "dagayn status",
    ),
}


def _is_dagayn_generated_hook_entry(hook_name: str, entry: Any) -> bool:
    """Return True for hook entries generated by dagayn itself."""
    if not isinstance(entry, dict):
        return False
    needles = _DAGAYN_HOOK_NEEDLES.get(hook_name)
    if not needles:
        return False
    hooks = entry.get("hooks", [])
    if not isinstance(hooks, list):
        return False
    return any(
        isinstance(hook, dict) and any(needle in str(hook.get("command", "")) for needle in needles)
        for hook in hooks
    )


def _merge_dagayn_hook_entries(
    existing_hooks: SkillPayload,
    hooks_config: SkillPayload,
) -> SkillPayload:
    """Merge hook config, replacing stale dagayn-generated entries in place."""
    merged_hooks = dict(existing_hooks)
    for hook_name, hook_entries in hooks_config.get("hooks", {}).items():
        if not isinstance(hook_entries, list):
            continue
        if isinstance(merged_hooks.get(hook_name), list):
            merged_list = [
                entry
                for entry in merged_hooks[hook_name]
                if not _is_dagayn_generated_hook_entry(hook_name, entry)
            ]
        else:
            merged_list = []
        for entry in hook_entries:
            if entry not in merged_list:
                merged_list.append(entry)
        merged_hooks[hook_name] = merged_list
    return merged_hooks


def _ensure_codex_hooks_feature(config_path: Path) -> None:
    """Enable Codex hooks in config.toml without clobbering settings."""
    if not config_path.exists():
        write_text_atomic(config_path, "[features]\nhooks = true\n", encoding="utf-8")
        return

    existing = config_path.read_text(encoding="utf-8", errors="replace")
    lines = existing.splitlines()
    in_features = False
    features_index: int | None = None
    hooks_index: int | None = None
    codex_hooks_index: int | None = None

    for index, line in enumerate(lines):
        if line.strip() == "[features]":
            in_features = True
            features_index = index
            continue
        if in_features and line.lstrip().startswith("[") and line.strip().endswith("]"):
            in_features = False
        if not in_features:
            continue
        if re.match(r"^\s*hooks\s*=", line):
            hooks_index = index
        elif re.match(r"^\s*codex_hooks\s*=", line):
            codex_hooks_index = index

    if hooks_index is not None:
        lines[hooks_index] = re.sub(r"=\s*.*$", "= true", lines[hooks_index], count=1)
        if codex_hooks_index is not None:
            del lines[codex_hooks_index]
        write_text_atomic(config_path, "\n".join(lines) + "\n", encoding="utf-8")
        return

    if codex_hooks_index is not None:
        lines[codex_hooks_index] = re.sub(
            r"codex_hooks\s*=\s*.*$", "hooks = true", lines[codex_hooks_index], count=1
        )
        write_text_atomic(config_path, "\n".join(lines) + "\n", encoding="utf-8")
        return

    if features_index is not None:
        lines.insert(features_index + 1, "hooks = true")
        write_text_atomic(config_path, "\n".join(lines) + "\n", encoding="utf-8")
        return

    prefix = existing if existing.endswith("\n") else existing + "\n"
    if not prefix.endswith("\n\n"):
        prefix += "\n"
    write_text_atomic(config_path, prefix + "[features]\nhooks = true\n", encoding="utf-8")


def install_codex_hooks(
    repo_root: Path,
    extra_update_args: list[str] | None = None,
) -> Path:
    """Write Codex global hooks.json and enable the hooks feature flag."""
    codex_dir = Path.home() / ".codex"
    codex_dir.mkdir(parents=True, exist_ok=True)

    hooks_path = codex_dir / "hooks.json"
    existing: SkillPayload = {}
    if hooks_path.exists():
        try:
            existing = json.loads(hooks_path.read_text(encoding="utf-8", errors="replace"))
            backup_path = codex_dir / "hooks.json.bak"
            shutil.copy2(hooks_path, backup_path)
            logger.info("Backed up existing Codex hooks to %s", backup_path)
        except (json.JSONDecodeError, OSError) as exc:
            logger.warning("Could not read existing %s: %s", hooks_path, exc)

    # Codex has no worktree tools, so the EnterWorktree entry would never match.
    hooks_config = generate_hooks_config(
        repo_root,
        extra_update_args=extra_update_args,
        worktree_hook=False,
    )
    existing_hooks = existing.get("hooks", {})
    if not isinstance(existing_hooks, dict):
        logger.warning("Existing Codex hooks config is not a dict; replacing with defaults")
        existing_hooks = {}

    existing["hooks"] = _merge_dagayn_hook_entries(existing_hooks, hooks_config)
    write_text_atomic(hooks_path, json.dumps(existing, indent=2) + "\n", encoding="utf-8")
    _ensure_codex_hooks_feature(codex_dir / "config.toml")
    logger.info("Wrote Codex hooks config: %s", hooks_path)
    return hooks_path


def _install_git_hook_script(hook_path: Path, script: str, marker: str) -> None:
    """Install or replace one dagayn-managed block in a git hook."""
    if hook_path.exists():
        existing = hook_path.read_text(encoding="utf-8")
        if marker in existing:
            hook_path.chmod(0o755)
            return
        old_marker = "# Installed by dagayn. Remove this file to disable pre-commit graph checks."
        if old_marker in existing and "dagayn detect-changes" in existing:
            existing = existing[: existing.index(old_marker)].rstrip("\n")
        write_text_atomic(hook_path, existing.rstrip("\n") + "\n" + script, encoding="utf-8")
    else:
        write_text_atomic(hook_path, script, encoding="utf-8")

    hook_path.chmod(0o755)


def install_git_hook(repo_root: Path) -> Path | None:
    """Install git hooks that keep the graph current around commits.

    Called automatically by ``dagayn install``
    Creates ``pre-commit`` and ``post-commit`` in the repository's hooks
    directory if they don't exist, or appends to existing hooks — preserving
    any hooks already there. Returns None when no hooks directory can be
    resolved.

    The hooks directory is resolved through git, so this works when
    ``dagayn install`` runs inside a linked worktree (where ``.git`` is a file,
    not a directory) and honors ``core.hooksPath``. Git shares one hooks
    directory across every worktree, so a single install covers them all.
    """
    pre_commit_script = """\
#!/bin/sh
# >>> dagayn pre-commit
# Installed by dagayn. Remove this block to disable pre-commit graph checks.
if command -v dagayn >/dev/null 2>&1; then
    dagayn update --skip-flows || true
    dagayn detect-changes --brief || true
fi
# <<< dagayn pre-commit
"""
    post_commit_script = """\
#!/bin/sh
# >>> dagayn post-commit
# Installed by dagayn. Remove this block to disable post-commit graph refresh.
if command -v dagayn >/dev/null 2>&1; then
    dagayn update || true
fi
# <<< dagayn post-commit
"""
    pre_marker = "# >>> dagayn pre-commit"
    post_marker = "# >>> dagayn post-commit"

    from ..worktree import git_hooks_dir

    hooks_dir = git_hooks_dir(repo_root) if (repo_root / ".git").exists() else None
    if hooks_dir is None:
        logger.warning(
            "No git hooks directory found for %s — skipping git hook install.", repo_root
        )
        return None

    hooks_dir.mkdir(parents=True, exist_ok=True)
    pre_commit_path = hooks_dir / "pre-commit"
    post_commit_path = hooks_dir / "post-commit"

    _install_git_hook_script(pre_commit_path, pre_commit_script, pre_marker)
    _install_git_hook_script(post_commit_path, post_commit_script, post_marker)

    logger.info("Wrote git pre-commit hook: %s", pre_commit_path)
    logger.info("Wrote git post-commit hook: %s", post_commit_path)
    return pre_commit_path


def install_hooks(
    repo_root: Path,
    platform: str = "claude",
    extra_update_args: list[str] | None = None,
) -> Path:
    """Write hooks config to platform-specific settings.json.

    Merges new hook entries into existing settings, preserving both
    non-hook configuration and user-defined hooks.  A backup of the
    original file is created before any modifications.

    Args:
        repo_root: Repository root directory.
        platform: Target platform ("claude" or "qoder"). Claude hooks are
            written to the global user settings; Qoder hooks remain project-local.
        extra_update_args: Additional CLI args appended to the hook's
            ``dagayn update`` command.
    """
    platform = normalize_platform_target(platform)

    if platform == "qoder":
        settings_dir = repo_root / ".qoder"
    else:
        settings_dir = Path.home() / ".claude"
    settings_dir.mkdir(parents=True, exist_ok=True)
    settings_path = settings_dir / "settings.json"

    existing: SkillPayload = {}
    if settings_path.exists():
        try:
            existing = json.loads(settings_path.read_text(encoding="utf-8", errors="replace"))
            backup_path = settings_dir / "settings.json.bak"
            shutil.copy2(settings_path, backup_path)
            logger.info("Backed up existing settings to %s", backup_path)
        except (json.JSONDecodeError, OSError) as exc:
            logger.warning("Could not read existing %s: %s", settings_path, exc)

    hooks_config = generate_hooks_config(repo_root, extra_update_args=extra_update_args)
    existing_hooks = existing.get("hooks", {})
    if not isinstance(existing_hooks, dict):
        logger.warning("Existing hooks config is not a dict; replacing with defaults")
        existing_hooks = {}

    existing["hooks"] = _merge_dagayn_hook_entries(existing_hooks, hooks_config)

    write_text_atomic(settings_path, json.dumps(existing, indent=2) + "\n", encoding="utf-8")
    logger.info("Wrote hooks config: %s", settings_path)
    return settings_path
