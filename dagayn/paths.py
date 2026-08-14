"""Where a repository's graph data lives.

Kept apart from :mod:`dagayn.incremental_files` so that import-light callers —
agent hooks, :mod:`dagayn.worktree` — can resolve a graph path without pulling
in the parser. :mod:`dagayn.incremental_files` re-exports these names, so
``from dagayn.incremental import get_db_path`` keeps working.
"""

from __future__ import annotations

import hashlib
import logging
import os
import re
import sqlite3
import sys
from pathlib import Path

logger = logging.getLogger(__name__)

_SLUG_NAME_RE = re.compile(r"[^A-Za-z0-9._-]+")


def same_repo_path(left: Path | str, right: Path | str) -> bool:
    """Return True when two paths name the same directory.

    ``os.path.normcase`` only folds case on Windows and is a no-op on macOS,
    which is exactly where a case-insensitive filesystem makes ``/x/MAIN`` and
    ``/x/main`` the same directory. ``samefile`` compares ``st_dev``/``st_ino``.
    """
    left_s = os.fspath(Path(left).expanduser())
    right_s = os.fspath(Path(right).expanduser())
    if os.path.normcase(left_s) == os.path.normcase(right_s):
        return True
    try:
        return os.path.samefile(left_s, right_s)
    except OSError:
        try:
            return Path(left_s).resolve() == Path(right_s).resolve()
        except (OSError, RuntimeError):
            return False


def is_project_root(path: Path) -> bool:
    """Return True when *path* is a VCS checkout or already holds a graph.

    An empty ``.dagayn`` directory is not enough: :func:`get_data_dir` used to
    create one as a side effect of path lookup, which then made any previously
    resolved directory look like a project root forever. See: #90, #127
    """
    resolved = Path(path)
    if (resolved / ".git").exists() or (resolved / ".svn").exists():
        return True
    return (resolved / ".dagayn" / "graph.db").is_file()


def _filesystem_case_insensitive(path: Path) -> bool:
    """Best-effort check that *path*'s filesystem folds case."""
    probe = path if path.exists() else path.parent
    swapped_name = probe.name.swapcase()
    if swapped_name != probe.name:
        swapped = probe.with_name(swapped_name)
        try:
            if swapped.exists():
                return os.path.samefile(probe, swapped)
        except OSError:
            pass
    return sys.platform in ("win32", "darwin")


def _slug_name(resolved: Path) -> str:
    name = _SLUG_NAME_RE.sub("-", resolved.name).strip("-") or "repo"
    if _filesystem_case_insensitive(resolved):
        return name.lower()
    return name


def _repo_identity_key(resolved: Path) -> str:
    """Stable identity for hashing: inode when the path exists, else path text."""
    try:
        st = resolved.stat()
        return f"ino:{st.st_dev}:{st.st_ino}"
    except OSError:
        return f"path:{os.path.normcase(str(resolved))}"


def _legacy_repo_slug(repo_root: Path) -> str:
    """Pre-#87 slug: SHA-256 of ``resolve()``d path text, no case/inode identity."""
    resolved = Path(repo_root).expanduser().resolve()
    digest = hashlib.sha256(str(resolved).encode("utf-8")).hexdigest()[:12]
    name = _SLUG_NAME_RE.sub("-", resolved.name).strip("-") or "repo"
    return f"{name}-{digest}"


def repo_slug(repo_root: Path) -> str:
    """Return a stable, filesystem-safe directory name for *repo_root*.

    The readable part is the directory name; the suffix is a digest of the
    directory's inode identity when it exists, otherwise of the case-folded
    path. Two spellings of the same checkout on a case-insensitive filesystem
    therefore share one graph. See: #87
    """
    resolved = Path(repo_root).expanduser().resolve()
    digest = hashlib.sha256(_repo_identity_key(resolved).encode("utf-8")).hexdigest()[:12]
    return f"{_slug_name(resolved)}-{digest}"


def _shared_dir_from_env() -> Path | None:
    env_override = os.environ.get("CRG_DATA_DIR", "").strip()
    if not env_override:
        return None
    return Path(env_override).expanduser().resolve()


def _existing_shared_data_dir(
    shared_dir: Path,
    repo_root: Path,
    canonical: Path,
) -> Path | None:
    """Find an already-created CRG_DATA_DIR subdirectory for *repo_root*."""
    if canonical.exists():
        return canonical
    legacy = shared_dir / _legacy_repo_slug(repo_root)
    if legacy.exists() and legacy != canonical:
        return legacy
    if not shared_dir.is_dir():
        return None
    for child in shared_dir.iterdir():
        if not child.is_dir() or child == canonical:
            continue
        db = child / "graph.db"
        if db.is_file() and _shared_graph_belongs_to(db, repo_root):
            return child
    return None


def data_dir_for(repo_root: Path) -> Path:
    """Return where this project's graph *would* live. Creates nothing.

    Lookup and validation must not resurrect deleted checkouts. See: #90
    """
    shared_dir = _shared_dir_from_env()
    if shared_dir is None:
        return Path(repo_root) / ".dagayn"
    canonical = shared_dir / repo_slug(repo_root)
    return _existing_shared_data_dir(shared_dir, repo_root, canonical) or canonical


def db_path_for(repo_root: Path) -> Path:
    """Return the ``graph.db`` path. Creates nothing and migrates nothing."""
    return data_dir_for(repo_root) / "graph.db"


def _ensure_inner_gitignore(data_dir: Path) -> None:
    inner_gitignore = data_dir / ".gitignore"
    if inner_gitignore.exists():
        return
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


def get_data_dir(repo_root: Path) -> Path:
    """Return the directory where this project's graph data lives.

    By default, ``<repo_root>/.dagayn``. If the ``CRG_DATA_DIR`` environment
    variable is set, graph data goes to a per-repository subdirectory of it,
    ``<CRG_DATA_DIR>/<name>-<identity digest>`` — letting you keep graphs
    outside the working tree (useful for ephemeral workspaces, Docker volumes,
    or shared caches). See: #155, #87

    The subdirectory matters: ``CRG_DATA_DIR`` used to be honored verbatim, so
    every repository and worktree that saw the variable shared one
    ``graph.db``. Exporting it from a shell profile silently mixed several
    projects' nodes into a single graph, with ``repo_root`` metadata belonging
    to whichever one last wrote.

    A pre-existing ``<CRG_DATA_DIR>/graph.db`` is migrated into the
    subdirectory when its ``repo_root`` metadata names *this* repository, so a
    single-repository setup keeps its graph.

    This function creates the directory if it does not already exist and
    writes an inner ``.gitignore`` (with ``*``) so any accidentally-nested
    files never get committed. Both are idempotent. For existence checks use
    :func:`data_dir_for` / :func:`db_path_for` instead. See: #90
    """
    shared_dir = _shared_dir_from_env()
    if shared_dir is not None:
        canonical = shared_dir / repo_slug(repo_root)
        found = _existing_shared_data_dir(shared_dir, repo_root, canonical)
        if found is not None and found != canonical:
            try:
                canonical.parent.mkdir(parents=True, exist_ok=True)
                found.rename(canonical)
                data_dir = canonical
            except OSError as exc:
                logger.warning("Could not move %s to %s: %s", found, canonical, exc)
                data_dir = found
        else:
            data_dir = canonical
        data_dir.mkdir(parents=True, exist_ok=True)
        _migrate_shared_data_dir(shared_dir, data_dir, repo_root)
    else:
        data_dir = Path(repo_root) / ".dagayn"
        data_dir.mkdir(parents=True, exist_ok=True)

    _ensure_inner_gitignore(data_dir)
    return data_dir


def _shared_graph_belongs_to(db_path: Path, repo_root: Path) -> bool:
    """Return True when *db_path* records *repo_root* as its repository."""
    try:
        conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    except sqlite3.Error:
        return False
    try:
        row = conn.execute("SELECT value FROM metadata WHERE key = 'repo_root'").fetchone()
    except sqlite3.Error:
        return False
    finally:
        conn.close()
    if not row or not row[0]:
        return False
    try:
        return same_repo_path(Path(str(row[0])), repo_root)
    except (OSError, RuntimeError):
        return False


def _migrate_shared_data_dir(shared_dir: Path, data_dir: Path, repo_root: Path) -> None:
    """Move a pre-subdirectory ``CRG_DATA_DIR`` graph into *data_dir*.

    Only when the graph says it describes *repo_root*: a graph belonging to
    another repository must stay where it is rather than be claimed by whoever
    runs next. Side-files are dropped — the graph is checkpointed on close, and
    a stale WAL cannot be matched to a moved database with confidence.
    """
    legacy_db = shared_dir / "graph.db"
    target_db = data_dir / "graph.db"
    if target_db.exists() or not legacy_db.exists():
        return
    if not _shared_graph_belongs_to(legacy_db, repo_root):
        return
    try:
        legacy_db.rename(target_db)
    except OSError as exc:
        logger.warning("Could not move %s into %s: %s", legacy_db, data_dir, exc)
        return
    for suffix in ("-wal", "-shm", "-journal"):
        side = shared_dir / f"graph.db{suffix}"
        if side.exists():
            try:
                side.unlink()
            except OSError:  # nosec B110 — best-effort cleanup
                pass
    logger.info("Moved shared CRG_DATA_DIR graph into %s", data_dir)


def get_db_path(repo_root: Path) -> Path:
    """Determine the database path for a repository.

    Respects ``CRG_DATA_DIR`` (see :func:`get_data_dir`). Migrates a
    legacy top-level ``.dagayn.db`` file into the new
    directory when it exists (WAL/SHM side-files are discarded).

    This is a mutating helper for build/write paths. For lookup use
    :func:`db_path_for`.
    """
    crg_dir = get_data_dir(repo_root)
    new_db = crg_dir / "graph.db"

    # Migrate legacy database if present (only meaningful when the
    # legacy file sits at the repo root — if CRG_DATA_DIR is set we
    # skip the migration because there's no relationship between the
    # legacy location and the new one).
    legacy_db = Path(repo_root) / ".dagayn.db"
    if legacy_db.exists() and not new_db.exists():
        legacy_db.rename(new_db)
    # Discard stale WAL/SHM side-files from the old location
    for suffix in ("-wal", "-shm", "-journal"):
        side = Path(repo_root) / f".dagayn.db{suffix}"
        if side.exists():
            side.unlink()

    return new_db
