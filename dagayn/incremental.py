"""Incremental graph update logic.

Detects changed files via git diff, re-parses only changed + impacted files,
and updates the graph accordingly. Also supports CLI invocation for hooks.
"""

from __future__ import annotations

import concurrent.futures
import fnmatch
import hashlib
import importlib.util
import json
import logging
import os
import re
import subprocess
import time
from pathlib import Path, PurePosixPath
from typing import Any, Callable, Optional

from .graph import GraphStore
from .parser import CodeParser
from .parser._base.types import EdgeInfo, NodeInfo
from .parser.dispatch import detect_language as _detect_parser_language

_MAX_PARSE_WORKERS = int(os.environ.get("CRG_PARSE_WORKERS", str(min(os.cpu_count() or 4, 8))))
_STORE_BATCH_SIZE = int(os.environ.get("DAGAYN_STORE_BATCH_SIZE", "128"))
_RUST_PARSE_BATCH_SIZE = int(os.environ.get("DAGAYN_RUST_PARSE_BATCH_SIZE", "500"))
_DEFAULT_BACKEND = "rust"

StoreBatch = list[tuple[str, list[Any], list[Any], str, int]]

logger = logging.getLogger(__name__)

# Per-worker singleton — initialised once per process by _init_worker().
_worker_parser: CodeParser | None = None


def _init_worker() -> None:
    """Initialise one CodeParser per worker process, avoiding repeated grammar loads."""
    global _worker_parser
    _worker_parser = CodeParser()


# Default ignore patterns (in addition to .gitignore).
#
# `<dir>/**` patterns are matched at any depth by _should_ignore, so
# `node_modules/**` also excludes `packages/app/node_modules/react/index.js`
# inside monorepos. See: #91
DEFAULT_IGNORE_PATTERNS = [
    ".dagayn/**",
    "node_modules/**",
    ".git/**",
    ".svn/**",
    "__pycache__/**",
    "*.pyc",
    ".venv/**",
    "venv/**",
    "dist/**",
    "build/**",
    ".next/**",
    "target/**",
    "dagayn/_vendor_grammars/**",
    ".hatch-vendor-grammars/**",
    # PHP / Laravel / Composer
    "vendor/**",
    "bootstrap/cache/**",
    "public/build/**",
    # Ruby / Bundler
    ".bundle/**",
    # Java / Kotlin / Gradle
    ".gradle/**",
    "*.jar",
    # Dart / Flutter
    ".dart_tool/**",
    ".pub-cache/**",
    # General
    "coverage/**",
    ".cache/**",
    "*.min.js",
    "*.min.css",
    "*.map",
    "*.lock",
    "package-lock.json",
    "yarn.lock",
    "*.db",
    "*.sqlite",
    "*.db-journal",
    "*.db-wal",
]


def find_svn_root(start: Path | None = None) -> Optional[Path]:
    """Walk up from start to find the SVN working copy root.

    For SVN 1.7+, there is a single ``.svn`` at the WC root.
    For older SVN, every directory has ``.svn`` — we return the topmost one
    found so that the WC root is correctly identified.
    """
    current = start or Path.cwd()
    candidate: Optional[Path] = None
    while current != current.parent:
        if (current / ".svn").exists():
            candidate = current
        current = current.parent
    if (current / ".svn").exists():
        candidate = current
    return candidate


def find_repo_root(
    start: Path | None = None,
    stop_at: Path | None = None,
) -> Optional[Path]:
    """Walk up from ``start`` to find the nearest ``.git`` directory or SVN working copy root.

    Args:
        start: Starting directory.  Defaults to ``Path.cwd()``.
        stop_at: Optional boundary — if provided, the walk examines
            ``stop_at`` for a ``.git`` directory and then stops without
            crossing above it.  Useful for tests that create a synthetic
            repo under ``tmp_path`` (so the walk does not accidentally
            climb into a developer's home-directory dotfiles repo) and
            for any production caller that wants to bound the ancestor
            walk — e.g. multi-repo orchestrators, CI containers with
            bind-mounted volumes, embedded sandboxes.  See #241.

    Returns:
        The first ancestor containing ``.git`` or an SVN working copy,
        or ``None`` if no ancestor up to and including ``stop_at`` (when
        set) or the filesystem root (when ``stop_at is None``) contains one.
    """
    current = start or Path.cwd()
    while current != current.parent:
        if (current / ".git").exists():
            return current
        if stop_at is not None and current == stop_at:
            return None
        current = current.parent
    if (current / ".git").exists():
        return current
    # No Git root found — try SVN
    return find_svn_root(start)


def detect_vcs(root: Path) -> str:
    """Return ``'git'``, ``'svn'``, or ``'none'`` based on VCS markers at *root*."""
    if (root / ".git").exists():
        return "git"
    if (root / ".svn").exists():
        return "svn"
    return "none"


def find_project_root(
    start: Path | None = None,
    stop_at: Path | None = None,
) -> Path:
    """Find the project root.

    Resolution order (highest precedence first):

    1. ``CRG_REPO_ROOT`` environment variable — explicit override for
       anyone scripting the CLI from outside the repo (CI jobs, daemons,
       multi-repo orchestrators). See: #155
    2. Git repository root via :func:`find_repo_root` from ``start``,
       honoring ``stop_at`` if provided.
    3. ``start`` itself (or cwd if no start given).

    ``stop_at`` is forwarded to :func:`find_repo_root` so callers that
    want to bound the ancestor walk (typically tests; see #241) can do so
    without having to call ``find_repo_root`` directly.
    """
    env_override = os.environ.get("CRG_REPO_ROOT", "").strip()
    if env_override:
        p = Path(env_override).expanduser().resolve()
        if p.exists():
            return p
    root = find_repo_root(start, stop_at=stop_at)
    if root:
        return root
    return start or Path.cwd()


def get_data_dir(repo_root: Path) -> Path:
    """Return the directory where this project's graph data lives.

    By default, ``<repo_root>/.dagayn``. If the
    ``CRG_DATA_DIR`` environment variable is set, it is used verbatim
    instead — letting you keep graphs outside the working tree (useful
    for ephemeral workspaces, Docker volumes, or shared caches). See: #155

    The directory is created if it does not already exist; an inner
    ``.gitignore`` (with ``*``) is written so any accidentally-nested
    files never get committed. Both are idempotent.
    """
    env_override = os.environ.get("CRG_DATA_DIR", "").strip()
    if env_override:
        data_dir = Path(env_override).expanduser().resolve()
    else:
        data_dir = repo_root / ".dagayn"

    data_dir.mkdir(parents=True, exist_ok=True)

    inner_gitignore = data_dir / ".gitignore"
    if not inner_gitignore.exists():
        try:
            # `encoding="utf-8"` is REQUIRED — the em-dash in the header is
            # U+2014 which falls outside cp1252.  On Windows, calling
            # write_text without an encoding silently uses the system default
            # codepage, producing a file that subsequently fails to decode as
            # UTF-8 (see issue #239).
            inner_gitignore.write_text(
                "# Auto-generated by dagayn — do not commit database files.\n"
                "# The graph.db contains absolute paths and code structure metadata.\n"
                "*\n",
                encoding="utf-8",
            )
        except OSError:
            # Data dir might be read-only (rare); that's OK, it's a best-effort guard.
            pass

    return data_dir


def get_db_path(repo_root: Path) -> Path:
    """Determine the database path for a repository.

    Respects ``CRG_DATA_DIR`` (see :func:`get_data_dir`). Migrates a
    legacy top-level ``.dagayn.db`` file into the new
    directory when it exists (WAL/SHM side-files are discarded).
    """
    crg_dir = get_data_dir(repo_root)
    new_db = crg_dir / "graph.db"

    # Migrate legacy database if present (only meaningful when the
    # legacy file sits at the repo root — if CRG_DATA_DIR is set we
    # skip the migration because there's no relationship between the
    # legacy location and the new one).
    legacy_db = repo_root / ".dagayn.db"
    if legacy_db.exists() and not new_db.exists():
        legacy_db.rename(new_db)
    # Discard stale WAL/SHM side-files from the old location
    for suffix in ("-wal", "-shm", "-journal"):
        side = repo_root / f".dagayn.db{suffix}"
        if side.exists():
            side.unlink()

    return new_db


def _make_repo_relative(path_str: str, repo_root: Path) -> str:
    path = Path(path_str)
    if not path.is_absolute():
        return str(path)
    root_candidates = [repo_root]
    try:
        resolved_root = repo_root.resolve()
    except (OSError, RuntimeError):
        resolved_root = None
    if resolved_root is not None and resolved_root not in root_candidates:
        root_candidates.append(resolved_root)
    try:
        resolved_path = path.resolve()
    except (OSError, RuntimeError):
        resolved_path = path
    for root in root_candidates:
        for candidate in (path, resolved_path):
            try:
                return str(candidate.relative_to(root))
            except ValueError:
                continue
    return str(path)


def _make_repo_relative_qualified(value: str, repo_root: Path) -> str:
    if "::" not in value:
        return _make_repo_relative(value, repo_root)
    file_path, rest = value.split("::", 1)
    return f"{_make_repo_relative(file_path, repo_root)}::{rest}"


_REPO_RELATIVE_QUALIFIED_EXTRA_KEYS = frozenset(
    {
        "parent_section",
    }
)


def _relativize_extra(value: Any, repo_root: Path) -> Any:
    if isinstance(value, dict):
        return {
            key: _make_repo_relative_qualified(extra_value, repo_root)
            if key in _REPO_RELATIVE_QUALIFIED_EXTRA_KEYS and isinstance(extra_value, str)
            else _relativize_extra(extra_value, repo_root)
            for key, extra_value in value.items()
        }
    if isinstance(value, list):
        return [_relativize_extra(item, repo_root) for item in value]
    return value


def _relativize_parsed_entities(
    nodes: list[NodeInfo], edges: list[EdgeInfo], repo_root: Path
) -> tuple[list[NodeInfo], list[EdgeInfo]]:
    rel_nodes = [
        NodeInfo(
            kind=node.kind,
            name=node.name,
            file_path=_make_repo_relative(node.file_path, repo_root),
            line_start=node.line_start,
            line_end=node.line_end,
            language=node.language,
            parent_name=node.parent_name,
            params=node.params,
            return_type=node.return_type,
            modifiers=node.modifiers,
            is_test=node.is_test,
            extra=_relativize_extra(node.extra, repo_root),
        )
        for node in nodes
    ]
    rel_edges = [
        EdgeInfo(
            kind=edge.kind,
            source=_make_repo_relative_qualified(edge.source, repo_root),
            target=_make_repo_relative_qualified(edge.target, repo_root),
            file_path=_make_repo_relative(edge.file_path, repo_root),
            line=edge.line,
            extra=_relativize_extra(edge.extra, repo_root),
        )
        for edge in edges
    ]
    return rel_nodes, rel_edges


def ensure_repo_gitignore_excludes_crg(repo_root: Path) -> str:
    """Ensure repo-level .gitignore excludes ``.dagayn/``.

    Returns one of:
    - ``created``: .gitignore was created with the entry
    - ``updated``: entry was appended to existing .gitignore
    - ``already-present``: no changes were needed
    """
    gitignore_path = repo_root / ".gitignore"
    existing = gitignore_path.read_text(encoding="utf-8") if gitignore_path.exists() else ""

    for raw_line in existing.splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if line == ".dagayn" or line.startswith(".dagayn/"):
            return "already-present"

    block = "# Added by dagayn\n.dagayn/\n"
    prefix = "\n" if existing and not existing.endswith("\n") else ""
    gitignore_path.write_text(existing + prefix + block, encoding="utf-8")

    if existing:
        return "updated"
    return "created"


def _load_ignore_patterns(repo_root: Path) -> list[str]:
    """Load ignore patterns from .dagaynignore file."""
    patterns = list(DEFAULT_IGNORE_PATTERNS)
    ignore_file = repo_root / ".dagaynignore"
    if ignore_file.exists():
        for line in ignore_file.read_text(encoding="utf-8", errors="replace").splitlines():
            line = line.strip()
            if line and not line.startswith("#"):
                patterns.append(line)
    return patterns


def _should_ignore(path: str, patterns: list[str]) -> bool:
    """Check if a path matches any ignore pattern.

    Handles nested occurrences of ``<dir>/**`` patterns: for example,
    ``node_modules/**`` also matches ``packages/app/node_modules/foo.js``
    inside monorepos. ``fnmatch`` alone treats ``*`` as not crossing ``/``
    and only matches the prefix, so we additionally test each path segment
    against the bare prefix of ``<dir>/**`` patterns. See: #91
    """
    # Direct fnmatch first (cheap)
    if any(fnmatch.fnmatch(path, p) for p in patterns):
        return True
    # Then: treat simple single-segment "dir/**" patterns as
    # "this directory at any depth".
    parts = PurePosixPath(path).parts
    for p in patterns:
        if not p.endswith("/**"):
            continue
        prefix = p[:-3]
        # Only single-segment dir patterns (no "/" inside the prefix)
        # qualify for nested matching.
        if "/" in prefix or not prefix:
            continue
        if prefix in parts:
            return True
    return False


def _is_binary(path: Path) -> bool:
    """Quick heuristic: check if file appears to be binary."""
    try:
        chunk = path.read_bytes()[:8192]
        return b"\x00" in chunk
    except (OSError, PermissionError):
        return True


_GIT_TIMEOUT = int(os.environ.get("CRG_GIT_TIMEOUT", "30"))  # seconds, configurable

# When True, `git ls-files --recurse-submodules` is used so that files
# inside git submodules are included in the graph.  Opt-in via env var;
# can also be overridden per-call through function parameters.
_RECURSE_SUBMODULES = os.environ.get("CRG_RECURSE_SUBMODULES", "").lower() in ("1", "true", "yes")


def _git_branch_info(repo_root: Path) -> tuple[str, str]:
    """Return (branch_name, head_sha) for the current repo state."""
    branch = ""
    sha = ""
    try:
        result = subprocess.run(
            ["git", "rev-parse", "--abbrev-ref", "HEAD"],
            capture_output=True,
            text=True,
            cwd=str(repo_root),
            timeout=_GIT_TIMEOUT,
        )
        if result.returncode == 0:
            branch = result.stdout.strip()
    except (subprocess.TimeoutExpired, FileNotFoundError):
        pass
    try:
        result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            cwd=str(repo_root),
            timeout=_GIT_TIMEOUT,
        )
        if result.returncode == 0:
            sha = result.stdout.strip()
    except (subprocess.TimeoutExpired, FileNotFoundError):
        pass
    return branch, sha


def _svn_revision_info(repo_root: Path) -> tuple[str, str]:
    """Return (branch_path, revision_str) for the current SVN working copy."""
    branch = ""
    rev = ""
    try:
        result = subprocess.run(
            ["svn", "info", "--non-interactive"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            cwd=str(repo_root),
            timeout=_GIT_TIMEOUT,
        )
        if result.returncode == 0:
            for line in result.stdout.splitlines():
                if line.startswith("URL: "):
                    url = line[5:].strip()
                    # Extract trunk/branches/tags segment from SVN URL
                    for marker in ("/branches/", "/tags/", "/trunk"):
                        if marker in url:
                            idx = url.index(marker)
                            branch = url[idx:].lstrip("/")
                            break
                    if not branch and url:
                        branch = url.rstrip("/").split("/")[-1]
                elif line.startswith("Revision: "):
                    rev = line[10:].strip()
    except (subprocess.TimeoutExpired, FileNotFoundError):
        pass
    return branch, rev


_SAFE_GIT_REF = re.compile(r"^[A-Za-z0-9_.~^/@{}\-]+$")
_SAFE_SVN_REV = re.compile(r"^r?\d+(:r?\d+|:HEAD|:BASE|:COMMITTED)?$", re.IGNORECASE)


def _store_vcs_metadata(repo_root: Path, store: "GraphStore") -> None:
    """Persist VCS branch/revision info into the graph metadata table."""
    vcs = detect_vcs(repo_root)
    if vcs == "git":
        branch, sha = _git_branch_info(repo_root)
        if branch:
            store.set_metadata("git_branch", branch)
        if sha:
            store.set_metadata("git_head_sha", sha)
    elif vcs == "svn":
        branch, rev = _svn_revision_info(repo_root)
        if branch:
            store.set_metadata("svn_branch", branch)
        if rev:
            store.set_metadata("svn_revision", rev)


def get_changed_files(repo_root: Path, base: str = "HEAD~1") -> list[str]:
    """Get list of changed files via git diff or svn status.

    For SVN working copies the *base* parameter is ignored; modified/added/
    deleted files are detected from ``svn status``.  Pass an SVN revision
    range (e.g. ``"r100:HEAD"``) as *base* to compare against a specific
    revision instead.
    """
    if detect_vcs(repo_root) == "svn":
        return _get_svn_changed_files(repo_root, base if _SAFE_SVN_REV.match(base) else None)
    # Git path
    if not _SAFE_GIT_REF.match(base):
        logger.warning("Invalid git ref rejected: %s", base)
        return []
    try:
        result = subprocess.run(
            ["git", "diff", "--name-only", base, "--"],
            capture_output=True,
            text=True,
            cwd=str(repo_root),
            timeout=_GIT_TIMEOUT,
        )
        if result.returncode != 0:
            # Fallback: try diff against empty tree (initial commit)
            result = subprocess.run(
                ["git", "diff", "--name-only", "--cached"],
                capture_output=True,
                text=True,
                cwd=str(repo_root),
                timeout=_GIT_TIMEOUT,
            )
        files = [f.strip() for f in result.stdout.splitlines() if f.strip()]
        return files
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return []


def _get_svn_changed_files(repo_root: Path, rev_range: str | None = None) -> list[str]:
    """Return changed files in an SVN working copy.

    When *rev_range* is given (e.g. ``"r100:HEAD"``), ``svn diff --summarize``
    is used to list files changed between those revisions.  Otherwise
    ``svn status`` reports working-copy modifications.
    """
    try:
        if rev_range:
            result = subprocess.run(
                ["svn", "diff", "--summarize", "--non-interactive", "-r", rev_range],
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                cwd=str(repo_root),
                timeout=_GIT_TIMEOUT,
            )
            if result.returncode != 0:
                logger.warning(
                    "svn diff --summarize failed (rc=%d): %s",
                    result.returncode,
                    result.stderr[:200],
                )
                return []
            files = []
            for line in result.stdout.splitlines():
                # Format: "M       path/to/file"  (first char is status)
                if len(line) >= 2 and line[0] in ("M", "A", "D"):
                    files.append(line[1:].strip())
            return files
        else:
            result = subprocess.run(
                ["svn", "status", "--non-interactive"],
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                cwd=str(repo_root),
                timeout=_GIT_TIMEOUT,
            )
            files = []
            for line in result.stdout.splitlines():
                if len(line) < 2:
                    continue
                status_char = line[0]
                # M=modified, A=added, D=deleted, R=replaced, C=conflicted
                if status_char in ("M", "A", "D", "R", "C"):
                    # SVN status: 8 fixed-width columns then the path
                    path = line[8:].strip() if len(line) > 8 else line[1:].strip()
                    files.append(path)
            return files
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return []


def get_staged_and_unstaged(repo_root: Path) -> list[str]:
    """Get all modified files (staged + unstaged + untracked)."""
    if detect_vcs(repo_root) == "svn":
        return _get_svn_changed_files(repo_root)
    try:
        result = subprocess.run(
            ["git", "status", "--porcelain"],
            capture_output=True,
            text=True,
            cwd=str(repo_root),
            timeout=_GIT_TIMEOUT,
        )
        files = []
        for line in result.stdout.splitlines():
            if len(line) > 3:
                entry = line[3:].strip()
                # Handle renamed files: "R  old -> new"
                if " -> " in entry:
                    entry = entry.split(" -> ", 1)[1]
                files.append(entry)
        return files
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return []


def get_all_tracked_files(
    repo_root: Path,
    recurse_submodules: bool | None = None,
) -> list[str]:
    """Get all files tracked by git or svn.

    Args:
        repo_root: Repository root directory.
        recurse_submodules: If True, pass ``--recurse-submodules`` to
            ``git ls-files`` so that files inside git submodules are
            included.  When *None* (default), falls back to the
            ``CRG_RECURSE_SUBMODULES`` environment variable.
            (Ignored for SVN working copies.)
    """
    if detect_vcs(repo_root) == "svn":
        return _get_svn_all_tracked_files(repo_root)

    if recurse_submodules is None:
        recurse_submodules = _RECURSE_SUBMODULES

    cmd = ["git", "ls-files"]
    if recurse_submodules:
        cmd.append("--recurse-submodules")

    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            cwd=str(repo_root),
            timeout=_GIT_TIMEOUT,
        )
        return [f.strip() for f in result.stdout.splitlines() if f.strip()]
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return []


def _get_svn_all_tracked_files(repo_root: Path) -> list[str]:
    """Return SVN-versioned files by walking the working copy.

    Uses ``svn list -R`` to get the server-side file list, falling back to
    a filesystem walk (which is also the fallback in :func:`collect_all_files`).
    """
    try:
        result = subprocess.run(
            ["svn", "list", "--recursive", "--non-interactive"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            cwd=str(repo_root),
            timeout=60,  # svn list queries the server
        )
        if result.returncode == 0:
            # svn list returns paths relative to the WC URL; directories end with "/"
            files = [
                f.strip()
                for f in result.stdout.splitlines()
                if f.strip() and not f.strip().endswith("/")
            ]
            if files:
                return files
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass
    # Fallback: let collect_all_files do a filesystem walk
    return []


def collect_all_files(
    repo_root: Path,
    recurse_submodules: bool | None = None,
) -> list[str]:
    """Collect all parseable files in the repo, respecting ignore patterns.

    Args:
        repo_root: Repository root directory.
        recurse_submodules: If True, include files from git submodules.
            When *None*, falls back to ``CRG_RECURSE_SUBMODULES`` env var.
    """
    if _rust_backend_enabled() and detect_vcs(repo_root) != "svn":
        try:
            from dagayn._core import collect_parseable_files

            return collect_parseable_files(repo_root, recurse_submodules)
        except (ImportError, RuntimeError, TypeError, ValueError) as exc:
            raise RuntimeError(
                "Rust file discovery requires dagayn._core. "
                "Install a wheel with the native extension or rebuild from source."
            ) from exc

    ignore_patterns = _load_ignore_patterns(repo_root)
    parser = CodeParser()
    files = []
    # Prefer git ls-files for tracked files
    tracked = get_all_tracked_files(repo_root, recurse_submodules)
    if tracked:
        candidates = tracked
    else:
        # Fallback: walk directory
        candidates = [str(p.relative_to(repo_root)) for p in repo_root.rglob("*") if p.is_file()]

    for rel_path in candidates:
        if _should_ignore(rel_path, ignore_patterns):
            continue
        full_path = repo_root / rel_path
        if not full_path.is_file():
            continue
        if full_path.is_symlink():
            continue
        if parser.detect_language(full_path) is None:
            continue
        if _is_binary(full_path):
            continue
        files.append(rel_path)

    return files


_MAX_DEPENDENT_HOPS = int(os.environ.get("CRG_DEPENDENT_HOPS", "2"))
_MAX_DEPENDENT_FILES = 500


def _single_hop_dependents(store: GraphStore, file_path: str) -> set[str]:
    """Find files that directly depend on *file_path* (single hop)."""
    return _batch_hop_dependents(store, {file_path})


def _batch_hop_dependents(store: GraphStore, frontier: set[str]) -> set[str]:
    """Find all files that directly depend on any file in *frontier* (batched).

    Replaces N calls to ``_single_hop_dependents`` with 2-3 SQL queries
    regardless of frontier size.
    """
    if not frontier:
        return set()

    rust_get = getattr(store, "get_direct_dependents", None)
    if callable(rust_get):
        return set(rust_get(list(frontier))) - frontier

    dependents: set[str] = set()
    # Include normalized path forms to match get_edges_by_target behavior.
    fp_keys: list[str] = []
    for fp in frontier:
        fp_keys.append(fp)
        norm = store._normalize_qualified_key(fp)
        if norm != fp:
            fp_keys.append(norm)

    batch_size = 450

    # 1. File-level IMPORTS_FROM: edges where target_qualified is a frontier file path.
    for i in range(0, len(fp_keys), batch_size):
        chunk = fp_keys[i : i + batch_size]
        placeholders = ",".join("?" for _ in chunk)
        rows = store._conn.execute(  # nosec B608
            f"SELECT file_path FROM edges"
            f" WHERE target_qualified IN ({placeholders}) AND kind = 'IMPORTS_FROM'",
            chunk,
        ).fetchall()
        for row in rows:
            dependents.add(row["file_path"])

    # 2. Node-level: collect QNs for all frontier files in one query.
    fp_list = list(frontier)
    all_node_qns: list[str] = []
    for i in range(0, len(fp_list), batch_size):
        chunk = fp_list[i : i + batch_size]
        placeholders = ",".join("?" for _ in chunk)
        rows = store._conn.execute(  # nosec B608
            f"SELECT qualified_name FROM nodes WHERE file_path IN ({placeholders})",
            chunk,
        ).fetchall()
        all_node_qns.extend(row["qualified_name"] for row in rows)

    # 3. Batch incoming edges for all node QNs in one call.
    if all_node_qns:
        _, incoming = store.get_edges_by_endpoints(all_node_qns)
        for node_edges in incoming.values():
            for e in node_edges:
                if e.kind in ("CALLS", "IMPORTS_FROM", "INHERITS", "IMPLEMENTS"):
                    dependents.add(e.file_path)

    dependents -= frontier
    return dependents


class DependentList(list):
    """A ``list[str]`` with a ``.truncated`` flag.

    When :func:`find_dependents` hits ``_MAX_DEPENDENT_FILES`` it truncates
    the result and sets ``truncated = True`` so callers can distinguish a
    complete expansion from a capped one.  See issue #261.

    This is a transparent ``list`` subclass — existing callers that iterate,
    ``len()``, or slice continue to work unchanged; only callers that
    specifically check ``.truncated`` benefit from the signal.
    """

    truncated: bool

    def __init__(self, items: list, *, truncated: bool = False) -> None:
        super().__init__(items)
        self.truncated = truncated


def find_dependents(
    store: GraphStore,
    file_path: str,
    max_hops: int = _MAX_DEPENDENT_HOPS,
) -> DependentList:
    """Find files that import from or depend on the given file.

    Performs up to *max_hops* iterations of expansion (default 2).
    Stops early if the total exceeds 500 files.

    Returns a :class:`DependentList` — a regular ``list[str]`` that also
    carries a ``.truncated`` flag.  When ``truncated is True`` the
    returned list is capped at ``_MAX_DEPENDENT_FILES`` and the full
    set of dependents was not explored.  See issue #261.
    """
    return find_dependents_for_files(store, [file_path], max_hops=max_hops)


def find_dependents_for_files(
    store: GraphStore,
    file_paths: list[str] | set[str],
    max_hops: int = _MAX_DEPENDENT_HOPS,
) -> DependentList:
    """Find files that depend on any file in *file_paths*.

    Performs multi-source expansion so incremental updates with many changed
    files pay one batched traversal per hop instead of one traversal per file.
    """
    roots = set(file_paths)
    if not roots:
        return DependentList([])
    all_dependents: set[str] = set()
    visited: set[str] = set(roots)
    frontier: set[str] = set(roots)
    for _hop in range(max_hops):
        new_deps = _batch_hop_dependents(store, frontier) - visited
        all_dependents.update(new_deps)
        visited.update(new_deps)
        frontier = new_deps
        if not frontier:
            break
        if len(all_dependents) > _MAX_DEPENDENT_FILES:
            logger.warning(
                "Dependent expansion capped at %d files for %d roots",
                len(all_dependents),
                len(roots),
            )
            return DependentList(
                list(all_dependents)[:_MAX_DEPENDENT_FILES],
                truncated=True,
            )
    return DependentList(list(all_dependents))


def _parse_single_file(
    args: tuple[str, str],
) -> tuple[str, list, list, str | None, str, int]:
    """Parse one file in a worker process.

    Returns ``(rel_path, nodes, edges, error_or_none, file_hash, mtime_ns)``.
    Must be a module-level function so ``ProcessPoolExecutor`` can
    serialise it across processes.
    """
    rel_path, repo_root_str = args
    abs_path = Path(repo_root_str) / rel_path
    try:
        mtime_ns = int(abs_path.stat().st_mtime_ns)
        raw = abs_path.read_bytes()
        fhash = hashlib.sha256(raw).hexdigest()
        rust_parsed = _parse_with_rust_if_enabled(rel_path, raw)
        if rust_parsed is not None:
            nodes, edges = rust_parsed
            return (rel_path, nodes, edges, None, fhash, mtime_ns)
        parser = _worker_parser if _worker_parser is not None else CodeParser()
        nodes, edges = parser.parse_bytes(abs_path, raw)
        return (rel_path, nodes, edges, None, fhash, mtime_ns)
    except Exception as e:
        return (rel_path, [], [], str(e), "", 0)


def _parse_single_python_file(
    args: tuple[str, str],
) -> tuple[str, list, list, str | None, str, int]:
    """Parse one file known not to be owned by the Rust parser."""
    rel_path, repo_root_str = args
    abs_path = Path(repo_root_str) / rel_path
    try:
        mtime_ns = int(abs_path.stat().st_mtime_ns)
        raw = abs_path.read_bytes()
        fhash = hashlib.sha256(raw).hexdigest()
        parser = _worker_parser if _worker_parser is not None else CodeParser()
        nodes, edges = parser.parse_bytes(abs_path, raw)
        return (rel_path, nodes, edges, None, fhash, mtime_ns)
    except Exception as e:
        return (rel_path, [], [], str(e), "", 0)


def _parse_single_python_file_compact(
    args: tuple[str, str],
) -> tuple[str, list, list, str | None, str, int]:
    """Parse one Python-owned file and return Rust compact store entities."""
    rel_path, repo_root_str = args
    abs_path = Path(repo_root_str) / rel_path
    try:
        mtime_ns = int(abs_path.stat().st_mtime_ns)
        raw = abs_path.read_bytes()
        fhash = hashlib.sha256(raw).hexdigest()
        parser = _worker_parser if _worker_parser is not None else CodeParser()
        nodes, edges = parser.parse_bytes(abs_path, raw)
        nodes, edges = _relativize_parsed_entities(nodes, edges, Path(repo_root_str))
        nodes = _serialize_nodes(nodes)
        edges = _serialize_edges(edges)
        return (rel_path, nodes, edges, None, fhash, mtime_ns)
    except Exception as e:
        return (rel_path, [], [], str(e), "", 0)


def _filter_incremental_candidates(
    repo_root: Path,
    rel_paths: set[str],
    ignore_patterns: list[str],
) -> tuple[list[str], list[str]]:
    """Return ``(parseable_files, removed_files)`` for incremental update."""
    if _rust_backend_enabled():
        try:
            from dagayn._core import filter_incremental_candidates

            return filter_incremental_candidates(
                repo_root,
                list(rel_paths),
                ignore_patterns,
            )
        except (ImportError, RuntimeError, TypeError, ValueError) as exc:
            raise RuntimeError(
                "Rust incremental candidate filtering requires dagayn._core. "
                "Install a wheel with the native extension or rebuild from source."
            ) from exc

    existing_files: list[str] = []
    removed_files: list[str] = []
    for rel_path in rel_paths:
        if _should_ignore(rel_path, ignore_patterns):
            continue
        abs_path = repo_root / rel_path
        if not abs_path.is_file():
            removed_files.append(rel_path)
            continue
        existing_files.append(rel_path)

    parser = CodeParser()
    candidates = []
    for rel_path in existing_files:
        if parser.detect_language(repo_root / rel_path) is not None:
            candidates.append(rel_path)
    return candidates, removed_files


def _classify_python_changed_files(
    repo_root: Path,
    file_paths: list[str],
    file_meta: dict[str, tuple[str, int]],
) -> tuple[list[str], list[tuple[int, str]]]:
    """Return content-changed Python-owned files and mtime-only updates."""
    changed_files: list[str] = []
    mtime_only_updates: list[tuple[int, str]] = []
    for rel_path in file_paths:
        abs_path = repo_root / rel_path
        try:
            cur_mtime_ns = int(abs_path.stat().st_mtime_ns)
            meta = file_meta.get(rel_path)
            if meta and meta[1] == cur_mtime_ns:
                continue
            raw = abs_path.read_bytes()
            fhash = hashlib.sha256(raw).hexdigest()
            if meta and meta[0] == fhash:
                mtime_only_updates.append((cur_mtime_ns, rel_path))
                continue
        except (OSError, PermissionError):
            pass
        changed_files.append(rel_path)
    return changed_files, mtime_only_updates


def _get_file_meta_for_candidates(
    store: GraphStore,
    file_paths: list[str],
) -> dict[str, tuple[str, int]]:
    """Return stored file metadata for only the requested paths."""
    if not file_paths:
        return {}
    if hasattr(store, "get_file_meta_for_files"):
        return store.get_file_meta_for_files(file_paths)
    if hasattr(store, "get_file_meta_map"):
        return store.get_file_meta_map()
    return {path: (fhash, 0) for path, fhash in store.get_file_hashes(file_paths).items()}


def _callable_store_attr(store: GraphStore, name: str) -> Callable[..., Any] | None:
    attr = getattr(store, name, None)
    return attr if callable(attr) else None


class _StoreBulkLoad:
    def __init__(self, store: GraphStore) -> None:
        self._begin = _callable_store_attr(store, "begin_bulk_load")
        self._finish = _callable_store_attr(store, "finish_bulk_load")
        self._active = False

    def __enter__(self) -> None:
        if self._begin is not None and self._finish is not None:
            self._begin()
            self._active = True

    def __exit__(self, exc_type: object, exc: object, tb: object) -> None:
        if self._active and self._finish is not None:
            self._finish()


def _flush_store_batch(store: GraphStore, batch: StoreBatch) -> None:
    """Write parsed file results through one store call.

    The Rust backend is intentionally crossed at batch granularity so PyO3
    overhead is paid per DB write phase chunk, not once for each parsed file.
    """
    if not batch:
        return
    store_file_batch_json = _callable_store_attr(store, "store_file_batch_json")
    if store_file_batch_json is not None:
        store_file_batch_json(_serialize_store_batch(batch))
    else:
        store.store_file_batch(batch)
    batch.clear()


def _serialize_store_batch(batch: StoreBatch) -> str:
    """Serialize parsed graph data in a compact Rust-owned wire format."""
    return json.dumps(
        [
            (
                file_path,
                _serialize_nodes(nodes),
                _serialize_edges(edges),
                fhash,
                mtime_ns,
            )
            for file_path, nodes, edges, fhash, mtime_ns in batch
        ],
        separators=(",", ":"),
    )


def _serialize_nodes(nodes: list[Any]) -> list[Any]:
    if _is_compact_entities(nodes):
        return nodes
    return [
        (
            n.kind,
            n.name,
            n.file_path,
            n.line_start,
            n.line_end,
            n.language,
            n.parent_name,
            n.params,
            n.return_type,
            n.modifiers,
            n.is_test,
            n.extra or {},
        )
        for n in nodes
    ]


def _serialize_edges(edges: list[Any]) -> list[Any]:
    if _is_compact_entities(edges):
        return edges
    return [
        (
            e.kind,
            e.source,
            e.target,
            e.file_path,
            e.line,
            e.extra or {},
        )
        for e in edges
    ]


def _is_compact_entities(entities: list[Any]) -> bool:
    return bool(entities) and isinstance(entities[0], (list, tuple))


def _uses_compact_entities(nodes: list[Any], edges: list[Any]) -> bool:
    return _is_compact_entities(nodes) or _is_compact_entities(edges)


def _backend_selection() -> str:
    return os.environ.get("DAGAYN_BACKEND", _DEFAULT_BACKEND).strip().lower()


def _rust_backend_explicitly_requested() -> bool:
    return os.environ.get("DAGAYN_BACKEND", "").strip().lower() == "rust"


def _rust_backend_available() -> bool:
    return importlib.util.find_spec("dagayn._core") is not None


def _rust_backend_enabled() -> bool:
    return _backend_selection() == "rust"


def _rust_parser_backend_enabled(store: GraphStore | None = None) -> bool:
    if not _rust_backend_enabled():
        return False
    if store is None:
        return True
    return (
        _callable_store_attr(store, "store_rust_owned_files") is not None
        or _callable_store_attr(store, "store_file_batch_json") is not None
    )


def _rust_parser_owns_path(rel_path: str, repo_root: Path | None = None) -> bool:
    lower = rel_path.lower()
    if lower.endswith(
        (
            ".md",
            ".markdown",
            ".tf",
            ".tfvars",
            ".rs",
            ".py",
            ".ipynb",
            ".js",
            ".jsx",
            ".mjs",
            ".ts",
            ".tsx",
            ".astro",
            ".sh",
            ".bash",
            ".zsh",
            ".ksh",
            ".go",
            ".java",
            ".rb",
            ".cs",
            ".php",
            ".kt",
            ".kts",
            ".scala",
            ".sol",
            ".dart",
            ".lua",
            ".luau",
            ".c",
            ".h",
            ".xs",
            ".cpp",
            ".cc",
            ".cxx",
            ".hpp",
            ".m",
            ".ex",
            ".exs",
            ".gd",
            ".r",
            ".jl",
            ".pl",
            ".pm",
            ".t",
            ".vue",
            ".svelte",
            ".zig",
            ".ps1",
            ".psm1",
            ".psd1",
            ".swift",
        )
    ):
        return True
    if PurePosixPath(rel_path).suffix or repo_root is None:
        return False
    return _detect_parser_language(repo_root / rel_path) in {
        "bash",
        "python",
        "javascript",
        "ruby",
        "perl",
        "lua",
        "r",
        "php",
    }


def _split_rust_parser_files(
    rel_paths: list[str],
    repo_root: Path | None = None,
    store: GraphStore | None = None,
) -> tuple[list[str], list[str]]:
    if not _rust_parser_backend_enabled(store):
        return [], rel_paths
    rust_files: list[str] = []
    python_files: list[str] = []
    for rel_path in rel_paths:
        if _rust_parser_owns_path(rel_path, repo_root):
            rust_files.append(rel_path)
        else:
            python_files.append(rel_path)
    return rust_files, python_files


def _store_rust_parse_batches(
    repo_root: Path,
    store: GraphStore,
    rel_paths: list[str],
) -> tuple[int, int, list[dict[str, str]]]:
    if not rel_paths:
        return 0, 0, []
    store_rust_owned_files = _callable_store_attr(store, "store_rust_owned_files")
    if store_rust_owned_files is not None:
        total_nodes = 0
        total_edges = 0
        errors: list[dict[str, str]] = []
        for idx in range(0, len(rel_paths), _RUST_PARSE_BATCH_SIZE):
            chunk = rel_paths[idx : idx + _RUST_PARSE_BATCH_SIZE]
            try:
                node_count, edge_count, raw_errors = store_rust_owned_files(
                    repo_root,
                    chunk,
                )
            except (RuntimeError, TypeError, ValueError) as exc:
                errors.extend({"file": rel_path, "error": str(exc)} for rel_path in chunk)
                continue
            total_nodes += int(node_count)
            total_edges += int(edge_count)
            errors.extend(
                {"file": str(file_path), "error": str(error)} for file_path, error in raw_errors
            )
        return total_nodes, total_edges, errors
    store_file_batch_json = _callable_store_attr(store, "store_file_batch_json")
    if store_file_batch_json is None:
        raise RuntimeError("Rust parser batch requires a GraphStore with store_file_batch_json")
    try:
        from dagayn._core import parse_rust_owned_files_compact_json
    except ImportError as exc:
        raise RuntimeError(
            "DAGAYN_BACKEND=rust was requested, but dagayn._core is not installed."
        ) from exc

    total_nodes = 0
    total_edges = 0
    errors: list[dict[str, str]] = []
    for idx in range(0, len(rel_paths), _RUST_PARSE_BATCH_SIZE):
        chunk = rel_paths[idx : idx + _RUST_PARSE_BATCH_SIZE]
        try:
            payload = json.loads(parse_rust_owned_files_compact_json(repo_root, chunk))
        except (RuntimeError, TypeError, ValueError, json.JSONDecodeError) as exc:
            errors.extend({"file": rel_path, "error": str(exc)} for rel_path in chunk)
            continue
        batch = payload.get("batch", [])
        for raw_error in payload.get("errors", []):
            if isinstance(raw_error, list | tuple) and len(raw_error) >= 2:
                errors.append({"file": str(raw_error[0]), "error": str(raw_error[1])})
            else:
                errors.append({"file": "", "error": str(raw_error)})
        if not batch:
            continue
        batch_with_mtime = [
            (
                item[0],
                item[1],
                item[2],
                item[3],
                int((repo_root / item[0]).stat().st_mtime_ns),
            )
            if len(item) == 4
            else item
            for item in batch
        ]
        store_file_batch_json(json.dumps(batch_with_mtime, separators=(",", ":")))
        total_nodes += sum(len(item[1]) for item in batch)
        total_edges += sum(len(item[2]) for item in batch)
    return total_nodes, total_edges, errors


def _parse_with_rust_if_enabled(
    rel_path: str,
    source: bytes,
) -> tuple[list[Any], list[Any]] | None:
    if not _rust_backend_enabled():
        return None
    lowered = rel_path.lower()
    parser_name: str
    parser_fn_name: str
    if lowered.endswith((".md", ".markdown")):
        parser_name = "Markdown"
        parser_fn_name = "parse_markdown_compact_json"
    elif lowered.endswith((".tf", ".tfvars")):
        parser_name = "Terraform"
        parser_fn_name = "parse_terraform_compact_json"
    elif lowered.endswith(".rs"):
        parser_name = "Rust"
        parser_fn_name = "parse_rust_compact_json"
    else:
        return None
    try:
        import dagayn._core as rust_core

        parser_fn = getattr(rust_core, parser_fn_name)
        nodes, edges = json.loads(parser_fn(rel_path, source))
        return nodes, edges
    except (
        AttributeError,
        ImportError,
        RuntimeError,
        TypeError,
        ValueError,
        json.JSONDecodeError,
    ) as exc:
        raise RuntimeError(f"Rust {parser_name} parser unavailable for {rel_path}: {exc}") from exc


def _queue_store_file(
    store: GraphStore,
    batch: StoreBatch,
    rel_path: str,
    nodes: list[Any],
    edges: list[Any],
    fhash: str,
    mtime_ns: int,
) -> None:
    batch.append((rel_path, nodes, edges, fhash, mtime_ns))
    if len(batch) >= _STORE_BATCH_SIZE:
        _flush_store_batch(store, batch)


def full_build(
    repo_root: Path,
    store: GraphStore,
    recurse_submodules: bool | None = None,
) -> dict:
    """Full rebuild of the entire graph.

    Args:
        repo_root: Repository root directory.
        store: Graph database store.
        recurse_submodules: If True, include files from git submodules.
            When *None*, falls back to ``CRG_RECURSE_SUBMODULES`` env var.
    """
    repo_root = repo_root.resolve()
    store.set_metadata("repo_root", str(repo_root))
    files = collect_all_files(repo_root, recurse_submodules)

    # Purge stale data from files no longer on disk
    existing_files = set(store.get_all_files())
    current_rel = set(files)
    stale_files = existing_files - current_rel
    store.remove_files_data(list(stale_files))
    # Ensure deletions are persisted before store_file_nodes_edges()
    # starts its own explicit transaction via BEGIN IMMEDIATE.
    if stale_files:
        store.commit()

    total_nodes = 0
    total_edges = 0
    errors = []
    file_count = len(files)

    with _StoreBulkLoad(store):
        use_serial = os.environ.get("CRG_SERIAL_PARSE", "") == "1"
        rust_files, python_files = _split_rust_parser_files(files, repo_root, store)
        if rust_files:
            rust_nodes, rust_edges, rust_errors = _store_rust_parse_batches(
                repo_root,
                store,
                rust_files,
            )
            total_nodes += rust_nodes
            total_edges += rust_edges
            errors.extend(rust_errors)
            logger.info("Progress: %d/%d files parsed", len(rust_files), file_count)

        if python_files:
            if use_serial or len(python_files) < 8:
                # Serial fallback (for debugging or tiny repos)
                batch: StoreBatch = []
                parser = CodeParser()
                for offset, rel_path in enumerate(python_files, 1):
                    i = len(rust_files) + offset
                    full_path = repo_root / rel_path
                    try:
                        mtime_ns = int(full_path.stat().st_mtime_ns)
                        source = full_path.read_bytes()
                        fhash = hashlib.sha256(source).hexdigest()
                        nodes, edges = parser.parse_bytes(full_path, source)
                        nodes, edges = _relativize_parsed_entities(nodes, edges, repo_root)
                        _queue_store_file(store, batch, rel_path, nodes, edges, fhash, mtime_ns)
                        total_nodes += len(nodes)
                        total_edges += len(edges)
                    except (OSError, PermissionError) as e:
                        errors.append({"file": rel_path, "error": str(e)})
                    except Exception as e:
                        logger.warning("Error parsing %s: %s", rel_path, e)
                        errors.append({"file": rel_path, "error": str(e)})
                    if i % 50 == 0 or i == file_count:
                        logger.info("Progress: %d/%d files parsed", i, file_count)
                _flush_store_batch(store, batch)
            else:
                # Parallel parsing — store calls remain serial (SQLite single-writer)
                args_list = [(rel_path, str(repo_root)) for rel_path in python_files]
                batch: StoreBatch = []
                parse_worker = (
                    _parse_single_python_file_compact
                    if _callable_store_attr(store, "store_file_batch_json") is not None
                    else _parse_single_python_file
                )
                with concurrent.futures.ProcessPoolExecutor(
                    max_workers=_MAX_PARSE_WORKERS,
                    initializer=_init_worker,
                ) as executor:
                    for i, (rel_path, nodes, edges, error, fhash, mtime_ns) in enumerate(
                        executor.map(parse_worker, args_list, chunksize=20),
                        len(rust_files) + 1,
                    ):
                        if error:
                            logger.warning("Error parsing %s: %s", rel_path, error)
                            errors.append({"file": rel_path, "error": error})
                            continue
                        if not _uses_compact_entities(nodes, edges):
                            nodes, edges = _relativize_parsed_entities(nodes, edges, repo_root)
                        _queue_store_file(store, batch, rel_path, nodes, edges, fhash, mtime_ns)
                        total_nodes += len(nodes)
                        total_edges += len(edges)
                        if i % 200 == 0 or i == file_count:
                            logger.info("Progress: %d/%d files parsed", i, file_count)
                _flush_store_batch(store, batch)

        store.set_metadata("last_updated", time.strftime("%Y-%m-%dT%H:%M:%S"))
        store.set_metadata("last_build_type", "full")
        _store_vcs_metadata(repo_root, store)
        store.commit()

    return {
        "files_parsed": len(files),
        "total_nodes": total_nodes,
        "total_edges": total_edges,
        "errors": errors,
    }


def incremental_update(
    repo_root: Path,
    store: GraphStore,
    base: str = "HEAD~1",
    changed_files: list[str] | None = None,
) -> dict:
    """Incremental update: re-parse changed + dependent files only."""
    repo_root = repo_root.resolve()
    store.set_metadata("repo_root", str(repo_root))
    ignore_patterns = _load_ignore_patterns(repo_root)

    # Determine changed files
    if changed_files is None:
        changed_files = get_changed_files(repo_root, base)

    if not changed_files:
        return {
            "files_updated": 0,
            "total_nodes": 0,
            "total_edges": 0,
            "changed_files": [],
            "dependent_files": [],
        }

    total_nodes = 0
    total_edges = 0
    errors = []
    mtime_only_updates: list[tuple[int, str]] = []  # (mtime_ns, file_path) pairs

    # First classify the changed roots themselves. Touch-only changes only need
    # their stored mtime refreshed; they should not force dependent expansion.
    changed_candidates, removed_files = _filter_incremental_candidates(
        repo_root,
        set(changed_files),
        ignore_patterns,
    )
    rust_changed_candidates, python_changed_candidates = _split_rust_parser_files(
        changed_candidates,
        repo_root,
        store,
    )
    content_changed_files: set[str] = set()
    rust_content_changed_files: set[str] = set()

    if rust_changed_candidates:
        classify_changed_rust_owned_files = _callable_store_attr(
            store, "classify_changed_rust_owned_files"
        )
        if classify_changed_rust_owned_files is not None:
            rust_changed, raw_errors = classify_changed_rust_owned_files(
                repo_root,
                rust_changed_candidates,
            )
            rust_content_changed_files.update(rust_changed)
            content_changed_files.update(rust_changed)
            errors.extend(
                {"file": str(file_path), "error": str(error)} for file_path, error in raw_errors
            )
        else:
            content_changed_files.update(rust_changed_candidates)

    if python_changed_candidates:
        changed_file_meta = _get_file_meta_for_candidates(store, python_changed_candidates)
        python_changed, python_mtime_updates = _classify_python_changed_files(
            repo_root,
            python_changed_candidates,
            changed_file_meta,
        )
        content_changed_files.update(python_changed)
        mtime_only_updates.extend(python_mtime_updates)

    dependency_roots = set(removed_files) | content_changed_files
    dependent_files = {
        _make_repo_relative(dep, repo_root)
        for dep in find_dependents_for_files(store, dependency_roots)
    }

    # Combine real content changes, deleted files, and their dependents.
    all_files = content_changed_files | set(removed_files) | dependent_files

    # Separate deleted/unparseable files from files that need re-parsing.
    # When there are no dependent files, the content-changed roots were already
    # filtered as parseable above, so avoid running candidate detection twice.
    if dependent_files:
        candidates, removed_files = _filter_incremental_candidates(
            repo_root,
            all_files,
            ignore_patterns,
        )
    else:
        candidates = list(content_changed_files)

    store.remove_files_data(removed_files)

    file_meta = _get_file_meta_for_candidates(store, candidates)

    rust_candidates, python_candidates = _split_rust_parser_files(candidates, repo_root, store)
    to_parse_rust_forced: list[str] = []
    to_parse_rust_checked: list[str] = []
    to_parse: list[tuple[str, int]] = []
    for rel_path in rust_candidates:
        if rel_path in rust_content_changed_files:
            to_parse_rust_forced.append(rel_path)
            continue
        abs_path = repo_root / rel_path
        try:
            cur_mtime_ns = int(abs_path.stat().st_mtime_ns)
        except (OSError, PermissionError):
            to_parse_rust_checked.append(rel_path)
            continue
        meta = file_meta.get(rel_path)
        if meta and meta[1] == cur_mtime_ns:
            continue
        to_parse_rust_checked.append(rel_path)

    for rel_path in python_candidates:
        abs_path = repo_root / rel_path
        try:
            cur_mtime_ns = int(abs_path.stat().st_mtime_ns)
            meta = file_meta.get(rel_path)
            if meta and meta[1] == cur_mtime_ns:
                continue  # mtime unchanged → definitely same content, skip read
            raw = abs_path.read_bytes()
            fhash = hashlib.sha256(raw).hexdigest()
            if meta and meta[0] == fhash:
                # Content identical despite mtime change (e.g. 'touch') — only
                # update the stored mtime so the fast path fires next time.
                mtime_only_updates.append((cur_mtime_ns, rel_path))
                continue
        except (OSError, PermissionError):
            cur_mtime_ns = 0
        to_parse.append((rel_path, cur_mtime_ns))

    # Persist deletions and mtime-only updates before store_file_nodes_edges()
    # opens its own explicit transaction — avoids nested transaction errors.
    if removed_files or mtime_only_updates:
        if mtime_only_updates:
            if hasattr(store, "update_file_mtimes"):
                store.update_file_mtimes(mtime_only_updates)
            elif hasattr(store, "update_file_mtime"):
                for mtime_ns, file_path in mtime_only_updates:
                    store.update_file_mtime(file_path, mtime_ns)
            elif hasattr(store, "_conn"):
                store._conn.executemany(
                    "UPDATE nodes SET mtime_ns=? WHERE file_path=?", mtime_only_updates
                )
        store.commit()

    if (
        not removed_files
        and not to_parse_rust_forced
        and not to_parse_rust_checked
        and not to_parse
    ):
        return {
            "files_updated": len(all_files),
            "total_nodes": total_nodes,
            "total_edges": total_edges,
            "changed_files": list(changed_files),
            "dependent_files": list(dependent_files),
            "errors": errors,
        }

    use_serial = os.environ.get("CRG_SERIAL_PARSE", "") == "1"
    to_parse_mtime = dict(to_parse)
    store_changed_rust_owned_files = _callable_store_attr(store, "store_changed_rust_owned_files")
    if to_parse_rust_forced:
        if store_changed_rust_owned_files is not None:
            rust_nodes, rust_edges, raw_errors = store_changed_rust_owned_files(
                repo_root,
                to_parse_rust_forced,
            )
            rust_errors = [
                {"file": str(file_path), "error": str(error)} for file_path, error in raw_errors
            ]
        else:
            rust_nodes, rust_edges, rust_errors = _store_rust_parse_batches(
                repo_root,
                store,
                to_parse_rust_forced,
            )
        total_nodes += rust_nodes
        total_edges += rust_edges
        errors.extend(rust_errors)

    if to_parse_rust_checked:
        if store_changed_rust_owned_files is not None:
            rust_nodes, rust_edges, raw_errors = store_changed_rust_owned_files(
                repo_root,
                to_parse_rust_checked,
            )
            rust_errors = [
                {"file": str(file_path), "error": str(error)} for file_path, error in raw_errors
            ]
        else:
            rust_nodes, rust_edges, rust_errors = _store_rust_parse_batches(
                repo_root,
                store,
                to_parse_rust_checked,
            )
        total_nodes += rust_nodes
        total_edges += rust_edges
        errors.extend(rust_errors)

    if use_serial or len(to_parse) < 8:
        batch: StoreBatch = []
        if to_parse:
            parser = CodeParser()
            for rel_path, _ in to_parse:
                mtime_ns = to_parse_mtime.get(rel_path, 0)
                abs_path = repo_root / rel_path
                try:
                    source = abs_path.read_bytes()
                    fhash = hashlib.sha256(source).hexdigest()
                    nodes, edges = parser.parse_bytes(abs_path, source)
                    nodes, edges = _relativize_parsed_entities(nodes, edges, repo_root)
                    _queue_store_file(store, batch, rel_path, nodes, edges, fhash, mtime_ns)
                    total_nodes += len(nodes)
                    total_edges += len(edges)
                except (OSError, PermissionError) as e:
                    errors.append({"file": rel_path, "error": str(e)})
                except Exception as e:
                    logger.warning("Error parsing %s: %s", rel_path, e)
                    errors.append({"file": rel_path, "error": str(e)})
        _flush_store_batch(store, batch)
    else:
        args_list = [(rel_path, str(repo_root)) for rel_path, _ in to_parse]
        batch: StoreBatch = []
        parse_worker = (
            _parse_single_python_file_compact
            if _callable_store_attr(store, "store_file_batch_json") is not None
            else _parse_single_python_file
        )
        with concurrent.futures.ProcessPoolExecutor(
            max_workers=_MAX_PARSE_WORKERS,
            initializer=_init_worker,
        ) as executor:
            for rel_path, nodes, edges, error, fhash, mtime_ns in executor.map(
                parse_worker,
                args_list,
                chunksize=20,
            ):
                if error:
                    logger.warning("Error parsing %s: %s", rel_path, error)
                    errors.append({"file": rel_path, "error": error})
                    continue
                if not _uses_compact_entities(nodes, edges):
                    nodes, edges = _relativize_parsed_entities(nodes, edges, repo_root)
                _queue_store_file(store, batch, rel_path, nodes, edges, fhash, mtime_ns)
                total_nodes += len(nodes)
                total_edges += len(edges)
        _flush_store_batch(store, batch)

    store.set_metadata("last_updated", time.strftime("%Y-%m-%dT%H:%M:%S"))
    store.set_metadata("last_build_type", "incremental")
    _store_vcs_metadata(repo_root, store)
    store.commit()

    return {
        "files_updated": len(all_files),
        "total_nodes": total_nodes,
        "total_edges": total_edges,
        "changed_files": list(changed_files),
        "dependent_files": list(dependent_files),
        "errors": errors,
    }


# ---------------------------------------------------------------------------
# Watch mode
# ---------------------------------------------------------------------------


_DEBOUNCE_SECONDS = 0.3


def watch(
    repo_root: Path,
    store: GraphStore,
    on_files_updated: Optional[Callable] = None,
) -> None:
    """Watch for file changes and auto-update the graph.

    Uses a 300ms debounce to batch rapid-fire saves into a single update.

    Args:
        repo_root: Repository root to watch.
        store: Graph database to update.
        on_files_updated: Optional callback invoked after each debounced
            batch of file updates completes.  Receives the store as its
            only argument.  Used by the CLI to run post-processing
            (FTS, flows, communities) after watch updates.
    """
    import threading

    from watchdog.events import FileSystemEventHandler
    from watchdog.observers import Observer

    parser = CodeParser()
    repo_root = repo_root.resolve()
    store.set_metadata("repo_root", str(repo_root))
    ignore_patterns = _load_ignore_patterns(repo_root)

    class GraphUpdateHandler(FileSystemEventHandler):
        def __init__(self):
            self._pending: set[str] = set()
            self._lock = threading.Lock()
            self._timer: threading.Timer | None = None

        def _should_handle(self, path: str) -> bool:
            if Path(path).is_symlink():
                return False
            try:
                rel = str(Path(path).relative_to(repo_root))
            except ValueError:
                return False
            if _should_ignore(rel, ignore_patterns):
                return False
            if parser.detect_language(Path(path)) is None:
                return False
            return True

        def on_modified(self, event):
            if event.is_directory:
                return
            if self._should_handle(event.src_path):
                self._schedule(event.src_path)

        def on_created(self, event):
            if event.is_directory:
                return
            if self._should_handle(event.src_path):
                self._schedule(event.src_path)

        def on_deleted(self, event):
            if event.is_directory:
                return
            # Only handle files we would normally track
            try:
                rel = str(Path(event.src_path).relative_to(repo_root))
            except ValueError:
                return
            if _should_ignore(rel, ignore_patterns):
                return
            try:
                store.remove_file_data(rel)
                store.commit()
                logger.info("Removed: %s", rel)
            except Exception as e:
                logger.error("Error removing %s: %s", rel, e)

        def _schedule(self, abs_path: str):
            """Add file to pending set and reset the debounce timer."""
            with self._lock:
                self._pending.add(abs_path)
                if self._timer is not None:
                    self._timer.cancel()
                self._timer = threading.Timer(_DEBOUNCE_SECONDS, self._flush)
                self._timer.start()

        def _flush(self):
            """Process all pending files after the debounce window."""
            with self._lock:
                paths = list(self._pending)
                self._pending.clear()
                self._timer = None

            updated = 0
            for abs_path in paths:
                if self._update_file(abs_path):
                    updated += 1

            if updated > 0 and on_files_updated is not None:
                try:
                    on_files_updated(store)
                except Exception as e:
                    logger.error("Post-update callback failed: %s", e)

        def _update_file(self, abs_path: str) -> bool:
            path = Path(abs_path)
            if not path.is_file():
                return False
            if path.is_symlink():
                return False
            if _is_binary(path):
                return False
            rel = str(path.relative_to(repo_root))
            try:
                source = path.read_bytes()
                fhash = hashlib.sha256(source).hexdigest()
                nodes, edges = parser.parse_bytes(path, source)
                nodes, edges = _relativize_parsed_entities(nodes, edges, repo_root)
                store.store_file_nodes_edges(rel, nodes, edges, fhash)
                store.set_metadata("last_updated", time.strftime("%Y-%m-%dT%H:%M:%S"))
                store.commit()
                logger.info(
                    "Updated: %s (%d nodes, %d edges)",
                    rel,
                    len(nodes),
                    len(edges),
                )
                return True
            except Exception as e:
                logger.error("Error updating %s: %s", abs_path, e)
                return False

    handler = GraphUpdateHandler()
    observer = Observer()
    observer.schedule(handler, str(repo_root), recursive=True)
    observer.start()

    logger.info("Watching %s for changes... (Ctrl+C to stop)", repo_root)
    try:
        import time as _time

        while True:
            _time.sleep(1)
    except KeyboardInterrupt:
        observer.stop()
    observer.join()
    logger.info("Watch stopped.")
