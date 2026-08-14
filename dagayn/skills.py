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
import stat
import sys
from pathlib import Path
from typing import Any

import yaml

logger = logging.getLogger(__name__)

_UPDATE_HOOK_TIMEOUT_SECONDS = 300
_STATUS_HOOK_TIMEOUT_SECONDS = 60
_SESSION_PREPARE_BUDGET_SECONDS = 45

# --- Multi-platform MCP install ---


def _zed_settings_path() -> Path:
    """Return the Zed settings.json path for the current OS."""
    if platform.system() == "Darwin":
        return Path.home() / "Library" / "Application Support" / "Zed" / "settings.json"
    return Path.home() / ".config" / "zed" / "settings.json"


PLATFORMS: dict[str, dict[str, Any]] = {
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
    plat: dict[str, Any],
    key: str = "",
    extra_serve_args: list[str] | None = None,
) -> dict[str, Any]:
    """Build the MCP server entry for a platform."""
    command, args = _detect_serve_command()
    if extra_serve_args:
        args = args + extra_serve_args
    entry: dict[str, Any] = {"command": command, "args": args}
    if plat["needs_type"]:
        entry["type"] = "stdio"
    # Cursor launches user-level MCP with cwd=$HOME and does not reliably
    # expand ${workspaceFolder} in ~/.cursor/mcp.json. Do not pin --repo to a
    # template or absolute path; dagayn serve resolves the open workspace from
    # WORKSPACE_FOLDER_PATHS (and related IDE env) at startup.
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
    server_entry: dict[str, Any],
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
            config_path.write_text(updated, encoding="utf-8")
        return True

    if dry_run:
        return True

    config_path.parent.mkdir(parents=True, exist_ok=True)
    prefix = ""
    if existing:
        prefix = existing if existing.endswith("\n") else existing + "\n"
        if not prefix.endswith("\n\n"):
            prefix += "\n"
    config_path.write_text(prefix + section, encoding="utf-8")
    return True


def _merge_yaml_mcp_server(
    config_path: Path,
    servers_key: str,
    server_name: str,
    server_entry: dict[str, Any],
    dry_run: bool = False,
) -> bool:
    """Upsert an MCP server entry in a YAML mapping."""
    existing: dict[str, Any] = {}
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
    config_path.write_text(yaml.safe_dump(existing, sort_keys=False), encoding="utf-8")
    return True


def _merge_hermes_hook_entries(
    existing_hooks: dict[str, Any],
    hooks_config: dict[str, Any],
) -> dict[str, Any]:
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
    existing_hooks: list[Any],
    hooks_config: list[dict[str, Any]],
) -> list[Any]:
    """Merge pi-yaml-hooks entries, replacing dagayn-managed bash actions."""
    kept_entries = []
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
        existing: dict[str, Any] = {}
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
            config_path.write_text(json.dumps(existing, indent=2) + "\n", encoding="utf-8")
            print(f"  {plat['name']}: configured {config_path}")

        configured.append(plat["name"])

        # Cursor prefers user-scoped MCP (shown as ``user-dagayn``). Keep
        # ~/.cursor/mcp.json aligned with the same workspace-relative entry so
        # a shared global config cannot pin serve to one absolute repo path.
        if key == "cursor":
            _sync_cursor_user_mcp(server_entry, dry_run=dry_run)

    return configured


def _sync_cursor_user_mcp(server_entry: dict[str, Any], *, dry_run: bool) -> None:
    """Merge the Cursor MCP server entry into ``~/.cursor/mcp.json``."""
    config_path = Path.home() / ".cursor" / "mcp.json"
    existing: dict[str, Any] = {}
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
        cleaned: list[Any] = []
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
    config_path.write_text(json.dumps(existing, indent=2) + "\n", encoding="utf-8")
    print(f"  Cursor (user): configured {config_path}")


# --- Skill file contents ---


_SKILL_EMBEDDING_CONTEXT_START = "<!-- dagayn skill embedding context -->"
_SKILL_EMBEDDING_CONTEXT_END = "<!-- /dagayn skill embedding context -->"


def _resolve_source_skills_dir() -> Path | None:
    """Locate the on-disk ``skills/`` directory shipped with dagayn.

    Tries the wheel-install layout first (``<site-packages>/dagayn/skills``),
    then falls back to the development checkout layout (``<repo>/skills``).
    Returns ``None`` if no directory containing ``<name>/SKILL.md`` files is
    found.

    The wheel-first order avoids accidentally picking up a stale or unrelated
    ``skills/`` directory that may exist at the site-packages root
    (``parent.parent / "skills"``) when multiple packages are installed.
    """
    candidates = [
        Path(__file__).resolve().parent / "skills",
        Path(__file__).resolve().parent.parent / "skills",
    ]
    for candidate in candidates:
        if candidate.is_dir() and any(
            (entry / "SKILL.md").is_file() for entry in candidate.iterdir() if entry.is_dir()
        ):
            return candidate
    return None


def _embedding_context_lines(
    embedding_mode: str | None = None,
    embedding_preset: str | None = None,
    embedding_provider: str | None = None,
) -> list[str]:
    """Return install-specific search guidance for generated skills."""
    if embedding_mode == "local-embedding":
        return [
            "## Installed Search Mode",
            "",
            "Installed with local embeddings (`--mode local-embedding`): managed "
            "BGE-M3 llama.cpp sidecar.",
            "",
            "- MCP search defaults to hybrid retrieval when matching embeddings exist.",
            "- Read `search_mode`, `rerank_intent`, and per-result `source` before "
            "judging search quality.",
            "- Routine graph refreshes for parser, flow, documentation, or review "
            'verification should pass `local_embedding="none"` so they do not '
            "inherit the server embedding mode and trigger a large embedding refresh.",
            "- Use embedding-enabled full rebuilds only for explicit embedding-quality "
            "or end-to-end maintenance work after stating the reason.",
            "- Exact identifier lookup can still rely on FTS; use semantic search for "
            "fuzzy concepts, domain terms, cross-language search, or unfamiliar code. "
            "Process-pattern prose should use narrative embeddings when available.",
        ]
    if embedding_mode == "local-embedding-llama":
        preset = embedding_preset or "low"
        return [
            "## Installed Search Mode",
            "",
            "Installed with managed Qwen3 embeddings "
            f"(`--mode local-embedding-llama --preset {preset}`).",
            "",
            "- MCP search defaults to hybrid retrieval when matching embeddings exist.",
            "- Read `search_mode`, `rerank_intent`, and per-result `source` before "
            "judging search quality.",
            "- Routine graph refreshes for parser, flow, documentation, or review "
            'verification should pass `local_embedding="none"` so they do not '
            "inherit the server sidecar mode and trigger a large embedding refresh.",
            "- Use embedding-enabled full rebuilds only for explicit embedding-quality "
            "or end-to-end maintenance work after stating the reason.",
            "- Exact identifier lookup can still rely on FTS; use semantic search for "
            "fuzzy concepts, domain terms, cross-language search, or unfamiliar code. "
            "Process-pattern prose should use narrative embeddings when available.",
        ]
    if embedding_mode == "remote-embedding":
        provider = embedding_provider or "openai"
        return [
            "## Installed Search Mode",
            "",
            f"Installed with remote embeddings (`--mode remote-embedding --provider {provider}`).",
            "",
            "- MCP search defaults to the configured provider when matching embeddings exist.",
            "- Read `search_mode`, `rerank_intent`, and per-result `source` before "
            "judging search quality.",
            "- `build_or_update_graph_tool()` refreshes graph and FTS data; run "
            f'`embed_graph_tool(provider="{provider}")` after graph refresh when hybrid '
            "search is required.",
            "- Use FTS for exact lookup and reserve remote embedding calls for fuzzy, "
            "cross-repo, or conceptual searches.",
        ]
    if embedding_mode == "fts-only":
        return [
            "## Installed Search Mode",
            "",
            "Installed in FTS-only mode (`--mode fts-only`).",
            "",
            "- Treat `semantic_search_nodes_tool` as keyword/FTS search, not vector "
            "semantic search.",
            "- `search_mode` should normally be `fts_only`; `keyword_fallback` means "
            "the FTS index is absent and should be refreshed before quality claims.",
            "- Prefer exact symbols, file names, graph relationships, and one targeted "
            "`rg` for literals.",
            "- Do not rebuild embeddings unless the user explicitly changes install mode.",
        ]
    return [
        "## Installed Search Mode",
        "",
        "This packaged skill is mode-neutral. `dagayn install` rewrites this section with",
        "the selected embedding mode so agents can avoid stale or wasteful search advice.",
        "Without that install context, inspect MCP serve args or `semantic_search_nodes_tool`",
        "`search_mode` before assuming hybrid search is available.",
    ]


def _render_skill_content(
    content: str,
    *,
    embedding_mode: str | None = None,
    embedding_preset: str | None = None,
    embedding_provider: str | None = None,
) -> str:
    """Render install-time context inside a packaged skill if it opts in."""
    start_index = content.find(_SKILL_EMBEDDING_CONTEXT_START)
    if start_index < 0:
        return content
    end_index = content.find(_SKILL_EMBEDDING_CONTEXT_END, start_index)
    if end_index < 0:
        return content

    context = "\n".join(
        _embedding_context_lines(
            embedding_mode=embedding_mode,
            embedding_preset=embedding_preset,
            embedding_provider=embedding_provider,
        )
    )
    replacement = f"{_SKILL_EMBEDDING_CONTEXT_START}\n{context}\n{_SKILL_EMBEDDING_CONTEXT_END}"
    return (
        content[:start_index]
        + replacement
        + content[end_index + len(_SKILL_EMBEDDING_CONTEXT_END) :]
    )


def generate_skills(
    repo_root: Path,
    skills_dir: Path | None = None,
    *,
    embedding_mode: str | None = None,
    embedding_preset: str | None = None,
    embedding_provider: str | None = None,
) -> Path:
    """Generate Claude Code skill files.

    Reads ``skills/<name>/SKILL.md`` from the dagayn package and writes
    each one as ``<skills_dir>/<name>.md`` (Claude Code's flat layout).

    Args:
        repo_root: Repository root directory.
        skills_dir: Custom skills directory. Defaults to repo_root/.claude/skills.
        embedding_mode: Optional install mode (``fts-only``,
            ``local-embedding``, ``local-embedding-llama``, or
            ``remote-embedding``) used to render search guidance in skills that
            opt in.
        embedding_preset: Local sidecar preset when ``embedding_mode`` is
            ``local-embedding-llama``.
        embedding_provider: Remote embedding provider when ``embedding_mode``
            is ``remote-embedding``.

    Returns:
        Path to the skills directory.
    """
    if skills_dir is None:
        skills_dir = repo_root / ".claude" / "skills"
    skills_dir.mkdir(parents=True, exist_ok=True)

    source_dir = _resolve_source_skills_dir()
    if source_dir is None:
        logger.warning("No skills/ directory found alongside dagayn; nothing installed.")
        return skills_dir

    for entry in sorted(source_dir.iterdir()):
        if not entry.is_dir():
            continue
        skill_file = entry / "SKILL.md"
        if not skill_file.is_file():
            continue
        target = skills_dir / f"{entry.name}.md"
        content = _render_skill_content(
            skill_file.read_text(encoding="utf-8"),
            embedding_mode=embedding_mode,
            embedding_preset=embedding_preset,
            embedding_provider=embedding_provider,
        )
        target.write_text(content, encoding="utf-8")
        logger.info("Wrote skill: %s", target)

    return skills_dir


def _install_skill_tree(
    target_dir: Path,
    *,
    embedding_mode: str | None = None,
    embedding_preset: str | None = None,
    embedding_provider: str | None = None,
) -> Path:
    """Install packaged skills as ``<name>/SKILL.md`` directories.

    Each dagayn-managed skill directory is replaced from source on every run
    so stale ``SKILL.md`` content or removed auxiliary files do not linger
    after upgrading dagayn. Unrelated user-created skills in the same root are
    left untouched.
    """
    target_dir.mkdir(parents=True, exist_ok=True)

    source_dir = _resolve_source_skills_dir()
    if source_dir is None:
        logger.warning("No skills/ directory found alongside dagayn; nothing installed.")
        return target_dir

    for entry in sorted(source_dir.iterdir()):
        if not entry.is_dir() or not (entry / "SKILL.md").is_file():
            continue
        destination = target_dir / entry.name
        if destination.exists():
            shutil.rmtree(destination)
        shutil.copytree(entry, destination)
        target_skill = destination / "SKILL.md"
        target_skill.write_text(
            _render_skill_content(
                target_skill.read_text(encoding="utf-8"),
                embedding_mode=embedding_mode,
                embedding_preset=embedding_preset,
                embedding_provider=embedding_provider,
            ),
            encoding="utf-8",
        )
        logger.info("Wrote skill directory: %s", destination)

    return target_dir


def install_global_skills(
    *,
    embedding_mode: str | None = None,
    embedding_preset: str | None = None,
    embedding_provider: str | None = None,
) -> Path:
    """Install Claude Code skills into ``~/.claude/skills/``.

    Mirrors the source ``skills/`` tree as flat ``<name>.md`` files under
    the user home so the writing/reading-markdown-document skills (and the
    other dagayn skills) are available across all projects.
    """
    target = Path.home() / ".claude" / "skills"
    return generate_skills(
        repo_root=Path.home(),
        skills_dir=target,
        embedding_mode=embedding_mode,
        embedding_preset=embedding_preset,
        embedding_provider=embedding_provider,
    )


def install_codex_skills(
    *,
    embedding_mode: str | None = None,
    embedding_preset: str | None = None,
    embedding_provider: str | None = None,
) -> Path:
    """Install dagayn skills into Codex's global user skills directory."""
    return _install_skill_tree(
        Path.home() / ".codex" / "skills",
        embedding_mode=embedding_mode,
        embedding_preset=embedding_preset,
        embedding_provider=embedding_provider,
    )


def install_opencode_skills(
    *,
    embedding_mode: str | None = None,
    embedding_preset: str | None = None,
    embedding_provider: str | None = None,
) -> Path:
    """Install dagayn skills into OpenCode's global user skills directory."""
    return _install_skill_tree(
        Path.home() / ".config" / "opencode" / "skills",
        embedding_mode=embedding_mode,
        embedding_preset=embedding_preset,
        embedding_provider=embedding_provider,
    )


def install_pi_skills(
    *,
    embedding_mode: str | None = None,
    embedding_preset: str | None = None,
    embedding_provider: str | None = None,
) -> Path:
    """Install dagayn skills into Pi's global user skills directory."""
    return _install_skill_tree(
        Path.home() / ".pi" / "agent" / "skills",
        embedding_mode=embedding_mode,
        embedding_preset=embedding_preset,
        embedding_provider=embedding_provider,
    )


def install_hermes_skills(
    *,
    embedding_mode: str | None = None,
    embedding_preset: str | None = None,
    embedding_provider: str | None = None,
) -> Path:
    """Install dagayn skills into Hermes Agent's global user skills directory."""
    return _install_skill_tree(
        Path.home() / ".hermes" / "skills",
        embedding_mode=embedding_mode,
        embedding_preset=embedding_preset,
        embedding_provider=embedding_provider,
    )


def _dagayn_hook_scripts(extra_update_args: list[str] | None = None) -> dict[str, str]:
    """Return shell scripts shared by hook integrations that expect JSON stdout."""
    update_args = ""
    if extra_update_args:
        update_args = " " + " ".join(shlex.quote(arg) for arg in extra_update_args)
    return {
        "dagayn-update.sh": f"""#!/usr/bin/env bash
# dagayn: auto-update graph after agent file/tool activity
set -u
cat >/dev/null || true
repo="$(git rev-parse --show-toplevel 2>/dev/null || true)"
if [ -n "$repo" ]; then
  dagayn update --skip-flows{update_args} --repo "$repo" >/dev/null 2>&1 || true
fi
printf '{{}}\\n'
""",
        "dagayn-status.sh": f"""#!/usr/bin/env bash
# dagayn: prepare a usable+synced graph at session start
set -u
cat >/dev/null || true
repo="$(git rev-parse --show-toplevel 2>/dev/null || true)"
if [ -n "$repo" ]; then
  DAGAYN_HOOK_UPDATE=1 dagayn session prepare \\
    --budget-seconds {_SESSION_PREPARE_BUDGET_SECONDS}{update_args} \\
    --repo "$repo" >/dev/null 2>&1 || true
fi
printf '{{}}\\n'
""",
    }


def _write_hook_scripts(hooks_dir: Path, scripts: dict[str, str]) -> None:
    hooks_dir.mkdir(parents=True, exist_ok=True)
    for filename, content in scripts.items():
        script_path = hooks_dir / filename
        script_path.write_text(content, encoding="utf-8")
        script_path.chmod(stat.S_IRWXU | stat.S_IRGRP | stat.S_IXGRP | stat.S_IROTH | stat.S_IXOTH)


def generate_hermes_hooks_config() -> dict[str, Any]:
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
    existing: dict[str, Any] = {}
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
    config_path.write_text(yaml.safe_dump(existing, sort_keys=False), encoding="utf-8")
    return config_path


def generate_pi_hooks_config() -> list[dict[str, Any]]:
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
    existing: dict[str, Any] = {}
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
    hooks_path.write_text(yaml.safe_dump(existing, sort_keys=False), encoding="utf-8")
    return hooks_path


def generate_hooks_config(
    repo_root: Path,
    extra_update_args: list[str] | None = None,
    *,
    worktree_hook: bool = True,
) -> dict[str, Any]:
    """Generate Claude Code hooks configuration.

    Hooks use the v1.x+ schema: each entry needs a ``matcher`` and a nested
    ``hooks`` array. Timeouts are in seconds. ``PreCommit`` is not a valid
    Claude Code event — pre-commit checks are handled by ``install_git_hook``.

    Args:
        repo_root: Unused; hooks resolve the active repository at runtime.
        extra_update_args: Additional CLI args for the ``dagayn update`` hook.
        worktree_hook: Include the ``EnterWorktree`` / ``ExitWorktree``
            ``PostToolUse`` entry. Disable for hosts without those tools
            (Codex), where the entry would never match.
    """
    del repo_root  # Hooks are global; resolve the active repository at runtime.
    update_args = ""
    if extra_update_args:
        update_args = " " + " ".join(shlex.quote(arg) for arg in extra_update_args)
    # ``git rev-parse`` first: hooks run in the session's working directory, so
    # in a worktree session it resolves to that worktree rather than the main
    # checkout. ``CLAUDE_PROJECT_DIR`` covers a cwd outside the repository.
    repo_expr = (
        'repo="$(git rev-parse --show-toplevel 2>/dev/null)"'
        ' || repo="${CLAUDE_PROJECT_DIR:-}"; [ -n "$repo" ]'
    )
    post_tool_use: list[dict[str, Any]] = [
        {
            "matcher": "Edit|Write|Bash",
            "hooks": [
                {
                    "type": "command",
                    "command": (
                        f"{repo_expr}"
                        f" && DAGAYN_HOOK_UPDATE=1 dagayn update --skip-flows"
                        f"{update_args}"
                        ' --repo "$repo"'
                        " || true"
                    ),
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
                            f"{update_args} || true"
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
                                f"{update_args}"
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
    existing_hooks: dict[str, Any],
    hooks_config: dict[str, Any],
) -> dict[str, Any]:
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
        config_path.write_text("[features]\nhooks = true\n", encoding="utf-8")
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
        config_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return

    if codex_hooks_index is not None:
        lines[codex_hooks_index] = re.sub(
            r"codex_hooks\s*=\s*.*$", "hooks = true", lines[codex_hooks_index], count=1
        )
        config_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return

    if features_index is not None:
        lines.insert(features_index + 1, "hooks = true")
        config_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return

    prefix = existing if existing.endswith("\n") else existing + "\n"
    if not prefix.endswith("\n\n"):
        prefix += "\n"
    config_path.write_text(prefix + "[features]\nhooks = true\n", encoding="utf-8")


def install_codex_hooks(
    repo_root: Path,
    extra_update_args: list[str] | None = None,
) -> Path:
    """Write Codex global hooks.json and enable the hooks feature flag."""
    codex_dir = Path.home() / ".codex"
    codex_dir.mkdir(parents=True, exist_ok=True)

    hooks_path = codex_dir / "hooks.json"
    existing: dict[str, Any] = {}
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
    hooks_path.write_text(json.dumps(existing, indent=2) + "\n", encoding="utf-8")
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
        hook_path.write_text(existing.rstrip("\n") + "\n" + script, encoding="utf-8")
    else:
        hook_path.write_text(script, encoding="utf-8")

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

    from .worktree import git_hooks_dir

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

    existing: dict[str, Any] = {}
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

    settings_path.write_text(json.dumps(existing, indent=2) + "\n", encoding="utf-8")
    logger.info("Wrote hooks config: %s", settings_path)
    return settings_path


_CLAUDE_MD_SECTION_MARKER = "<!-- dagayn MCP tools -->"
_MARKDOWN_POLICY_MARKER = "<!-- dagayn markdown policy -->"
_CLAUDE_MD_SECTION_HEADING = "## MCP Tools: dagayn"
_MARKDOWN_POLICY_HEADING = (
    "## Markdown documentation policy: declare dependencies via directive comments"
)


def _instruction_section_aliases(marker: str) -> tuple[str, ...]:
    if marker == _CLAUDE_MD_SECTION_MARKER:
        return (_CLAUDE_MD_SECTION_HEADING,)
    if marker == _MARKDOWN_POLICY_MARKER:
        return (
            _MARKDOWN_POLICY_HEADING,
            "## Markdown documentation policy",
            "### Markdown documentation policy",
        )
    return ()


def _has_instruction_section(content: str, marker: str) -> bool:
    """Return True when content already has a dagayn section, marker or not."""
    return marker in content or any(
        alias in content for alias in _instruction_section_aliases(marker)
    )


_MARKDOWN_POLICY_SECTION = f"""{_MARKDOWN_POLICY_MARKER}
## Markdown documentation policy: declare dependencies via directive comments

When authoring or editing a Markdown document in this repository, declare
inter-section and inter-document dependencies as HTML directive comments so
they are captured by the dagayn graph (`DEPENDS_ON` / `IMPORTS_FROM` edges)
and discoverable via `query_graph_tool` / `review_tool(mode="impact")`.

### Required form

```markdown
<!-- <kind> <target> -->
```

`<kind>` MUST be one of: `constrained-by`, `blocked-by`, `supersedes`,
`derived-from`. Choose the kind whose semantics best match the dependency:

| Kind | Use when |
| ---- | -------- |
| `constrained-by` | This section's design is bounded by the referenced document/section |
| `blocked-by` | This item cannot proceed until the referenced item resolves |
| `supersedes` | This document replaces the referenced content |
| `derived-from` | This section is derived from the referenced source |

### Three target shapes

| Dependency type | Target syntax | Example |
| --------------- | ------------- | ------- |
| Within-document section | `#section-slug` | `<!-- derived-from #background -->` |
| Other document (whole file) | `./relative/path.md` | `<!-- blocked-by ./specs/open-issue.md -->` |
| Other document + section | `./path.md#slug` | `<!-- constrained-by ./adr.md#context -->` |

Slugs follow GitHub Markdown rules: lowercase, non-alphanumerics removed,
spaces and hyphens collapsed to `-`. Place the directive immediately under
the heading whose content depends on the target. External URLs
(`http://`, `https://`) are not graph-resolvable — keep them as ordinary
Markdown links, not directive targets.

### When to add a directive

- Section design references an ADR, spec, or research note → `constrained-by` or `derived-from`.
- A document replaces an older one → `supersedes` (place in the new document).
- A spec/task section is blocked on another being resolved → `blocked-by`.
- A later section extends an earlier one non-obviously → `derived-from #earlier-section`.

If no real dependency exists, do not invent one. Directives are signal, not decoration.
"""

_CLAUDE_MD_SECTION = f"""{_CLAUDE_MD_SECTION_MARKER}
## MCP Tools: dagayn

**IMPORTANT: This project has a knowledge graph. ALWAYS use the
dagayn MCP tools BEFORE using Grep/Glob/Read to explore
the codebase.** The graph is faster, cheaper (fewer tokens), and gives
you structural context (callers, dependents, test coverage) that file
scanning cannot.

### When to use graph tools FIRST

- **Any new task**: `get_minimal_context_tool` for graph freshness, risk, and next-tool hints
- **Exploring code**: `semantic_search_nodes_tool` or `query_graph_tool` instead of Grep
- **Understanding impact**: `review_tool(mode="impact")` instead of manually tracing imports
- **Code review**: `review_tool(mode="changes")` first; use its `analysis_summary` before
  calling drill-down tools
- **Finding relationships**: `query_graph_tool` with callers_of/callees_of/imports_of/tests_for
- **Architecture questions**: `architecture_analysis_tool(mode="overview")`
  first; use `architecture_health` and the Architecture Analysis skill before
  choosing a drill-down mode

Fall back to Grep/Glob/Read **only** when the graph result is missing, stale,
ambiguous, or lacks the exact source text needed for the task.

### Tool surface

`dagayn serve` exposes the compact workflow tool surface by default. Use
`dagayn serve --tools ...` when a deployment needs an exact allow-list; the same
allow-list can be supplied with `CRG_TOOLS`. Use `all`, `full`, or `*` to expose
advanced/maintenance tools.

### Default workflow tools

| Tool | Use when |
| ------ | ---------- |
| `get_minimal_context_tool` | Start here: graph freshness, risk, communities, next tools |
| `ensure_graph_tool` | Empty or missing graph; safe bootstrap without embeddings |
| `review_tool` | Primary change review and review drill-down dispatcher |
| `flow_tool` | Reachable-set flow lists and BFS membership (not call sequences) |
| `architecture_analysis_tool` | Primary architecture review and drill-down dispatcher |
| `refactor_tool` | Planning renames, finding dead code, and evidence-ranked refactor suggestions |
| `query_graph_tool` | Tracing callers, callees, imports, tests, dependencies |
| `semantic_search_nodes_tool` | Finding functions/classes by name or keyword |

### Drill-down tools

| Tool | Use when |
| ------ | ---------- |
| `review_tool(mode="impact")` | Need a wider or deeper blast-radius view |
| `review_tool(mode="affected_flows")` | Need full affected execution-path details |
| `architecture_analysis_tool(mode=...)` | Architecture drill-downs for boundaries and metrics |

### How to judge analysis output

- Treat graph insights as **evidence-ranked leads**, not automatic truth.
- Prefer outputs that expose metrics, thresholds, counts, reason codes, and
  `truncated`/`total` fields; mention those numbers when making recommendations.
- Check test coverage with `query_graph_tool` pattern=\"tests_for\" before claiming a
  code path is untested.
- For refactors, verify public APIs, dynamic dispatch, generated code, test
  artifacts, and framework entry points before editing.
- If an output is truncated or approximate, narrow with `top_n`, `detail_level`,
  `max_depth`, or a targeted follow-up query before drawing conclusions.

### Workflow

1. Start with `get_minimal_context_tool(task=...)`.
2. Use the suggested next tool or a targeted query.
3. For reviews, use `review_tool(mode=\"changes\")` and read `analysis_summary`
   first. Call `review_tool(mode=\"context\")`, `review_tool(mode=\"affected_flows\")`,
   `review_tool(mode=\"impact\")`, or `query_graph_tool` only when the summary points there.
4. For architecture work, use
   `architecture_analysis_tool(mode=\"overview\", detail_level=\"minimal\")`
   and read `architecture_health` first. Use the Architecture Analysis skill to
   choose drill-down modes only when the health summary identifies a concrete risk.
5. For refactors, use `refactor_tool(mode=\"suggest\")` first, then preview
   renames with `refactor_tool(mode=\"rename\")`. Apply with
   `dagayn tool apply_refactor_tool` (advanced MCP surface: `dagayn serve --tools all`).
"""


def _inject_instructions(
    file_path: Path,
    marker: str,
    section: str,
    *,
    errors: list[str] | None = None,
) -> bool:
    """Append an instruction section to a file if not already present.

    Idempotent: checks if the marker is already present before appending.
    Creates the file if it doesn't exist.

    Returns True if the file was modified.
    """
    existing = ""
    try:
        if file_path.exists():
            existing = file_path.read_text(encoding="utf-8", errors="replace")

        if marker in existing:
            logger.info("%s already contains instructions, skipping.", file_path.name)
            return False

        for marker_heading in _instruction_section_aliases(marker):
            if marker_heading in existing:
                updated = existing.replace(marker_heading, f"{marker}\n{marker_heading}", 1)
                file_path.write_text(updated, encoding="utf-8")
                logger.info("Added missing dagayn marker to %s", file_path)
                return True

        separator = "\n" if existing and not existing.endswith("\n") else ""
        extra_newline = "\n" if existing else ""
        file_path.parent.mkdir(parents=True, exist_ok=True)
        file_path.write_text(existing + separator + extra_newline + section, encoding="utf-8")
    except OSError as exc:
        message = f"{file_path} ({exc})"
        if errors is not None:
            errors.append(message)
        logger.debug("Skipped instruction injection for %s: %s", file_path, exc)
        return False
    logger.info("Appended MCP tools section to %s", file_path)
    return True


def inject_claude_md(
    repo_root: Path | None = None,
    *,
    errors: list[str] | None = None,
) -> list[str]:
    """Append MCP tools section and Markdown policy to ``~/.claude/CLAUDE.md``."""
    claude_md = Path.home() / ".claude" / "CLAUDE.md"
    updated = False
    if _inject_instructions(
        claude_md,
        _CLAUDE_MD_SECTION_MARKER,
        _CLAUDE_MD_SECTION,
        errors=errors,
    ):
        updated = True
    if _inject_instructions(
        claude_md,
        _MARKDOWN_POLICY_MARKER,
        _MARKDOWN_POLICY_SECTION,
        errors=errors,
    ):
        updated = True
    return ["~/.claude/CLAUDE.md"] if updated else []


# Cross-platform instruction files and which platforms own each one.
# Used to filter writes when the user passes --platform <X>: only files
# whose owner set includes the target (or "all") are written.
_PLATFORM_INSTRUCTION_FILES: dict[str, tuple[str, ...]] = {
    "AGENTS.md": ("codex", "cursor", "opencode", "antigravity"),
    "GEMINI.md": ("antigravity",),
    ".cursorrules": ("cursor",),
    ".windsurfrules": ("windsurf",),
    "QODER.md": ("qoder",),
    ".kiro/steering/dagayn.md": ("kiro",),
}


def _platform_instruction_paths(repo_root: Path, filename: str, target: str) -> list[Path]:
    """Return the destination path(s) for a platform instruction file."""
    target = normalize_platform_target(target)

    if filename != "AGENTS.md":
        return [repo_root / filename]

    if target == "codex":
        return [Path.home() / ".codex" / "AGENTS.md"]
    if target == "opencode":
        return [Path.home() / ".config" / "opencode" / "AGENTS.md"]
    if target == "all":
        return [
            repo_root / "AGENTS.md",
            Path.home() / ".codex" / "AGENTS.md",
            Path.home() / ".config" / "opencode" / "AGENTS.md",
        ]
    return [repo_root / "AGENTS.md"]


def inject_platform_instructions(
    repo_root: Path,
    target: str = "all",
    *,
    errors: list[str] | None = None,
) -> list[str]:
    """Inject 'use graph first' instructions into platform rule files.

    Writes AGENTS.md, GEMINI.md, .cursorrules, and/or .windsurfrules
    depending on ``target``:

    - ``"all"`` (default): writes every file — matches pre-filter behavior.
    - ``"claude"``: writes nothing (``~/.claude/CLAUDE.md`` is handled by ``inject_claude_md``).
    - any other platform key (``cursor``, ``windsurf``, ``antigravity``,
      ``opencode``): writes only the files associated with that platform.

    Returns list of filenames that were created or updated.
    """
    target = normalize_platform_target(target)
    updated: list[str] = []
    for filename, owners in _PLATFORM_INSTRUCTION_FILES.items():
        if target != "all" and target not in owners:
            continue
        changed = False
        for path in _platform_instruction_paths(repo_root, filename, target):
            if _inject_instructions(
                path,
                _CLAUDE_MD_SECTION_MARKER,
                _CLAUDE_MD_SECTION,
                errors=errors,
            ):
                changed = True
            if _inject_instructions(
                path,
                _MARKDOWN_POLICY_MARKER,
                _MARKDOWN_POLICY_SECTION,
                errors=errors,
            ):
                changed = True
        if changed:
            updated.append(filename)
    return updated


# --- Worktree file inheritance (.worktreeinclude) ---


_WORKTREEINCLUDE_START = "# >>> dagayn worktree include"
_WORKTREEINCLUDE_END = "# <<< dagayn worktree include"


def worktree_include_patterns(repo_root: Path, platform_keys: list[str] | None = None) -> list[str]:
    """Return ``.worktreeinclude`` patterns for dagayn config in *repo_root*.

    Only patterns whose target exists **and** is gitignored are returned:
    tracked files are already checked out into every worktree, and
    ``.worktreeinclude`` copies gitignored matches only.
    """
    from .worktree import PLATFORM_CONFIG_PATTERNS, config_pattern_target, is_gitignored

    keys = list(PLATFORM_CONFIG_PATTERNS) if platform_keys is None else platform_keys
    patterns: list[str] = []
    for key in keys:
        for pattern in PLATFORM_CONFIG_PATTERNS.get(normalize_platform_target(key), ()):
            target = config_pattern_target(pattern)
            path = repo_root / target
            if not path.exists():
                continue
            if not is_gitignored(repo_root, target):
                continue
            if pattern not in patterns:
                patterns.append(pattern)
    return patterns


def ensure_worktree_include(
    repo_root: Path,
    patterns: list[str],
    dry_run: bool = False,
) -> str:
    """Maintain a dagayn-managed block in ``<repo_root>/.worktreeinclude``.

    Claude Code copies gitignored files matching this file into every worktree
    it creates (``--worktree``, ``EnterWorktree``, subagent and desktop
    worktrees), which is how MCP config survives into a worktree session.

    Returns ``"created"``, ``"updated"``, ``"unchanged"``, or ``"skipped"``
    (nothing to add).
    """
    if not patterns:
        return "skipped"

    block_lines = [
        _WORKTREEINCLUDE_START,
        "# Copied into new git worktrees so agent sessions there keep the dagayn",
        "# MCP server and skills. Managed by 'dagayn install'.",
        *patterns,
        _WORKTREEINCLUDE_END,
    ]
    block = "\n".join(block_lines) + "\n"

    path = repo_root / ".worktreeinclude"
    existing = ""
    if path.exists():
        try:
            existing = path.read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            logger.warning("Could not read %s: %s", path, exc)
            return "skipped"

    if _WORKTREEINCLUDE_START in existing:
        start = existing.index(_WORKTREEINCLUDE_START)
        end_marker = existing.find(_WORKTREEINCLUDE_END, start)
        if end_marker == -1:
            # No end marker (hand-edited or a partially-written file). Replacing
            # everything from the start marker onward silently discarded the
            # user's own patterns below it; keep the remainder instead.
            remainder = existing[start + len(_WORKTREEINCLUDE_START) :]
            updated = existing[:start] + block.rstrip("\n") + "\n" + remainder.lstrip("\n")
        else:
            end = end_marker + len(_WORKTREEINCLUDE_END)
            updated = existing[:start] + block.rstrip("\n") + existing[end:]
        if updated == existing:
            return "unchanged"
        if not dry_run:
            path.write_text(updated, encoding="utf-8")
        return "updated"

    if not existing:
        if not dry_run:
            path.write_text(block, encoding="utf-8")
        return "created"

    prefix = existing if existing.endswith("\n") else existing + "\n"
    if not dry_run:
        path.write_text(prefix + block, encoding="utf-8")
    return "updated"


_CURSOR_WORKTREE_SETUP_COMMAND = "dagayn session prepare --budget-seconds 45"
_CURSOR_SETUP_KEYS = ("setup-worktree-unix", "setup-worktree-windows")
_CURSOR_SETUP_FALLBACK_KEY = "setup-worktree"


def _merge_cursor_setup_commands(commands: list[Any]) -> list[Any]:
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
    existing: dict[str, Any] = {}
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
        config_path.write_text(json.dumps(updated, indent=2) + "\n", encoding="utf-8")
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


def generate_cursor_hooks_config() -> dict[str, Any]:
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
    - crg-update.sh: runs ``dagayn update --skip-flows`` after file edits
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
        extra_update_args: Additional CLI args appended to ``dagayn update`` /
            ``session prepare`` (e.g. ``["--local-embedding"]``) so hooks use
            the same embedding configuration as the install.
    """
    update_args = ""
    if extra_update_args:
        update_args = " " + " ".join(shlex.quote(arg) for arg in extra_update_args)

    update_script = f"""\
#!/usr/bin/env bash
# dagayn: auto-update graph after file edits (Cursor hook)
# Fails gracefully — never blocks the editor.
{_CURSOR_HOOK_PROLOGUE}
# afterFileEdit fires on every edit, so run detached: the editor never waits
# and DAGAYN_HOOK_UPDATE makes concurrent updates skip instead of pile up.
# afterFileEdit is observational — no output schema to satisfy.
if [ -n "$repo" ]; then
  ( DAGAYN_HOOK_UPDATE=1 dagayn update --skip-flows{update_args} \\
      --repo "$repo" >/dev/null 2>&1 & ) >/dev/null 2>&1
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
  --budget-seconds {_SESSION_PREPARE_BUDGET_SECONDS}{update_args} \\
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
dagayn update --skip-flows{update_args} --repo "$repo" >/dev/null 2>&1 || true
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
  --budget-seconds {_SESSION_PREPARE_BUDGET_SECONDS}{update_args} \\
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
    existing: dict[str, Any] = {}
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
    hooks_json_path.write_text(
        json.dumps(existing, indent=2) + "\n",
        encoding="utf-8",
    )
    logger.info("Wrote Cursor hooks config: %s", hooks_json_path)

    # --- Write hook scripts ---
    hooks_script_dir.mkdir(parents=True, exist_ok=True)
    scripts = _cursor_hook_scripts(extra_update_args)

    for filename, content in scripts.items():
        script_path = hooks_script_dir / filename
        script_path.write_text(content, encoding="utf-8")
        # Make executable (owner rwx, group rx, other rx)
        script_path.chmod(stat.S_IRWXU | stat.S_IRGRP | stat.S_IXGRP | stat.S_IROTH | stat.S_IXOTH)
        logger.info("Wrote Cursor hook script: %s", script_path)

    return hooks_json_path


def install_qoder_skills(
    repo_root: Path,
    *,
    embedding_mode: str | None = None,
    embedding_preset: str | None = None,
    embedding_provider: str | None = None,
) -> Path | None:
    """Install skills to Qoder's project-level skills directory.

    Qoder expects skills in .qoder/skills/{skillName}/SKILL.md format within the project.
    This function copies the project's skills/ directory contents to that location.

    Args:
        repo_root: Repository root directory (where the skills/ folder is located).
        embedding_mode: Optional install mode (``fts-only``,
            ``local-embedding``, ``local-embedding-llama``, or
            ``remote-embedding``) used to render search guidance in skills that
            opt in.
        embedding_preset: Local sidecar preset when ``embedding_mode`` is
            ``local-embedding-llama``.
        embedding_provider: Remote embedding provider when ``embedding_mode``
            is ``remote-embedding``.

    Returns:
        Path to the Qoder skills directory, or None if installation failed.
    """
    # Qoder skills directory (project-level)
    qoder_skills_dir = repo_root / ".qoder" / "skills"
    qoder_skills_dir.mkdir(parents=True, exist_ok=True)

    # Source skills directory in the project
    source_skills_dir = repo_root / "skills"
    if not source_skills_dir.exists():
        logger.warning("No skills/ directory found in %s", repo_root)
        return None

    installed_count = 0
    for skill_dir in source_skills_dir.iterdir():
        if skill_dir.is_dir():
            skill_file = skill_dir / "SKILL.md"
            if skill_file.exists():
                target_dir = qoder_skills_dir / skill_dir.name
                if target_dir.exists():
                    shutil.rmtree(target_dir)
                shutil.copytree(skill_dir, target_dir)
                target_skill = target_dir / "SKILL.md"
                target_skill.write_text(
                    _render_skill_content(
                        target_skill.read_text(encoding="utf-8"),
                        embedding_mode=embedding_mode,
                        embedding_preset=embedding_preset,
                        embedding_provider=embedding_provider,
                    ),
                    encoding="utf-8",
                )
                logger.info("Installed Qoder skill: %s", skill_dir.name)
                installed_count += 1

    if installed_count > 0:
        logger.info("Installed %d skill(s) to %s", installed_count, qoder_skills_dir)
        return qoder_skills_dir
    return None


# --- OpenCode plugin ---


def _opencode_plugin_content(extra_update_args: list[str] | None = None) -> str:
    """Return TypeScript source for the OpenCode user-level plugin.

    The plugin hooks into four OpenCode events to mirror the Claude Code
    hook behaviors:

    1. ``file.edited`` — runs ``dagayn update --skip-flows``
    2. ``session.created`` — prepares a usable+synced graph, then status
    3. ``tool.execute.before`` — when the tool is a shell command starting
       with ``git commit``, runs ``dagayn update --skip-flows`` followed by
       ``dagayn detect-changes --brief``
    4. ``tool.execute.after`` — when the tool is a HEAD-moving git command
       (``checkout`` / ``switch`` / ``pull`` / …), runs ``session prepare``
       so the graph catches up to the new HEAD

    Every command resolves the repository with ``git rev-parse --show-toplevel``
    and passes ``--repo`` explicitly so worktree sessions update the checkout
    they are running in, not whichever directory OpenCode happened to start in.

    All handlers use try/catch so errors never break the editor session.
    The plugin uses Bun's ``$`` shell API (provided by OpenCode's plugin
    context) for subprocess execution.
    """
    update_args = ""
    if extra_update_args:
        update_args = " " + " ".join(shlex.quote(arg) for arg in extra_update_args)
    template = """\
import type { Plugin } from "@opencode-ai/plugin"

/**
 * dagayn plugin for OpenCode.
 *
 * Keeps the knowledge graph up-to-date and surfaces status
 * information automatically during coding sessions.
 *
 * Installed by: dagayn install --platform opencode
 */

// Resolve the git repository root for the active project directory.
async function resolveRepo($: any): Promise<string> {
  try {
    const result = await $`git rev-parse --show-toplevel`.quiet()
    return result.stdout?.toString().trim() ?? ""
  } catch {
    return ""
  }
}

function shellCommand(ctx: any): string {
  const input = ctx?.input ?? ctx?.params ?? ctx?.args ?? {}
  const cmd =
    input.command ??
    input.cmd ??
    input.content ??
    ctx?.args?.command ??
    ""
  return typeof cmd === "string" ? cmd : ""
}

export default (app: any) => {
  // 1. Auto-update graph after file edits
  app.on("file.edited", async ({ $ }: { $: any }) => {
    try {
      const repo = await resolveRepo($)
      if (repo) {
        await $`dagayn update --skip-flows__DAGAYN_UPDATE_ARGS__ --repo ${repo}`.quiet()
      } else {
        await $`dagayn update --skip-flows__DAGAYN_UPDATE_ARGS__`.quiet()
      }
    } catch {
      // Swallow — graph may not be built yet for this project.
    }
  })

  // 2. Prepare a usable+synced graph when a session starts
  app.on("session.created", async ({ $ }: { $: any }) => {
    try {
      const prepare =
        "DAGAYN_HOOK_UPDATE=1 dagayn session prepare --budget-seconds 45__DAGAYN_UPDATE_ARGS__"
      const repo = await resolveRepo($)
      if (repo) {
        await $`${prepare} --repo ${repo}`.quiet()
        const result = await $`dagayn status --repo ${repo}`.quiet()
        const output = result.stdout?.toString().trim()
        if (output) {
          console.log("[dagayn]", output)
        }
      } else {
        await $`${prepare}`.quiet()
        const result = await $`dagayn status`.quiet()
        const output = result.stdout?.toString().trim()
        if (output) {
          console.log("[dagayn]", output)
        }
      }
    } catch {
      // Swallow — not every project has a graph.
    }
  })

  // 3. Detect changes before git commit commands
  app.on("tool.execute.before", async (ctx: any) => {
    try {
      const cmd = shellCommand(ctx)
      if (/(?:^|[\\/\\\\]|\\s)git(?:\\.exe)?\\s+commit\\b/i.test(cmd)) {
        const repo = await resolveRepo(ctx.$)
        if (repo) {
          await ctx.$`dagayn update --skip-flows__DAGAYN_UPDATE_ARGS__ --repo ${repo}`.quiet()
          const result =
            await ctx.$`dagayn detect-changes --brief --repo ${repo}`.quiet()
          const output = result.stdout?.toString().trim()
          if (output) {
            console.log("[dagayn] Pre-commit analysis:\\n" + output)
          }
        } else {
          await ctx.$`dagayn update --skip-flows__DAGAYN_UPDATE_ARGS__`.quiet()
          const result =
            await ctx.$`dagayn detect-changes --brief`.quiet()
          const output = result.stdout?.toString().trim()
          if (output) {
            console.log("[dagayn] Pre-commit analysis:\\n" + output)
          }
        }
      }
    } catch {
      // Swallow — never block a commit.
    }
  })

  // 4. Re-prepare after HEAD-moving git commands (post-execution)
  app.on("tool.execute.after", async (ctx: any) => {
    try {
      const cmd = shellCommand(ctx)
      if (
        /(?:^|[\\/\\\\]|\\s)git(?:\\.exe)?\\s+(?:checkout|switch|reset|pull|merge|rebase|cherry-pick)\\b/i.test(
          cmd,
        )
      ) {
        const repo = await resolveRepo(ctx.$)
        const prepare =
          "DAGAYN_HOOK_UPDATE=1 dagayn session prepare --budget-seconds 45__DAGAYN_UPDATE_ARGS__"
        if (repo) {
          await ctx.$`${prepare} --repo ${repo}`.quiet()
        } else {
          await ctx.$`${prepare}`.quiet()
        }
      }
    } catch {
      // Swallow — never block a checkout.
    }
  })
}
"""
    return template.replace("__DAGAYN_UPDATE_ARGS__", update_args)


def install_opencode_plugin(extra_update_args: list[str] | None = None) -> Path:
    """Install the OpenCode user-level plugin for dagayn.

    Writes ``~/.config/opencode/plugins/crg-plugin.ts``.  Creates the
    directories if they don't exist.  If the file already exists it is
    overwritten (the plugin is self-contained and idempotent).

    Returns:
        Path to the plugin file that was written.
    """
    plugins_dir = Path.home() / ".config" / "opencode" / "plugins"
    plugin_path = plugins_dir / "crg-plugin.ts"

    plugins_dir.mkdir(parents=True, exist_ok=True)
    plugin_path.write_text(_opencode_plugin_content(extra_update_args), encoding="utf-8")
    logger.info("Wrote OpenCode plugin: %s", plugin_path)

    return plugin_path
