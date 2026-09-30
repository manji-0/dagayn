"""Claude Code skills and hooks auto-install.

Generates Claude Code agent skill files, hooks configuration, and
CLAUDE.md integration for seamless dagayn usage.
Also supports multi-platform MCP server installation and
Cursor hooks / OpenCode plugin generation.
"""

from __future__ import annotations

import json
import logging
import os
import platform
import re
import shlex
import shutil
import sys
from pathlib import Path
from typing import Any

import yaml

from ..atomic_write import write_text_atomic
from ..hook_guard import DEFAULT_HOOK_BUDGET_SECONDS

logger = logging.getLogger(__name__)

type SkillValue = Any
type SkillPayload = dict[str, SkillValue]

# Slightly above the budget ``dagayn update`` applies to itself for hook runs
# (:data:`dagayn.hook_guard.DEFAULT_HOOK_BUDGET_SECONDS`), so the process stops
# itself with a diagnostic instead of the editor abandoning it. An abandoned
# hook shell leaves the dagayn child reparented to PID 1 and still running.
_UPDATE_HOOK_TIMEOUT_SECONDS = DEFAULT_HOOK_BUDGET_SECONDS + 30
_STATUS_HOOK_TIMEOUT_SECONDS = 60
_SESSION_PREPARE_BUDGET_SECONDS = 45


def _embedding_hook_args(extra_update_args: list[str] | None) -> str:
    """Shell fragment carrying the install's embedding flags, or ``""``.

    Only ``session prepare`` gets these. Passing them to the edit-triggered
    ``dagayn update`` re-embeds on every single file edit, which is far too
    expensive to sit in that path — embeddings are refreshed at session start
    and by explicit ``dagayn update --local-embedding`` / ``embed_graph_tool``
    runs instead.

    ``--keep-local-embedding-server`` is appended so a sidecar started by the
    session-start prepare stays warm: the next embedding pass (MCP
    ``ensure_graph_tool``, a manual ``dagayn update --local-embedding``) reuses
    it via the port probe instead of paying the model-load cost again.
    """
    if not extra_update_args:
        return ""
    args = [*extra_update_args, "--keep-local-embedding-server"]
    return " " + " ".join(shlex.quote(arg) for arg in args)


# --- Multi-platform MCP install ---


def _zed_settings_path() -> Path:
    """Return the Zed settings.json path for the current OS."""
    if platform.system() == "Darwin":
        return Path.home() / "Library" / "Application Support" / "Zed" / "settings.json"
    return Path.home() / ".config" / "zed" / "settings.json"


PLATFORMS: dict[str, SkillPayload] = {
    "codex": {
        "name": "Codex",
        "config_path": lambda root: Path.home() / ".codex" / "config.toml",
        "key": "mcp_servers",
        "detect": lambda: (Path.home() / ".codex").exists(),
        "format": "toml",
        "needs_type": True,
    },
    "claude": {
        "name": "Claude Code",
        "config_path": lambda root: root / ".mcp.json",
        "key": "mcpServers",
        "detect": lambda: True,
        "format": "object",
        "needs_type": True,
    },
    "cursor": {
        "name": "Cursor",
        "config_path": lambda root: root / ".cursor" / "mcp.json",
        "key": "mcpServers",
        "detect": lambda: (Path.home() / ".cursor").exists(),
        "format": "object",
        "needs_type": True,
    },
    "windsurf": {
        "name": "Windsurf",
        "config_path": lambda root: Path.home() / ".codeium" / "windsurf" / "mcp_config.json",
        "key": "mcpServers",
        "detect": lambda: (Path.home() / ".codeium" / "windsurf").exists(),
        "format": "object",
        "needs_type": False,
    },
    "zed": {
        "name": "Zed",
        "config_path": lambda root: _zed_settings_path(),
        "key": "context_servers",
        "detect": lambda: _zed_settings_path().parent.exists(),
        "format": "object",
        "needs_type": False,
    },
    "continue": {
        "name": "Continue",
        "config_path": lambda root: Path.home() / ".continue" / "config.json",
        "key": "mcpServers",
        "detect": lambda: (Path.home() / ".continue").exists(),
        "format": "array",
        "needs_type": True,
    },
    "opencode": {
        "name": "OpenCode",
        "config_path": lambda root: root / ".opencode.json",
        "key": "mcpServers",
        "detect": lambda: True,
        "format": "object",
        "needs_type": True,
    },
    "antigravity": {
        "name": "Antigravity",
        "config_path": lambda root: Path.home() / ".gemini" / "antigravity" / "mcp_config.json",
        "key": "mcpServers",
        "detect": lambda: (Path.home() / ".gemini" / "antigravity").exists(),
        "format": "object",
        "needs_type": False,
    },
    "qwen": {
        "name": "Qwen Code",
        "config_path": lambda root: Path.home() / ".qwen" / "settings.json",
        "key": "mcpServers",
        "detect": lambda: (Path.home() / ".qwen").exists(),
        "format": "object",
        "needs_type": True,
    },
    "kiro": {
        "name": "Kiro",
        "config_path": lambda root: root / ".kiro" / "settings" / "mcp.json",
        "key": "mcpServers",
        "detect": lambda: (Path.home() / ".kiro").exists(),
        "format": "object",
        "needs_type": True,
    },
    "qoder": {
        "name": "Qoder",
        "config_path": lambda root: root / ".qoder" / "mcp.json",
        "key": "mcpServers",
        "detect": lambda: True,
        "format": "object",
        "needs_type": True,
    },
    "pi": {
        "name": "Pi",
        "config_path": lambda root: root / ".pi" / "mcp.json",
        "key": "mcpServers",
        "detect": lambda: (Path.home() / ".pi").exists(),
        "format": "object",
        "needs_type": False,
    },
    "hermes": {
        "name": "Hermes Agent",
        "config_path": lambda root: Path.home() / ".hermes" / "config.yaml",
        "key": "mcp_servers",
        "detect": lambda: (Path.home() / ".hermes").exists(),
        "format": "yaml",
        "needs_type": False,
    },
}

_PLATFORM_ALIASES = {
    "claude-code": "claude",
    "qcoder": "qoder",
}


def normalize_platform_target(target: str) -> str:
    """Return the canonical platform key for CLI/config aliases."""
    return _PLATFORM_ALIASES.get(target, target)


def _in_poetry_project() -> bool:
    """Return True when the running interpreter is a Poetry-managed virtualenv.

    Two signals are checked so that **both** ``poetry shell`` and ``poetry run``
    are detected:

    * ``POETRY_ACTIVE=1`` — set by ``poetry shell`` when the user activates the
      virtual environment interactively.
    * ``VIRTUAL_ENV`` containing ``"pypoetry"`` — set by **both** ``poetry shell``
      and ``poetry run`` because Poetry stores its virtualenvs under a path that
      includes the string ``pypoetry`` (e.g.
      ``~/.cache/pypoetry/virtualenvs/<name>`` on Linux/macOS or
      ``%LOCALAPPDATA%\\pypoetry\\Cache\\virtualenvs\\<name>`` on Windows).

    Checking only ``POETRY_ACTIVE`` would miss the ``poetry run`` case, which is
    the primary scenario described in issue #256.
    """
    if os.environ.get("POETRY_ACTIVE") == "1":
        return True
    virtual_env = os.environ.get("VIRTUAL_ENV", "")
    return bool(virtual_env) and "pypoetry" in virtual_env.lower()


def _in_uv_project() -> bool:
    """Return True if ``sys.executable`` lives inside a uv-managed project.

    A project is considered uv-managed when a ``uv.lock`` file exists in any
    ancestor directory of the running Python interpreter (stopping at the home
    directory to avoid false positives on system-wide installations).
    """
    exe = Path(sys.executable).resolve()
    home = Path.home()
    for parent in exe.parents:
        if (parent / "uv.lock").exists():
            return True
        # Stop searching once we reach the home directory or filesystem root
        if parent == home or parent == parent.parent:
            break
    return False


def _detect_serve_command() -> tuple[str, list[str]]:
    """Return ``(command, args)`` that correctly launches ``dagayn serve``.

    Detection priority
    ------------------
    1. **Poetry** – ``POETRY_ACTIVE=1`` OR ``VIRTUAL_ENV`` contains ``"pypoetry"``
       (covers both ``poetry shell`` and ``poetry run``) and ``poetry`` is on PATH
       → ``poetry run dagayn serve``
    2. **uv project** – ``UV_PROJECT_ENVIRONMENT`` is set, or a ``uv.lock``
       ancestor is found alongside ``sys.executable``, and ``uv`` is on PATH
       → ``uv run dagayn serve``
    3. **Installed CLI** – ``dagayn`` is available on PATH
       → ``dagayn serve``
    4. **uvx** – ``uvx`` is available on PATH
       → ``uvx dagayn serve``
    5. **Fallback** – use the absolute path of the running Python interpreter
       → ``sys.executable -m dagayn serve``

    The fallback is always safe: ``sys.executable`` is the exact interpreter
    that is currently running, so it resolves correctly inside any virtual
    environment, conda env, or system installation.
    """
    # 1. Poetry (poetry shell or poetry run)
    if _in_poetry_project():
        poetry = shutil.which("poetry")
        if poetry:
            return ("poetry", ["run", "dagayn", "serve"])

    # 2. uv managed project environment
    if os.environ.get("UV_PROJECT_ENVIRONMENT") or _in_uv_project():
        uv = shutil.which("uv")
        if uv:
            return ("uv", ["run", "dagayn", "serve"])

    # 3. Globally installed CLI (for ``uv tool install dagayn`` or equivalent)
    if shutil.which("dagayn"):
        return ("dagayn", ["serve"])

    # 4. uvx global tool runner
    if shutil.which("uvx"):
        return ("uvx", ["dagayn", "serve"])

    # 5. Absolute-path fallback using the running interpreter
    return (sys.executable, ["-m", "dagayn", "serve"])


def _build_server_entry(
    plat: SkillPayload,
    key: str = "",
    extra_serve_args: list[str] | None = None,
) -> SkillPayload:
    """Build the MCP server entry for a platform."""
    command, args = _detect_serve_command()
    if extra_serve_args:
        args = args + extra_serve_args
    entry: SkillPayload = {"command": command, "args": args}
    if plat["needs_type"]:
        entry["type"] = "stdio"
    # Cursor launches user-level MCP with cwd=$HOME. ${workspaceFolder} in
    # ~/.cursor/mcp.json is the folder containing that file, not the open
    # project, so the user-level copy must not pin cwd/--repo. Project-level
    # `.cursor/mcp.json` does: that process starts in the repository.
    # `_sync_cursor_user_mcp` strips cwd/--repo from the user copy. Unpinned
    # `dagayn serve` resolves the repo on each tool call from workspace hints.
    if key == "cursor":
        entry["cwd"] = "${workspaceFolder}"
    if key == "opencode":
        entry["env"] = []
    if key == "pi":
        entry["transport"] = "stdio"
        entry["lifecycle"] = "lazy"
    return entry


def _format_toml_value(value: Any) -> str:
    """Format a primitive Python value as TOML."""
    if isinstance(value, str):
        escaped = value.replace("\\", "\\\\").replace('"', '\\"')
        return f'"{escaped}"'
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, list):
        return "[" + ", ".join(_format_toml_value(item) for item in value) + "]"
    raise TypeError(f"Unsupported TOML value: {type(value)!r}")


def _merge_toml_mcp_server(
    config_path: Path,
    server_name: str,
    server_entry: SkillPayload,
    dry_run: bool = False,
) -> bool:
    """Upsert a Codex MCP server section without clobbering the rest of the file."""
    section_header = f"[mcp_servers.{server_name}]"
    existing = ""
    if config_path.exists():
        existing = config_path.read_text(encoding="utf-8")

    section_lines = [section_header]
    for key, value in server_entry.items():
        section_lines.append(f"{key} = {_format_toml_value(value)}")
    section = "\n".join(section_lines) + "\n"

    if section_header in existing:
        pattern = re.compile(
            rf"(?ms)^{re.escape(section_header)}\n.*?(?=^\[|\Z)",
        )
        updated = pattern.sub(section, existing, count=1)
        if updated == existing:
            return False
        if not dry_run:
            write_text_atomic(config_path, updated, encoding="utf-8")
        return True

    if dry_run:
        return True

    config_path.parent.mkdir(parents=True, exist_ok=True)
    prefix = ""
    if existing:
        prefix = existing if existing.endswith("\n") else existing + "\n"
        if not prefix.endswith("\n\n"):
            prefix += "\n"
    write_text_atomic(config_path, prefix + section, encoding="utf-8")
    return True


def _merge_yaml_mcp_server(
    config_path: Path,
    servers_key: str,
    server_name: str,
    server_entry: SkillPayload,
    dry_run: bool = False,
) -> bool:
    """Upsert an MCP server entry in a YAML mapping."""
    existing: SkillPayload = {}
    if config_path.exists():
        try:
            loaded = yaml.safe_load(config_path.read_text(encoding="utf-8", errors="replace"))
            if isinstance(loaded, dict):
                existing = loaded
            elif loaded is not None:
                logger.warning("Invalid YAML shape in %s, will overwrite.", config_path)
        except (yaml.YAMLError, OSError):
            logger.warning("Invalid YAML in %s, will overwrite.", config_path)

    servers = existing.get(servers_key, {})
    if not isinstance(servers, dict):
        servers = {}

    current = servers.get(server_name, {})
    if not isinstance(current, dict):
        current = {}
    updated_entry = {**current, **server_entry}
    if updated_entry == current:
        return False

    servers[server_name] = updated_entry
    existing[servers_key] = servers

    if dry_run:
        return True

    config_path.parent.mkdir(parents=True, exist_ok=True)
    write_text_atomic(config_path, yaml.safe_dump(existing, sort_keys=False), encoding="utf-8")
    return True


def _merge_hermes_hook_entries(
    existing_hooks: SkillPayload,
    hooks_config: SkillPayload,
) -> SkillPayload:
    """Merge Hermes hook config, replacing dagayn-managed commands."""
    merged_hooks = dict(existing_hooks)
    for hook_name, hook_entries in hooks_config.items():
        if not isinstance(hook_entries, list):
            continue
        existing_entries = merged_hooks.get(hook_name, [])
        if not isinstance(existing_entries, list):
            existing_entries = []
        kept_entries = [
            entry
            for entry in existing_entries
            if not (
                isinstance(entry, dict)
                and "dagayn-" in str(entry.get("command", ""))
                and str(entry.get("command", "")).endswith(".sh")
            )
        ]
        merged_hooks[hook_name] = kept_entries + hook_entries
    return merged_hooks


def _merge_pi_hook_entries(
    existing_hooks: list[object],
    hooks_config: list[SkillPayload],
) -> list[object]:
    """Merge pi-yaml-hooks entries, replacing dagayn-managed bash actions."""
    kept_entries: list[object] = []
    for entry in existing_hooks:
        if not isinstance(entry, dict):
            kept_entries.append(entry)
            continue
        actions = entry.get("actions", [])
        if not isinstance(actions, list):
            kept_entries.append(entry)
            continue
        if any(
            isinstance(action, dict) and "dagayn-" in str(action.get("bash", ""))
            for action in actions
        ):
            continue
        kept_entries.append(entry)
    return kept_entries + hooks_config


def install_platform_configs(
    repo_root: Path,
    target: str = "all",
    dry_run: bool = False,
    extra_serve_args: list[str] | None = None,
) -> list[str]:
    """Install MCP config for one or all detected platforms.

    Args:
        repo_root: Project root directory.
        target: Platform key or "all".
        dry_run: If True, print what would be done without writing.
        extra_serve_args: Additional CLI args appended to the ``dagayn serve``
            command written into MCP config files (e.g.
            ``["--local-embedding", "low"]``).

    Returns:
        List of platform names that were configured.
    """
    target = normalize_platform_target(target)

    if target == "all":
        platforms_to_install = {k: v for k, v in PLATFORMS.items() if v["detect"]()}
        # Workspace-level Kiro detection
        if "kiro" not in platforms_to_install and (repo_root / ".kiro").is_dir():
            platforms_to_install["kiro"] = PLATFORMS["kiro"]
        if "pi" not in platforms_to_install and (repo_root / ".pi").is_dir():
            platforms_to_install["pi"] = PLATFORMS["pi"]
    else:
        if target not in PLATFORMS:
            logger.error("Unknown platform: %s", target)
            return []
        platforms_to_install = {target: PLATFORMS[target]}

    configured: list[str] = []

    for key, plat in platforms_to_install.items():
        config_path: Path = plat["config_path"](repo_root)
        server_key = plat["key"]
        server_entry = _build_server_entry(plat, key=key, extra_serve_args=extra_serve_args)

        if plat["format"] == "toml":
            changed = _merge_toml_mcp_server(
                config_path,
                "dagayn",
                server_entry,
                dry_run=dry_run,
            )
            if not changed:
                print(f"  {plat['name']}: already configured in {config_path}")
                configured.append(plat["name"])
                continue
            if dry_run:
                print(f"  [dry-run] {plat['name']}: would write {config_path}")
            else:
                print(f"  {plat['name']}: configured {config_path}")
            configured.append(plat["name"])
            continue

        if plat["format"] == "yaml":
            changed = _merge_yaml_mcp_server(
                config_path,
                server_key,
                "dagayn",
                server_entry,
                dry_run=dry_run,
            )
            if not changed:
                print(f"  {plat['name']}: already configured in {config_path}")
                configured.append(plat["name"])
                continue
            if dry_run:
                print(f"  [dry-run] {plat['name']}: would write {config_path}")
            else:
                print(f"  {plat['name']}: configured {config_path}")
            configured.append(plat["name"])
            continue

        # Read existing config
        existing: SkillPayload = {}
        if config_path.exists():
            try:
                existing = json.loads(config_path.read_text(encoding="utf-8", errors="replace"))
            except (json.JSONDecodeError, OSError):
                logger.warning("Invalid JSON in %s, will overwrite.", config_path)
                existing = {}

        if plat["format"] == "array":
            arr = existing.get(server_key, [])
            if not isinstance(arr, list):
                arr = []
            arr_entry = {"name": "dagayn", **server_entry}
            changed = False
            for index, item in enumerate(arr):
                if isinstance(item, dict) and item.get("name") == "dagayn":
                    updated_item = {**item, **arr_entry}
                    if updated_item != item:
                        arr[index] = updated_item
                        changed = True
                    break
            else:
                arr.append(arr_entry)
                changed = True
            if not changed:
                print(f"  {plat['name']}: already configured in {config_path}")
                configured.append(plat["name"])
                continue
            existing[server_key] = arr
        else:
            servers = existing.get(server_key, {})
            if not isinstance(servers, dict):
                servers = {}
            if "dagayn" in servers:
                current = servers["dagayn"]
                if not isinstance(current, dict):
                    current = {}
                updated_entry = {**current, **server_entry}
                if updated_entry == current:
                    print(f"  {plat['name']}: already configured in {config_path}")
                    configured.append(plat["name"])
                    if key == "cursor":
                        _sync_cursor_user_mcp(server_entry, dry_run=dry_run)
                    continue
                servers["dagayn"] = updated_entry
            else:
                servers["dagayn"] = server_entry
            existing[server_key] = servers

        if dry_run:
            print(f"  [dry-run] {plat['name']}: would write {config_path}")
        else:
            config_path.parent.mkdir(parents=True, exist_ok=True)
            write_text_atomic(config_path, json.dumps(existing, indent=2) + "\n", encoding="utf-8")
            print(f"  {plat['name']}: configured {config_path}")

        configured.append(plat["name"])

        # Cursor prefers user-scoped MCP (shown as ``user-dagayn``). Keep
        # ~/.cursor/mcp.json aligned with the same workspace-relative entry so
        # a shared global config cannot pin serve to one absolute repo path.
        if key == "cursor":
            _sync_cursor_user_mcp(server_entry, dry_run=dry_run)

    return configured


def _sync_cursor_user_mcp(server_entry: SkillPayload, *, dry_run: bool) -> None:
    """Merge the Cursor MCP server entry into ``~/.cursor/mcp.json``."""
    config_path = Path.home() / ".cursor" / "mcp.json"
    existing: SkillPayload = {}
    if config_path.exists():
        try:
            existing = json.loads(config_path.read_text(encoding="utf-8", errors="replace"))
        except (json.JSONDecodeError, OSError):
            logger.warning("Invalid JSON in %s, will overwrite dagayn entry.", config_path)
            existing = {}

    servers = existing.get("mcpServers", {})
    if not isinstance(servers, dict):
        servers = {}
    current = servers.get("dagayn")
    if not isinstance(current, dict):
        current = {}
    updated = {**current, **server_entry}
    # Drop stale absolute-path / placeholder pins that defeat shared global MCP.
    updated.pop("cwd", None)
    env = updated.get("env")
    if isinstance(env, dict):
        env = {
            key: value for key, value in env.items() if key not in {"DAGAYN_REPO", "CRG_REPO_ROOT"}
        }
        if env:
            updated["env"] = env
        else:
            updated.pop("env", None)
    args = updated.get("args")
    if isinstance(args, list):
        cleaned: list[object] = []
        skip_next = False
        for item in args:
            if skip_next:
                skip_next = False
                continue
            if item == "--repo":
                skip_next = True
                continue
            cleaned.append(item)
        updated["args"] = cleaned

    if updated == current and "dagayn" in servers:
        print(f"  Cursor (user): already configured in {config_path}")
        return
    if dry_run:
        print(f"  [dry-run] Cursor (user): would write {config_path}")
        return

    servers["dagayn"] = updated
    existing["mcpServers"] = servers
    config_path.parent.mkdir(parents=True, exist_ok=True)
    write_text_atomic(config_path, json.dumps(existing, indent=2) + "\n", encoding="utf-8")
    print(f"  Cursor (user): configured {config_path}")
