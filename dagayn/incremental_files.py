"""Repository discovery, ignore rules, and file-change detection."""

from __future__ import annotations

import fnmatch
import json
import logging
import os
import re
import subprocess
from pathlib import Path, PurePosixPath
from typing import TYPE_CHECKING, Any, Optional

if TYPE_CHECKING:
    from .graph import GraphStore

from .parser import CodeParser
from .parser._base.types import EdgeInfo, NodeInfo

# Re-exported: these used to live here, and ``dagayn.incremental`` publishes
# them from this module.
from .paths import data_dir_for as data_dir_for
from .paths import db_path_for as db_path_for
from .paths import get_data_dir as get_data_dir
from .paths import get_db_path as get_db_path
from .paths import is_project_root as is_project_root
from .paths import repo_slug as repo_slug
from .paths import same_repo_path as same_repo_path

logger = logging.getLogger(__name__)

_DEFAULT_BACKEND = "rust"

# Default ignore patterns applied after git's indexable set (tracked +
# untracked, excluding gitignored). ``.dagaynignore`` is an extra restriction.
#
# `<dir>/**` patterns are matched at any depth by _should_ignore, so
# `node_modules/**` also excludes `packages/app/node_modules/react/index.js`
# inside monorepos. See: #91
DEFAULT_IGNORE_PATTERNS = [
    ".dagayn/**",
    "node_modules/**",
    # Git worktrees checked out inside the repository are additional copies of
    # the same history, so indexing them multiplies the whole graph by the
    # number of worktrees. `git ls-files --others` stops at the nested-repo
    # boundary and reports the directory, which the is_file() checks already
    # drop -- but the directory-walk fallback used when git returns nothing has
    # no such boundary, and one such graph reached 1.6M nodes across 39
    # worktrees against 30k for the repository itself.
    ".worktrees/**",
    ".claude/worktrees/**",
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


def _workspace_folder_candidates() -> list[Path]:
    """Return IDE/workspace roots hinted by process environment.

    Cursor may launch MCP servers with ``cwd=$HOME`` even when a project is
    open. Prefer these hints over treating the home directory as a repo.
    Cursor injects ``WORKSPACE_FOLDER_PATHS`` (comma-separated) even when it
    does not expand ``${workspaceFolder}`` in user-level ``mcp.json``.
    """
    candidates: list[Path] = []
    for var in ("CURSOR_PROJECT_DIR", "CLAUDE_PROJECT_DIR"):
        raw = os.environ.get(var, "").strip()
        if raw:
            candidates.append(Path(raw))

    raw = os.environ.get("WORKSPACE_FOLDER_PATHS", "").strip()
    if raw:
        if raw.startswith("["):
            try:
                parsed = json.loads(raw)
            except json.JSONDecodeError:
                parsed = None
            if isinstance(parsed, list):
                candidates.extend(Path(str(item)) for item in parsed)
        elif "," in raw:
            # Cursor uses comma-separated absolute paths (multi-root).
            candidates.extend(Path(part.strip()) for part in raw.split(",") if part.strip())
        else:
            for part in raw.split(":"):
                part = part.strip()
                if part:
                    candidates.append(Path(part))

    resolved: list[Path] = []
    seen: set[Path] = set()
    for candidate in candidates:
        try:
            path = candidate.expanduser().resolve()
        except OSError:
            continue
        if not path.exists() or path in seen:
            continue
        seen.add(path)
        resolved.append(path)
    return resolved


def _pick_workspace_root(
    candidates: list[Path],
    *,
    stop_at: Path | None = None,
) -> Path | None:
    """Choose the best IDE workspace root among multi-root candidates."""
    scored: list[tuple[tuple[object, ...], Path]] = []
    for workspace in candidates:
        hinted = find_repo_root(workspace, stop_at=stop_at)
        root = hinted or (workspace if (workspace / ".dagayn").is_dir() else None)
        if root is None:
            continue
        graph = root / ".dagayn" / "graph.db"
        has_graph = graph.is_file()
        try:
            mtime = graph.stat().st_mtime if has_graph else 0.0
        except OSError:
            mtime = 0.0
        scored.append(((has_graph, mtime, (root / ".git").exists()), root))
    if not scored:
        return None
    scored.sort(key=lambda item: item[0], reverse=True)
    return scored[0][1]


_UNRESOLVED_PATH_PLACEHOLDER = re.compile(r"^\$\{[^}]+\}$")


def is_unresolved_path_placeholder(value: str | None) -> bool:
    """Return True for IDE template strings like ``${workspaceFolder}``."""
    if value is None:
        return False
    return bool(_UNRESOLVED_PATH_PLACEHOLDER.match(value.strip()))


def resolve_cli_repo_root(
    repo: str | None = None,
    *,
    start: Path | None = None,
    stop_at: Path | None = None,
) -> Path:
    """Resolve a CLI/MCP ``--repo`` value, ignoring unexpanded IDE placeholders.

    Cursor's user-level MCP config often passes ``--repo ${workspaceFolder}``
    literally. Treat missing, placeholder, and non-existent paths as auto-detect
    via :func:`find_project_root`.
    """
    if repo and not is_unresolved_path_placeholder(repo):
        try:
            path = Path(repo).expanduser().resolve()
        except OSError:
            path = None
        if path is not None and path.exists():
            return find_repo_root(path, stop_at=stop_at) or path
    return find_project_root(start, stop_at=stop_at)


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
    3. Workspace/IDE hints (``CURSOR_PROJECT_DIR``, ``CLAUDE_PROJECT_DIR``,
       ``WORKSPACE_FOLDER_PATHS``) when the start path is not already inside
       a git root — e.g. Cursor launching MCP with ``cwd=$HOME``. Multi-root
       workspaces prefer the folder with the richest existing ``.dagayn``
       graph.
    4. ``start`` itself (or cwd if no start given).

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

    picked = _pick_workspace_root(_workspace_folder_candidates(), stop_at=stop_at)
    if picked is not None:
        return picked

    return start or Path.cwd()


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
    """Quick heuristic: check if file appears to be binary.

    Reads a bounded prefix. ``read_bytes()[:8192]`` loaded the whole file into
    memory first, and neither this path nor ``watch``'s flush caps file size.
    """
    try:
        with path.open("rb") as handle:
            chunk = handle.read(8192)
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


def resolve_commit_sha(repo_root: Path, ref: str) -> str | None:
    """Return the full sha *ref* names in *repo_root*, or None if it has none.

    Non-git working copies return None: only git has the metadata contract
    (``git_head_sha``) the callers of this guard.
    """
    if detect_vcs(repo_root) != "git":
        return None
    if not _SAFE_GIT_REF.match(ref):
        return None
    try:
        result = subprocess.run(
            ["git", "rev-parse", "--verify", "--quiet", f"{ref}^{{commit}}"],
            capture_output=True,
            text=True,
            cwd=str(repo_root),
            timeout=_GIT_TIMEOUT,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return None
    if result.returncode != 0:
        return None
    return result.stdout.strip() or None


def get_changed_files(repo_root: Path, base: str = "HEAD~1") -> list[str]:
    """Get list of changed files via git diff plus working-tree status.

    For SVN working copies the *base* parameter is ignored; modified/added/
    deleted files are detected from ``svn status``.  Pass an SVN revision
    range (e.g. ``"r100:HEAD"``) as *base* to compare against a specific
    revision instead.
    """
    if detect_vcs(repo_root) == "svn":
        return _get_svn_changed_files(repo_root, base if _SAFE_SVN_REV.match(base) else None)
    return get_changed_file_sources(repo_root, base).get("files", [])


def get_changed_file_sources(repo_root: Path, base: str = "HEAD~1") -> dict[str, list[str]]:
    """Get changed files grouped by origin.

    ``base_diff`` contains files changed between *base* and the current
    revision. ``worktree`` contains local staged, unstaged, and untracked
    changes. The combined ``files`` list preserves first-seen order.
    """
    if detect_vcs(repo_root) == "svn":
        files = _get_svn_changed_files(repo_root, base if _SAFE_SVN_REV.match(base) else None)
        return {
            "files": files,
            "base_diff": [],
            "worktree": files,
            "staged": [],
            "unstaged": files,
            "untracked": [],
        }

    if not _SAFE_GIT_REF.match(base):
        logger.warning("Invalid git ref rejected: %s", base)
        return {
            "files": [],
            "base_diff": [],
            "worktree": [],
            "staged": [],
            "unstaged": [],
            "untracked": [],
        }

    base_diff = _get_git_diff_files(repo_root, base)
    worktree_sources = _get_git_worktree_change_sources(repo_root)
    worktree = worktree_sources["worktree"]
    return {
        "files": _dedupe_preserve_order(base_diff + worktree),
        "base_diff": base_diff,
        **worktree_sources,
    }


def _nul_fields(payload: str) -> list[str]:
    """Split a ``-z`` git payload into its NUL-separated fields."""
    return [field for field in payload.split("\0") if field]


def _get_git_diff_files(repo_root: Path, base: str) -> list[str]:
    """Return the files changed between *base* and HEAD, both sides of renames.

    ``--name-status -M -z`` rather than ``--name-only``: git's rename detection
    is on by default, so ``--name-only`` reports a rename's destination *only*,
    and the source path's nodes were left in the graph forever -- the same code
    served under two paths. ``-z`` additionally keeps non-ASCII paths usable,
    which ``core.quotePath`` (on by default) otherwise C-quotes into a literal
    no ``open()`` can resolve.
    """
    try:
        result = subprocess.run(
            ["git", "diff", "--name-status", "-M", "-z", base, "HEAD", "--"],
            capture_output=True,
            text=True,
            cwd=str(repo_root),
            timeout=_GIT_TIMEOUT,
        )
        if result.returncode != 0:
            logger.warning("git diff failed (rc=%d): %s", result.returncode, result.stderr[:200])
            return []
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return []

    fields = _nul_fields(result.stdout)
    files: list[str] = []
    index = 0
    while index < len(fields):
        status = fields[index]
        # Rename/copy records carry two paths: ``R100 old new``. Both matter --
        # the old path needs its nodes pruned, the new one needs parsing.
        paths_wanted = 2 if status[:1] in {"R", "C"} else 1
        for offset in range(1, paths_wanted + 1):
            if index + offset < len(fields):
                files.append(fields[index + offset])
        index += paths_wanted + 1
    return _dedupe_preserve_order(files)


def _get_git_worktree_change_sources(repo_root: Path) -> dict[str, list[str]]:
    """Return staged / unstaged / untracked working-tree paths.

    Parsed from ``--porcelain -z``: the text form C-quotes non-ASCII paths and
    encodes a rename as ``R  old -> new``, which the previous ` -> ` split
    discarded the old half of (leaving its nodes in the graph) and which mangles
    any filename containing that literal sequence. In ``-z`` mode a rename is
    two NUL-separated fields, new path first.
    """
    try:
        result = subprocess.run(
            ["git", "status", "--porcelain", "-z", "--untracked-files=all"],
            capture_output=True,
            text=True,
            cwd=str(repo_root),
            timeout=_GIT_TIMEOUT,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return {"worktree": [], "staged": [], "unstaged": [], "untracked": []}

    staged: list[str] = []
    unstaged: list[str] = []
    untracked: list[str] = []
    entries = [entry for entry in result.stdout.split("\0") if entry]
    index = 0
    while index < len(entries):
        entry = entries[index]
        index += 1
        if len(entry) <= 3:
            continue
        x_status = entry[0]
        y_status = entry[1]
        paths = [entry[3:]]
        if x_status in {"R", "C"} or y_status in {"R", "C"}:
            if index < len(entries):
                paths.append(entries[index])
                index += 1
        if x_status == "?" and y_status == "?":
            untracked.extend(paths)
            continue
        if x_status != " ":
            staged.extend(paths)
        if y_status != " ":
            unstaged.extend(paths)

    return {
        "worktree": _dedupe_preserve_order(staged + unstaged + untracked),
        "staged": _dedupe_preserve_order(staged),
        "unstaged": _dedupe_preserve_order(unstaged),
        "untracked": _dedupe_preserve_order(untracked),
    }


def _dedupe_preserve_order(paths: list[str]) -> list[str]:
    """Return unique paths while preserving first-seen order."""
    seen: set[str] = set()
    result: list[str] = []
    for path in paths:
        if path in seen:
            continue
        seen.add(path)
        result.append(path)
    return result


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
    return _get_git_worktree_change_sources(repo_root)["worktree"]


def _git_ls_files(repo_root: Path, extra_args: list[str]) -> list[str]:
    """Run ``git ls-files -z`` with *extra_args* and split the NUL payload."""
    cmd = ["git", "ls-files", "-z", *extra_args]
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            cwd=str(repo_root),
            timeout=_GIT_TIMEOUT,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return []
    return _nul_fields(result.stdout)


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
        from . import incremental as inc

        recurse_submodules = inc._RECURSE_SUBMODULES

    extra: list[str] = []
    if recurse_submodules:
        extra.append("--recurse-submodules")
    return _git_ls_files(repo_root, extra)


def get_vcs_indexable_files(
    repo_root: Path,
    recurse_submodules: bool | None = None,
) -> list[str]:
    """Return git-indexable paths: tracked plus untracked, excluding gitignored.

    This is the working-tree source set for ``build``, ``update``, and
    ``watch``. ``.dagaynignore`` is applied later by :func:`collect_all_files`.
    SVN working copies stay tracked-only.

    ``git ls-files --recurse-submodules`` only supports ``--cached``, so
    untracked files are collected from the superproject only.
    """
    if detect_vcs(repo_root) == "svn":
        return _get_svn_all_tracked_files(repo_root)

    if recurse_submodules is None:
        from . import incremental as inc

        recurse_submodules = inc._RECURSE_SUBMODULES

    cached_args = ["--cached"]
    if recurse_submodules:
        cached_args.append("--recurse-submodules")
    cached = _git_ls_files(repo_root, cached_args)
    others = _git_ls_files(repo_root, ["--others", "--exclude-standard"])
    return _dedupe_preserve_order(cached + others)


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


def _backend_selection() -> str:
    return os.environ.get("DAGAYN_BACKEND", _DEFAULT_BACKEND).strip().lower()


def _rust_backend_enabled() -> bool:
    return _backend_selection() == "rust"


def collect_all_files(
    repo_root: Path,
    recurse_submodules: bool | None = None,
) -> list[str]:
    """Collect parseable files in the git-indexable set, then apply ``.dagaynignore``.

    Git scope is tracked plus untracked, excluding gitignored. ``.dagaynignore``
    and :data:`DEFAULT_IGNORE_PATTERNS` are extra restrictions on that set.
    SVN working copies stay tracked-only.

    Args:
        repo_root: Repository root directory.
        recurse_submodules: If True, include files from git submodules.
            When *None*, falls back to ``CRG_RECURSE_SUBMODULES`` env var.
    """
    if recurse_submodules is None:
        # Resolve the env default *before* crossing into Rust: the Rust side
        # does `recurse_submodules.unwrap_or(false)` and never sees the env var,
        # so CRG_RECURSE_SUBMODULES was silently ignored under the default
        # backend (the fallback below is the only place that honoured it).
        from . import incremental as inc

        recurse_submodules = inc._RECURSE_SUBMODULES

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
    # Prefer git's indexable set (tracked + untracked, excluding gitignored).
    indexable = get_vcs_indexable_files(repo_root, recurse_submodules)
    if indexable:
        candidates = indexable
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
