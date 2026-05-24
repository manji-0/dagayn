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
_STATUS_HOOK_TIMEOUT_SECONDS = 10

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

    return configured


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
    if embedding_mode == "local":
        preset = embedding_preset or "low"
        return [
            "## Installed Search Mode",
            "",
            f"Installed with local embeddings (`--mode local --preset {preset}`).",
            "",
            "- MCP search defaults to hybrid retrieval when matching embeddings exist.",
            "- Routine graph refreshes for parser, flow, documentation, or review "
            'verification should pass `local_embedding="none"` so they do not '
            "inherit the server preset and trigger a large embedding refresh.",
            "- Use embedding-enabled full rebuilds only for explicit embedding-quality "
            "or end-to-end maintenance work after stating the reason.",
            "- Exact identifier lookup can still rely on FTS; use semantic search for "
            "fuzzy concepts, domain terms, cross-language search, or unfamiliar code.",
        ]
    if embedding_mode == "remote":
        provider = embedding_provider or "openai"
        return [
            "## Installed Search Mode",
            "",
            f"Installed with remote embeddings (`--mode remote --provider {provider}`).",
            "",
            "- MCP search defaults to the configured provider when matching embeddings exist.",
            "- `build_or_update_graph_tool()` refreshes graph and FTS data; run "
            f'`embed_graph_tool(provider="{provider}")` after graph refresh when hybrid '
            "search is required.",
            "- Use FTS for exact lookup and reserve remote embedding calls for fuzzy, "
            "cross-repo, or conceptual searches.",
        ]
    if embedding_mode == "fts":
        return [
            "## Installed Search Mode",
            "",
            "Installed in FTS-only mode (`--mode fts`).",
            "",
            "- Treat `semantic_search_nodes_tool` as keyword/FTS search, not vector "
            "semantic search.",
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
        embedding_mode: Optional install mode (``fts``, ``local``, or ``remote``)
            used to render search guidance in skills that opt in.
        embedding_preset: Local embedding preset when ``embedding_mode`` is
            ``local``.
        embedding_provider: Remote embedding provider when ``embedding_mode`` is
            ``remote``.

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
        "dagayn-status.sh": """#!/usr/bin/env bash
# dagayn: validate graph availability at session start
set -u
cat >/dev/null || true
repo="$(git rev-parse --show-toplevel 2>/dev/null || true)"
if [ -n "$repo" ]; then
  dagayn status --repo "$repo" >/dev/null 2>&1 || true
fi
printf '{}\\n'
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
) -> dict[str, Any]:
    """Generate Claude Code hooks configuration.

    Hooks use the v1.x+ schema: each entry needs a ``matcher`` and a nested
    ``hooks`` array. Timeouts are in seconds. ``PreCommit`` is not a valid
    Claude Code event — pre-commit checks are handled by ``install_git_hook``.
    """
    del repo_root  # Hooks are global; resolve the active repository at runtime.
    update_args = ""
    if extra_update_args:
        update_args = " " + " ".join(shlex.quote(arg) for arg in extra_update_args)
    repo_expr = 'repo="$(git rev-parse --show-toplevel 2>/dev/null)"'
    return {
        "hooks": {
            "PostToolUse": [
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
            ],
            "SessionStart": [
                {
                    "matcher": "",
                    "hooks": [
                        {
                            "type": "command",
                            "command": (
                                f"{repo_expr}"
                                ' && dagayn status --repo "$repo"'
                                " || echo 'Not a git repo, skipping'"
                            ),
                            "timeout": _STATUS_HOOK_TIMEOUT_SECONDS,
                        },
                    ],
                },
            ],
        }
    }


_DAGAYN_HOOK_NEEDLES = {
    "PostToolUse": "dagayn update --skip-flows",
    "SessionStart": "dagayn status --repo",
}


def _is_dagayn_generated_hook_entry(hook_name: str, entry: Any) -> bool:
    """Return True for hook entries generated by dagayn itself."""
    if not isinstance(entry, dict):
        return False
    needle = _DAGAYN_HOOK_NEEDLES.get(hook_name)
    if not needle:
        return False
    hooks = entry.get("hooks", [])
    if not isinstance(hooks, list):
        return False
    return any(isinstance(hook, dict) and needle in str(hook.get("command", "")) for hook in hooks)


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

    hooks_config = generate_hooks_config(repo_root, extra_update_args=extra_update_args)
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
    Creates ``.git/hooks/pre-commit`` and ``.git/hooks/post-commit`` if they
    don't exist, or appends to existing hooks — preserving any hooks already
    there. Returns None when no ``.git`` directory is found.
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

    git_dir = repo_root / ".git"
    if not git_dir.is_dir():
        logger.warning("No .git directory found at %s — skipping git hook install.", repo_root)
        return None

    hooks_dir = git_dir / "hooks"
    hooks_dir.mkdir(exist_ok=True)
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
| `review_tool` | Primary change review and review drill-down dispatcher |
| `flow_tool` | Execution-flow lists and step-by-step flow paths |
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
   renames with `refactor_tool(mode=\"rename\")` and `apply_refactor_tool(dry_run=True)`.
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


# --- Cursor hooks ---


def generate_cursor_hooks_config() -> dict[str, Any]:
    """Generate Cursor hooks.json configuration.

    Returns a dict conforming to the Cursor hooks schema (version 1) with
    hooks for afterFileEdit, sessionStart, and beforeShellExecution.
    Each hook points to a shell script in ~/.cursor/hooks/.

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
                    "timeout": 5,
                },
            ],
            "sessionStart": [
                {
                    "command": f"{hooks_dir}/crg-session-start.sh",
                    "timeout": 5,
                },
            ],
            "beforeShellExecution": [
                {
                    "matcher": "^git\\s+commit",
                    "command": f"{hooks_dir}/crg-pre-commit.sh",
                    "timeout": 10,
                },
            ],
        },
    }


def _cursor_hook_scripts() -> dict[str, str]:
    """Return a mapping of filename -> shell script content for Cursor hooks.

    Three scripts are generated:
    - crg-update.sh: runs ``dagayn update --skip-flows`` after file edits
    - crg-session-start.sh: runs ``dagayn status`` on session start
    - crg-pre-commit.sh: runs ``dagayn update --skip-flows`` and
      ``dagayn detect-changes --brief`` before git commit commands

    All scripts:
    - Read stdin (Cursor passes JSON context) and discard it
    - Fail gracefully (exit 0) so they never block the editor
    - Emit valid JSON on stdout per the Cursor hooks protocol
    """
    update_script = """\
#!/usr/bin/env bash
# dagayn: auto-update graph after file edits (Cursor hook)
# Fails gracefully — never blocks the editor.
set -euo pipefail

# Consume stdin (Cursor sends JSON context)
cat > /dev/null

# Run update; swallow errors so the hook always succeeds.
output=$(dagayn update --skip-flows 2>&1) || true

# Emit valid JSON on stdout per Cursor hooks protocol.
python3 -c "
import json, sys
print(json.dumps({'message': 'graph updated', 'passed': True}))
" 2>/dev/null || echo '{"passed":true}'

exit 0
"""

    session_start_script = """\
#!/usr/bin/env bash
# dagayn: show graph status on session start (Cursor hook)
# Fails gracefully — never blocks the editor.
set -euo pipefail

# Consume stdin
cat > /dev/null

# Capture status output
output=$(dagayn status 2>&1) || output="graph not built yet"

# Emit valid JSON on stdout
python3 -c "
import json, sys
msg = sys.stdin.read()
print(json.dumps({'message': msg, 'passed': True}))
" <<< "$output" 2>/dev/null || echo '{"passed":true}'

exit 0
"""

    pre_commit_script = """\
#!/usr/bin/env bash
# dagayn: detect changes before git commit (Cursor hook)
# Fails gracefully — never blocks the editor.
set -euo pipefail

# Consume stdin
cat > /dev/null

# Refresh the graph cheaply, then run detect-changes; swallow errors.
dagayn update --skip-flows >/dev/null 2>&1 || true
output=$(dagayn detect-changes --brief 2>&1) || output=""

# Emit valid JSON on stdout
python3 -c "
import json, sys
msg = sys.stdin.read()
print(json.dumps({'message': msg, 'passed': True}))
" <<< "$output" 2>/dev/null || echo '{"passed":true}'

exit 0
"""

    return {
        "crg-update.sh": update_script,
        "crg-session-start.sh": session_start_script,
        "crg-pre-commit.sh": pre_commit_script,
    }


def install_cursor_hooks() -> Path:
    """Install Cursor hooks configuration and scripts at user level.

    Writes ``~/.cursor/hooks.json`` (merging dagayn hooks
    into any existing configuration) and creates executable shell scripts
    in ``~/.cursor/hooks/``.

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
        # De-duplicate: skip if a hook with the same command already exists
        existing_commands = {h.get("command", "") for h in event_hooks if isinstance(h, dict)}
        for entry in entries:
            if entry["command"] not in existing_commands:
                event_hooks.append(entry)
        existing_hooks[event] = event_hooks

    existing["hooks"] = existing_hooks

    cursor_dir.mkdir(parents=True, exist_ok=True)
    hooks_json_path.write_text(
        json.dumps(existing, indent=2) + "\n",
        encoding="utf-8",
    )
    logger.info("Wrote Cursor hooks config: %s", hooks_json_path)

    # --- Write hook scripts ---
    hooks_script_dir.mkdir(parents=True, exist_ok=True)
    scripts = _cursor_hook_scripts()

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
        embedding_mode: Optional install mode (``fts``, ``local``, or ``remote``)
            used to render search guidance in skills that opt in.
        embedding_preset: Local embedding preset when ``embedding_mode`` is
            ``local``.
        embedding_provider: Remote embedding provider when ``embedding_mode`` is
            ``remote``.

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


def _opencode_plugin_content() -> str:
    """Return TypeScript source for the OpenCode user-level plugin.

    The plugin hooks into three OpenCode events to mirror the Claude Code
    hook behaviors:

    1. ``file.edited`` — runs ``dagayn update --skip-flows``
    2. ``session.created`` — runs ``dagayn status``
    3. ``tool.execute.before`` — when the tool is a shell command starting
       with ``git commit``, runs ``dagayn update --skip-flows`` followed by
       ``dagayn detect-changes --brief``

    All handlers use try/catch so errors never break the editor session.
    The plugin uses Bun's ``$`` shell API (provided by OpenCode's plugin
    context) for subprocess execution.
    """
    return """\
import type { Plugin } from "@opencode-ai/plugin"

/**
 * dagayn plugin for OpenCode.
 *
 * Keeps the knowledge graph up-to-date and surfaces status
 * information automatically during coding sessions.
 *
 * Installed by: dagayn install --platform opencode
 */

// Helper: run a shell command quietly, swallowing errors.
async function run($: any, cmd: string): Promise<string> {
  try {
    const result = await $`${cmd}`.quiet()
    return result.stdout?.toString().trim() ?? ""
  } catch {
    return ""
  }
}

export default (app: any) => {
  // 1. Auto-update graph after file edits
  app.on("file.edited", async ({ $ }: { $: any }) => {
    try {
      await $`dagayn update --skip-flows`.quiet()
    } catch {
      // Swallow — graph may not be built yet for this project.
    }
  })

  // 2. Show graph status when a new session starts
  app.on("session.created", async ({ $ }: { $: any }) => {
    try {
      const result = await $`dagayn status`.quiet()
      const output = result.stdout?.toString().trim()
      if (output) {
        console.log("[dagayn]", output)
      }
    } catch {
      // Swallow — not every project has a graph.
    }
  })

  // 3. Detect changes before git commit commands
  app.on("tool.execute.before", async (ctx: any) => {
    try {
      const input = ctx?.input ?? ctx?.params ?? {}
      const cmd =
        input.command ?? input.cmd ?? input.content ?? ""
      if (typeof cmd === "string" && /^git\\s+commit/i.test(cmd)) {
        await ctx.$`dagayn update --skip-flows`.quiet()
        const result =
          await ctx.$`dagayn detect-changes --brief`.quiet()
        const output = result.stdout?.toString().trim()
        if (output) {
          console.log("[dagayn] Pre-commit analysis:\\n" + output)
        }
      }
    } catch {
      // Swallow — never block a commit.
    }
  })
}
"""


def install_opencode_plugin() -> Path:
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
    plugin_path.write_text(_opencode_plugin_content(), encoding="utf-8")
    logger.info("Wrote OpenCode plugin: %s", plugin_path)

    return plugin_path
