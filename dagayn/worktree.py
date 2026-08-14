"""Git worktree awareness for hooks, MCP config, and graph data.

Claude Code (``claude --worktree`` / the ``EnterWorktree`` tool) and Cursor
run agent sessions inside linked git worktrees. A worktree is a fresh
checkout: gitignored files such as ``.mcp.json``, ``.cursor/mcp.json`` and
the ``.dagayn/`` graph directory do **not** exist there. Without help, every
dagayn hook in a worktree session updates nothing and every MCP tool reports
a missing graph.

This module provides the primitives used to fix that:

* :func:`main_worktree_root` / :func:`is_linked_worktree` — locate the main
  checkout from any linked worktree.
* :func:`git_hooks_dir` — resolve the shared hooks directory, so
  ``dagayn install`` works when run from inside a worktree.
* :func:`seed_worktree_graph` — inherit the main checkout's graph by copying
  it into the worktree, so only the branch diff needs re-parsing.
* :func:`resolve_hook_repo` — resolve the repository an agent hook is acting
  on from the hook's JSON payload, for hosts that run hook scripts from a
  directory unrelated to the project (Cursor user hooks run from
  ``~/.cursor``).
"""

from __future__ import annotations

import json
import logging
import os
import shutil
import sqlite3
import subprocess  # nosec B404 — git metadata queries with fixed argv
from dataclasses import dataclass
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)

_GIT_TIMEOUT = 5

#: Set to ``0``/``false``/``no``/``off`` to disable graph inheritance.
SEED_ENV_VAR = "DAGAYN_WORKTREE_SEED"
#: Metadata flag set on a seeded copy: its per-file mtimes came from the
#: main checkout, so content must be verified once before the graph can be
#: described as matching this worktree.
SEEDED_NEEDS_VERIFY_KEY = "seeded_needs_content_verify"

_FALSEY = frozenset({"0", "false", "no", "off"})

#: Project-local, gitignored files each platform needs in order to reach the
#: dagayn MCP server, keyed by platform. A worktree checks out tracked files
#: only, so these have to be carried over explicitly. Patterns ending in
#: ``/**`` denote a whole directory.
PLATFORM_CONFIG_PATTERNS: dict[str, tuple[str, ...]] = {
    "claude": (".mcp.json", ".claude/skills/**"),
    "cursor": (".cursor/mcp.json",),
    "opencode": (".opencode.json",),
    "kiro": (".kiro/settings/mcp.json",),
    "qoder": (".qoder/mcp.json", ".qoder/skills/**"),
    "pi": (".pi/mcp.json",),
}

#: Hook payload keys that may carry the directory a hook applies to.
#: Ordered most-specific first. ``file_path`` is a file, not a directory —
#: :func:`resolve_hook_repo` handles that case separately.
_HOOK_DIR_KEYS = (
    "worktree_path",
    "worktreePath",
    "worktree",
    "workspace_root",
    "project_dir",
    "path",
    "directory",
    "dir",
    "cwd",
)


def _git(repo_root: Path, *args: str) -> str | None:
    """Run ``git -C <repo_root> <args>`` and return stripped stdout.

    Returns ``None`` when git is unavailable, times out, exits non-zero, or
    prints nothing — every caller treats that as "not a git repository".
    """
    try:
        result = subprocess.run(  # nosec B603 B607 — fixed argv, no shell
            ["git", "-C", str(repo_root), *args],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=_GIT_TIMEOUT,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if result.returncode != 0:
        return None
    return result.stdout.strip() or None


def _same_path(left: Path, right: Path) -> bool:
    """Compare two paths, tolerating symlinked temp dirs (``/var`` on macOS)."""
    try:
        return left.resolve() == right.resolve()
    except (OSError, RuntimeError):
        return left == right


def git_common_dir(repo_root: Path) -> Path | None:
    """Return the shared ``.git`` directory for *repo_root*.

    In a linked worktree this is the main checkout's ``.git`` directory, not
    the worktree's own ``.git/worktrees/<name>`` administrative directory.
    Returns ``None`` when *repo_root* is not inside a git repository.
    """
    out = _git(repo_root, "rev-parse", "--path-format=absolute", "--git-common-dir")
    if out is None:
        # git < 2.31 has no --path-format; the answer may be relative to cwd.
        out = _git(repo_root, "rev-parse", "--git-common-dir")
    if out is None:
        return None
    path = Path(out)
    return path if path.is_absolute() else (repo_root / path)


def git_hooks_dir(repo_root: Path) -> Path | None:
    """Return the directory git reads hooks from for *repo_root*.

    Honors ``core.hooksPath`` and resolves to the **shared** hooks directory
    so hooks installed from inside a linked worktree apply to every worktree
    of the repository. Falls back to ``<repo_root>/.git/hooks`` when git
    cannot be queried but a ``.git`` directory exists (non-git-backed test
    fixtures, git binary missing).
    """
    configured = _git(repo_root, "config", "--get", "core.hooksPath")
    if configured:
        hooks_path = Path(configured).expanduser()
        return hooks_path if hooks_path.is_absolute() else (repo_root / hooks_path)

    common = git_common_dir(repo_root)
    if common is not None:
        return common / "hooks"

    legacy = repo_root / ".git"
    if legacy.is_dir():
        return legacy / "hooks"
    return None


def is_gitignored(repo_root: Path, relative: str) -> bool:
    """Return True when *relative* is ignored by git inside *repo_root*.

    ``git check-ignore -q`` prints nothing and signals the answer through its
    exit code, so this cannot go through :func:`_git`.
    """
    try:
        result = subprocess.run(  # nosec B603 B607 — fixed argv, no shell
            ["git", "-C", str(repo_root), "check-ignore", "-q", "--", relative],
            capture_output=True,
            timeout=_GIT_TIMEOUT,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return False
    return result.returncode == 0


def main_worktree_root(repo_root: Path) -> Path | None:
    """Return the main checkout root for the repository containing *repo_root*.

    Returns ``None`` for non-git directories and for bare repositories (which
    have no main working tree).
    """
    common = git_common_dir(repo_root)
    if common is None:
        return None
    if common.name == ".git":
        return common.parent

    # Bare repo, or a non-standard git dir location: ask git for the list. The
    # first entry of a bare-backed repository is the bare git dir itself, which
    # has no working tree — returning it contradicted this function's own
    # contract, made a bare-repo worktree unable to inherit while telling the
    # user to "build in the main checkout" that has no files, and (without
    # CRG_DATA_DIR) created ``<repo>.git/.dagayn/`` inside the git directory.
    out = _git(repo_root, "worktree", "list", "--porcelain")
    if out:
        for line in out.splitlines():
            if not line.startswith("worktree "):
                continue
            candidate = Path(line[len("worktree ") :].strip())
            if not candidate.is_dir():
                continue
            if _same_path(candidate, common):
                # The bare git dir listed as its own "worktree".
                continue
            if not (candidate / ".git").exists():
                # No git link/dir of its own: not a working tree.
                continue
            return candidate
    return None


def is_linked_worktree(repo_root: Path) -> bool:
    """Return True when *repo_root* is a linked worktree, not the main checkout."""
    main = main_worktree_root(repo_root)
    if main is None:
        return False
    return not _same_path(main, repo_root)


def worktree_label(repo_root: Path) -> str | None:
    """Return a short human label for a linked worktree (its directory name)."""
    if not is_linked_worktree(repo_root):
        return None
    return repo_root.name


@dataclass(frozen=True)
class SeedResult:
    """Outcome of a :func:`seed_worktree_graph` attempt.

    ``status`` is ``"seeded"``, ``"skipped"``, or ``"failed"``. ``base_sha``
    is the commit the inherited graph was built at, which callers use as the
    incremental-update base so the worktree's branch diff is re-parsed.
    """

    status: str
    reason: str
    source: Path | None = None
    dest: Path | None = None
    base_sha: str | None = None

    @property
    def seeded(self) -> bool:
        return self.status == "seeded"


def seeding_disabled() -> bool:
    """Return True when graph inheritance is switched off by environment."""
    return os.environ.get(SEED_ENV_VAR, "").strip().lower() in _FALSEY


def read_graph_metadata(db_path: Path, key: str) -> str | None:
    """Return a metadata value from a graph database, or ``None`` if unreadable."""
    try:
        conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    except sqlite3.Error:
        return None
    try:
        row = conn.execute("SELECT value FROM metadata WHERE key = ?", (key,)).fetchone()
    except sqlite3.Error:
        return None
    finally:
        conn.close()
    return str(row[0]) if row and row[0] is not None else None


def graph_has_nodes(db_path: Path) -> bool:
    """Return True when *db_path* is a readable graph with at least one node.

    An empty SQLite stub (schema only, 0 nodes) is treated as absent so
    :func:`seed_worktree_graph` can still inherit from the main checkout.
    ``dagayn status`` opens :class:`~dagayn.graph.GraphStore`, which creates
    such a stub when ``graph.db`` is missing — existence alone must not block
    inheritance.
    """
    try:
        if not db_path.exists() or db_path.stat().st_size <= 0:
            return False
        conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    except (OSError, sqlite3.Error):
        return False
    try:
        row = conn.execute("SELECT 1 FROM nodes LIMIT 1").fetchone()
    except sqlite3.Error:
        return False
    finally:
        conn.close()
    return row is not None


def _copy_graph_db(source: Path, dest: Path, repo_root: Path) -> None:
    """Copy *source* to *dest* via the sqlite backup API, then re-root it.

    A plain file copy would miss the write-ahead log, producing a graph that
    is silently behind the source. The backup API reads a consistent snapshot
    including WAL content. The copy is written to a temporary file in the
    destination directory and moved into place so a crash or a concurrent
    reader never sees a partial database.
    """
    tmp = dest.with_name(f"{dest.name}.seed-{os.getpid()}.tmp")
    # ``finally`` covers the whole body: a failure inside ``backup()`` or the
    # metadata upsert used to leave a full-size copy of the graph behind on
    # every retry.
    try:
        src_conn = sqlite3.connect(f"file:{source}?mode=ro", uri=True)
        try:
            dst_conn = sqlite3.connect(str(tmp))
            try:
                src_conn.backup(dst_conn)
                # The graph stores repo-relative paths, but metadata records the
                # absolute root. Re-root it so the worktree copy is
                # self-describing even before the first update runs.
                dst_conn.execute(
                    "INSERT INTO metadata (key, value) VALUES ('repo_root', ?) "
                    "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    (str(repo_root),),
                )
                # The copy inherits whatever the parent held, including nodes an
                # edit hook indexed from the parent's *uncommitted* files. The
                # worktree starts on the same commit, so the catch-up diff
                # ``base..HEAD`` is empty and nothing would ever remove those
                # phantoms. This marker makes the first assessment verify content
                # regardless of the usual hash-candidate cap (a fresh checkout
                # moves every mtime, so the cap always bites there).
                dst_conn.execute(
                    "INSERT INTO metadata (key, value) VALUES (?, '1') "
                    "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    (SEEDED_NEEDS_VERIFY_KEY,),
                )
                dst_conn.commit()
            finally:
                dst_conn.close()
        finally:
            src_conn.close()
        os.replace(tmp, dest)
    finally:
        tmp.unlink(missing_ok=True)


def seed_worktree_graph(repo_root: Path) -> SeedResult:
    """Inherit the main checkout's graph into the linked worktree *repo_root*.

    Does nothing (``status="skipped"``) unless every condition holds:

    * graph inheritance is not disabled via :data:`SEED_ENV_VAR`;
    * *repo_root* is a linked worktree with a reachable main checkout;
    * the worktree has no populated graph yet and the main checkout has one.
      Empty schema-only stubs (0 nodes) created by ``dagayn status`` /
      :class:`~dagayn.graph.GraphStore` do **not** count as populated.

    A full ``dagayn build`` in a fresh worktree re-parses every file and
    re-computes every embedding. Inheriting the main graph turns that into a
    copy plus an incremental update over the branch diff.

    Both paths come from :func:`~dagayn.paths.db_path_for`, so this works under
    ``CRG_DATA_DIR`` too. That used to be skipped, because the variable was
    honored verbatim and the worktree therefore *shared* the main checkout's
    graph file; now each gets its own subdirectory and has to be seeded.
    """
    if seeding_disabled():
        return SeedResult("skipped", f"{SEED_ENV_VAR} disables graph inheritance")

    main = main_worktree_root(repo_root)
    if main is None:
        return SeedResult("skipped", "not inside a git worktree")
    if _same_path(main, repo_root):
        return SeedResult("skipped", "already in the main checkout")

    from .paths import db_path_for, get_db_path

    dest = db_path_for(repo_root)
    if graph_has_nodes(dest):
        return SeedResult("skipped", "worktree already has a graph", dest=dest)

    source = db_path_for(main)
    if not graph_has_nodes(source):
        # Existence is not having a graph: ``dagayn status``/``serve`` leave a
        # schema-only 0-node stub behind. Seeding one produced a worktree graph
        # that held nothing yet was then stamped as describing HEAD.
        return SeedResult(
            "skipped",
            f"main checkout has no graph at {source} — run 'dagayn build' there first",
            source=source,
            dest=dest,
        )

    base_sha = read_graph_metadata(source, "git_head_sha")
    try:
        dest = get_db_path(repo_root)
        dest.parent.mkdir(parents=True, exist_ok=True)
        _copy_graph_db(source, dest, repo_root)
    except (OSError, sqlite3.Error) as exc:
        logger.warning("Could not inherit graph from %s: %s", source, exc)
        return SeedResult("failed", f"copy failed: {exc}", source=source, dest=dest)

    logger.info("Inherited graph from %s into %s", source, dest)
    return SeedResult("seeded", "inherited graph from main checkout", source, dest, base_sha)


def config_pattern_target(pattern: str) -> str:
    """Return the path a :data:`PLATFORM_CONFIG_PATTERNS` entry refers to."""
    return pattern[: -len("/**")] if pattern.endswith("/**") else pattern


def copy_worktree_config(repo_root: Path, main_root: Path | None = None) -> list[str]:
    """Copy the main checkout's gitignored dagayn config into *repo_root*.

    Claude Code has ``.worktreeinclude`` and Cursor has
    ``.cursor/worktrees.json`` setup commands, but both need the files to be
    named somewhere. This performs the copy itself so a single cross-platform
    command works from either host, and so a worktree created with plain
    ``git worktree add`` can be fixed up after the fact.

    Existing files are never overwritten — a worktree may legitimately have
    diverging config. Returns the relative paths that were copied.
    """
    if main_root is None:
        main_root = main_worktree_root(repo_root)
    if main_root is None or _same_path(main_root, repo_root):
        return []

    copied: list[str] = []
    for patterns in PLATFORM_CONFIG_PATTERNS.values():
        for pattern in patterns:
            relative = config_pattern_target(pattern)
            source = main_root / relative
            dest = repo_root / relative
            if not source.exists() or dest.exists():
                continue
            try:
                dest.parent.mkdir(parents=True, exist_ok=True)
                if source.is_dir():
                    shutil.copytree(source, dest)
                else:
                    shutil.copy2(source, dest)
            except OSError as exc:
                logger.warning("Could not copy %s into the worktree: %s", relative, exc)
                continue
            copied.append(relative)
    return copied


def ensure_worktree_graph(repo_root: Path) -> SeedResult:
    """Seed the worktree graph if needed, never raising.

    Convenience wrapper for call sites (``serve`` startup, ``update`` /
    ``status`` hooks) that must not fail because inheritance was impossible.
    """
    try:
        return seed_worktree_graph(repo_root)
    except Exception as exc:  # noqa: BLE001 — best-effort convenience path
        logger.warning("Graph inheritance skipped: %s", exc)
        return SeedResult("failed", str(exc))


# --- Hook payload repository resolution ---


def _candidate_dirs(payload: Any) -> list[str]:
    """Collect directory-ish strings from a hook payload, best candidates first."""
    if not isinstance(payload, dict):
        return []

    candidates: list[str] = []

    def _add(value: Any) -> None:
        if isinstance(value, str) and value.strip():
            candidates.append(value.strip())

    # Nested tool payloads describe the worktree the host just entered.
    for container_key in ("tool_response", "tool_result", "tool_input", "toolInput"):
        container = payload.get(container_key)
        if isinstance(container, dict):
            for key in _HOOK_DIR_KEYS:
                _add(container.get(key))
        else:
            _add(container)

    for key in _HOOK_DIR_KEYS:
        _add(payload.get(key))

    roots = payload.get("workspace_roots")
    if isinstance(roots, list):
        for root in roots:
            _add(root)
        # Cursor sends objects in some versions: [{"path": "..."}]
        for root in roots:
            if isinstance(root, dict):
                _add(root.get("path"))

    return candidates


def _repo_root_of(path: Path) -> Path | None:
    """Return the git toplevel for *path*, or ``None`` when it has none."""
    directory = path if path.is_dir() else path.parent
    if not directory.exists():
        return None
    out = _git(directory, "rev-parse", "--show-toplevel")
    return Path(out) if out else None


def resolve_hook_repo(payload: Any, *, fallback_cwd: bool = True) -> Path | None:
    """Resolve the repository root an agent hook is acting on.

    Resolution order:

    1. directory-valued fields of the payload (including the nested
       ``tool_input`` / ``tool_response`` of a Claude Code ``EnterWorktree``
       call, and Cursor's ``workspace_roots``);
    2. the directory of ``file_path`` for file-edit hooks;
    3. the ``CLAUDE_PROJECT_DIR`` / ``CURSOR_PROJECT_DIR`` environment
       variables the hosts export;
    4. the current working directory, unless *fallback_cwd* is False.

    Each candidate is passed through ``git rev-parse --show-toplevel``, so a
    worktree path resolves to that worktree — never to the main checkout.
    """
    for candidate in _candidate_dirs(payload):
        root = _repo_root_of(Path(candidate).expanduser())
        if root is not None:
            return root

    if isinstance(payload, dict):
        file_path = payload.get("file_path") or payload.get("filePath")
        if isinstance(file_path, str) and file_path.strip():
            root = _repo_root_of(Path(file_path.strip()).expanduser())
            if root is not None:
                return root

    for env_var in ("CLAUDE_PROJECT_DIR", "CURSOR_PROJECT_DIR"):
        value = os.environ.get(env_var, "").strip()
        if value:
            root = _repo_root_of(Path(value).expanduser())
            if root is not None:
                return root

    if fallback_cwd:
        return _repo_root_of(Path.cwd())
    return None


def parse_hook_payload(raw: str) -> Any:
    """Parse hook JSON from stdin, returning ``None`` for anything unusable."""
    text = (raw or "").strip()
    if not text:
        return None
    try:
        return json.loads(text)
    except (json.JSONDecodeError, ValueError):
        return None
