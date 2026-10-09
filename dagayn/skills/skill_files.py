"""Claude Code skills and hooks auto-install.

Generates Claude Code agent skill files, hooks configuration, and
CLAUDE.md integration for seamless dagayn usage.
Also supports multi-platform MCP server installation and
Cursor hooks / OpenCode plugin generation.
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

from ..atomic_write import write_text_atomic
from .platforms import logger

# --- Skill file contents ---


# Skills earlier dagayn versions installed and no longer ship: an upgrade
# removes the copies it left behind.
_RETIRED_SKILLS = ("wiki-research", "install-dagayn", "semantic-search")

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
        Path(__file__).resolve().parent.parent / "skills",
        Path(__file__).resolve().parent.parent.parent / "skills",
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
            "- Read `search_mode`, `embedding_health.requested_text_mode` (at "
            '`detail_level="verbose"`), and '
            '(at `detail_level="standard"`) per-result `source` before judging '
            "search quality.",
            "- Routine graph refreshes with `build_or_update_graph_tool` for parser, "
            'flow, documentation, or review verification should pass `local_embedding="none"` '
            "so they do not inherit the server embedding mode and trigger a large "
            "embedding refresh (`ensure_graph_tool` always follows the server mode).",
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
            "- Read `search_mode`, `embedding_health.requested_text_mode` (at "
            '`detail_level="verbose"`), and '
            '(at `detail_level="standard"`) per-result `source` before judging '
            "search quality.",
            "- Routine graph refreshes with `build_or_update_graph_tool` for parser, "
            'flow, documentation, or review verification should pass `local_embedding="none"` '
            "so they do not inherit the server sidecar mode and trigger a large "
            "embedding refresh (`ensure_graph_tool` always follows the server mode).",
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
            "- Read `search_mode`, `embedding_health.requested_text_mode` (at "
            '`detail_level="verbose"`), and '
            '(at `detail_level="standard"`) per-result `source` before judging '
            "search quality.",
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
            "neither FTS nor vectors matched and LIKE matching ran; try other terms, "
            "and check `dagayn status` before claiming the index is missing.",
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


# Skills that carry the full install-specific search guidance; every other
# skill gets the two-line summary from ``_embedding_summary_lines`` so the
# same paragraph is not loaded with each skill.
_FULL_EMBEDDING_CONTEXT_SKILLS = frozenset({"build-graph"})


def _embedding_summary_lines(
    embedding_mode: str | None = None,
    embedding_preset: str | None = None,
    embedding_provider: str | None = None,
) -> list[str]:
    """Return the short install-specific search note for non-search skills."""
    details = "The build-graph skill has the full search guidance."
    if embedding_mode == "local-embedding":
        body = (
            "Installed with local embeddings (`--mode local-embedding`, managed BGE-M3 "
            "sidecar): search is hybrid when vectors exist. Pass "
            '`local_embedding="none"` to routine `build_or_update_graph_tool` refreshes '
            "so they do not trigger an embedding refresh."
        )
    elif embedding_mode == "local-embedding-llama":
        preset = embedding_preset or "low"
        body = (
            "Installed with managed Qwen3 embeddings "
            f"(`--mode local-embedding-llama --preset {preset}`): search is hybrid when "
            'vectors exist. Pass `local_embedding="none"` to routine '
            "`build_or_update_graph_tool` refreshes so they do not trigger an embedding "
            "refresh."
        )
    elif embedding_mode == "remote-embedding":
        provider = embedding_provider or "openai"
        body = (
            "Installed with remote embeddings "
            f"(`--mode remote-embedding --provider {provider}`): use FTS for exact "
            "lookup and reserve remote embedding calls for fuzzy or conceptual searches."
        )
    elif embedding_mode == "fts-only":
        body = (
            "Installed in FTS-only mode (`--mode fts-only`): treat "
            "`semantic_search_nodes_tool` as keyword/FTS search, and do not rebuild "
            "embeddings unless the user changes the install mode."
        )
    else:
        body = (
            "This packaged skill is mode-neutral; check `search_mode` in a "
            "`semantic_search_nodes_tool` result before assuming hybrid search."
        )
    return ["## Installed Search Mode", "", body, details]


def _render_skill_content(
    content: str,
    *,
    skill_name: str | None = None,
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

    render_lines = (
        _embedding_context_lines
        if skill_name is None or skill_name in _FULL_EMBEDDING_CONTEXT_SKILLS
        else _embedding_summary_lines
    )
    context = "\n".join(
        render_lines(
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

    Writes each ``skills/<name>/SKILL.md`` of the dagayn package as
    ``<skills_dir>/<name>/SKILL.md``, the layout Claude Code loads skills
    from. Earlier versions wrote flat ``<skills_dir>/<name>.md`` files,
    which Claude Code never loaded; those are removed.

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
    _install_skill_tree(
        skills_dir,
        embedding_mode=embedding_mode,
        embedding_preset=embedding_preset,
        embedding_provider=embedding_provider,
    )
    source_dir = _resolve_source_skills_dir()
    names = [entry.name for entry in source_dir.iterdir()] if source_dir else []
    for name in [*names, *_RETIRED_SKILLS]:
        _remove_dagayn_skill(skills_dir / f"{name}.md", name)
    return skills_dir


def _remove_dagayn_skill(path: Path, name: str) -> bool:
    """Remove ``path`` (a flat ``<name>.md`` or a ``<name>/`` directory) when
    it is a dagayn skill of that name, judged by its frontmatter; any other
    file is the user's and stays. Returns whether it was removed."""
    skill_file = path / "SKILL.md" if path.is_dir() else path
    try:
        head = skill_file.read_text(encoding="utf-8", errors="replace")[:500]
    except OSError:
        return False
    if not (head.startswith("---\n") and f"\nname: {name}\n" in head):
        return False
    if path.is_dir():
        shutil.rmtree(path)
    else:
        path.unlink()
    logger.info("Removed stale dagayn skill: %s", path)
    return True


def remove_repo_local_skills(repo_root: Path) -> int:
    """Remove the dagayn skills an earlier install wrote into
    ``<repo>/.claude/skills``. Claude Code loads both that directory and
    ``~/.claude/skills``, so a repo-local copy next to the global one is
    listed twice in every session. Skills that are not dagayn's stay.
    Returns how many were removed."""
    skills_dir = repo_root / ".claude" / "skills"
    if not skills_dir.is_dir():
        return 0
    tracked = _git_tracked(repo_root, skills_dir)
    source_dir = _resolve_source_skills_dir()
    names = [entry.name for entry in source_dir.iterdir()] if source_dir else []
    removed = 0
    for name in [*names, *_RETIRED_SKILLS]:
        for path in (skills_dir / name, skills_dir / f"{name}.md"):
            # A committed copy is the team's, possibly customized: keep it.
            if any(t == path or path in t.parents for t in tracked):
                continue
            if path.exists() and _remove_dagayn_skill(path, name):
                removed += 1
    return removed


def _git_tracked(repo_root: Path, under: Path) -> list[Path]:
    """Paths git tracks under *under*; empty outside a git repository."""
    try:
        completed = subprocess.run(  # noqa: S603, S607 - fixed git command
            ["git", "-C", str(repo_root), "ls-files", "-z", "--", str(under)],
            capture_output=True,
            check=True,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError):
        return []
    return [
        repo_root / name for name in completed.stdout.decode("utf-8", "replace").split("\0") if name
    ]


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

    for name in _RETIRED_SKILLS:
        _remove_dagayn_skill(target_dir / name, name)
    for entry in sorted(source_dir.iterdir()):
        if not entry.is_dir() or not (entry / "SKILL.md").is_file():
            continue
        destination = target_dir / entry.name
        if destination.exists():
            shutil.rmtree(destination)
        shutil.copytree(entry, destination)
        target_skill = destination / "SKILL.md"
        write_text_atomic(
            target_skill,
            _render_skill_content(
                target_skill.read_text(encoding="utf-8"),
                skill_name=entry.name,
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

    Mirrors the source ``skills/`` tree as ``<name>/SKILL.md`` directories
    under the user home so the writing/reading-markdown-document skills (and
    the other dagayn skills) are available across all projects.
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
                write_text_atomic(
                    target_skill,
                    _render_skill_content(
                        target_skill.read_text(encoding="utf-8"),
                        skill_name=skill_dir.name,
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
