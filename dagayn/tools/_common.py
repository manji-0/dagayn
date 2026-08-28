"""Shared utilities for tool sub-modules."""

from __future__ import annotations

import contextvars
import json
import logging
import os
import sqlite3
import sys
import threading
from collections.abc import Mapping, MutableMapping, Sequence
from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as pkg_version
from pathlib import Path
from typing import Any, TypedDict, cast

from ..graph import GraphStore
from ..graph.sqlite_errors import (
    close_live_stores_for,
    is_sqlite_corrupt_error,
    probe_graph_database,
    register_live_store,
)
from ..incremental import (
    _backend_selection,
    find_project_root,
    get_db_path,
)
from ..paths import ALLOW_WIDE_ROOT_ENV, recorded_repo_root, same_repo_path, unsafe_root_reason
from ..state_types import (
    AnswerabilityRecord,
    MissingnessRecord,
    seal_answerability_summary,
    seal_guidance_item,
    seal_missingness_item,
)
from ..write_lock import (
    DEFAULT_READ_LOCK_TIMEOUT,
    WriteLockUnavailableError,
    acquire_graph_lock,
    bind_store_read_lock,
    lock_holder_pid,
    release_graph_lock,
    wrap_store_close_to_unbind,
    write_lock_is_held,
)

logger = logging.getLogger(__name__)


type DynamicValue = Any
type ToolPayload = dict[str, DynamicValue]


class RuntimeSummaryRecord(TypedDict):
    package: str
    version: str
    pid: int
    python: str
    package_root: str


class ToolHintStep(TypedDict):
    tool: str
    suggestion: str


class ToolHintsRecord(TypedDict):
    next_steps: list[ToolHintStep]
    related: list[object]
    warnings: list[str]


_TOOL_RUNTIME_ERRORS: tuple[type[BaseException], ...] = (
    OSError,
    sqlite3.Error,
    ValueError,
    KeyError,
    TypeError,
    AttributeError,
)


class RepoContextRecord(TypedDict):
    repo_root: str
    db_path: str
    source: str


#: The repository a tool call actually read, recorded by :func:`_get_store` so
#: the MCP layer can report it back. A ContextVar rather than a global: each
#: ``asyncio.to_thread`` tool runs in its own copied context, so concurrent
#: calls cannot overwrite each other's answer.
_repo_context: contextvars.ContextVar[RepoContextRecord | None] = contextvars.ContextVar(
    "dagayn_repo_context",
    default=None,
)


class RepoRootMismatchError(ValueError):
    """The graph opened for a repository says it describes a different one."""


def set_repo_context(repo_root: Path, db_path: Path, *, explicit: bool) -> RepoContextRecord:
    """Record which repository (and graph file) this tool call resolved to."""
    record: RepoContextRecord = {
        "repo_root": str(repo_root),
        "db_path": str(db_path),
        "source": "explicit" if explicit else "auto",
    }
    _repo_context.set(record)
    return record


def reset_repo_context() -> None:
    """Forget the recorded repository so a later call cannot inherit it."""
    _repo_context.set(None)


def repo_context_snapshot() -> RepoContextRecord | None:
    """Return the repository recorded during this call, if any."""
    return _repo_context.get()


def attach_repo_context(payload: ToolPayload) -> ToolPayload:
    """Add ``_repo`` (root, graph path, how it was resolved) to a response.

    Answering from the wrong repository used to be invisible: no response field
    named the root, and nothing logged it, so a mis-resolved ``cwd`` looked like
    an ordinary answer about someone else's code.
    """
    record = repo_context_snapshot()
    if record is not None:
        payload.setdefault("_repo", dict(record))
    return payload


def _assert_graph_matches_repo(db_path: Path, root: Path) -> None:
    """Refuse to answer from a graph that records a different repository.

    A graph whose recorded root no longer exists is treated as this
    repository's: a moved or renamed checkout is the common cause and its graph
    is still the right one. Only a recorded root that still exists *and* is a
    different directory means the resolution went to the wrong repository.
    """
    recorded = recorded_repo_root(db_path)
    if recorded is None:
        return
    try:
        if same_repo_path(recorded, root):
            return
        if not recorded.exists():
            logger.debug(
                "graph %s records a repo_root that no longer exists (%s); treating it as %s",
                db_path,
                recorded,
                root,
            )
            return
    except (OSError, RuntimeError):
        return
    raise RepoRootMismatchError(
        f"the graph at {db_path} describes {recorded}, not {root}, so its results would"
        f" belong to a different repository. Pass repo_root explicitly (MCP tools), set"
        f" CRG_REPO_ROOT, or give the MCP server entry a cwd/--repo for this project;"
        f" editors that launch the server without one resolve the repository from an"
        f" ambient working directory."
    )


def _error_response(
    message: str,
    status: str = "error",
    **extra: Any,
) -> ToolPayload:
    """Build a standardised error response dict."""
    return {"status": status, "error": message, "summary": message, **extra}


def tool_runtime_summary() -> RuntimeSummaryRecord:
    """Return compact runtime identity for comparing CLI and MCP responses."""
    try:
        version = pkg_version("dagayn")
    except PackageNotFoundError:
        version = "dev"
    module_file = Path(__file__).resolve()
    package_root = module_file.parents[1]
    return {
        "package": "dagayn",
        "version": version,
        "pid": os.getpid(),
        "python": sys.executable,
        "package_root": str(package_root),
    }


def attach_runtime_metadata(payload: ToolPayload) -> ToolPayload:
    """Attach runtime identity without overriding explicit tool metadata."""
    payload.setdefault("_runtime", tool_runtime_summary())
    return payload


# Common JS/TS builtin method names filtered from callers_of results.
# "Who calls .map()?" returns hundreds of hits and is never useful.
# These are kept in the graph (callees_of still shows them) but excluded
# when doing reverse call tracing to reduce noise.
_BUILTIN_CALL_NAMES: set[str] = {
    "map",
    "filter",
    "reduce",
    "reduceRight",
    "forEach",
    "find",
    "findIndex",
    "some",
    "every",
    "includes",
    "indexOf",
    "lastIndexOf",
    "push",
    "pop",
    "shift",
    "unshift",
    "splice",
    "slice",
    "concat",
    "join",
    "flat",
    "flatMap",
    "sort",
    "reverse",
    "fill",
    "keys",
    "values",
    "entries",
    "from",
    "isArray",
    "of",
    "at",
    "trim",
    "trimStart",
    "trimEnd",
    "split",
    "replace",
    "replaceAll",
    "match",
    "matchAll",
    "search",
    "substring",
    "substr",
    "toLowerCase",
    "toUpperCase",
    "startsWith",
    "endsWith",
    "padStart",
    "padEnd",
    "repeat",
    "charAt",
    "charCodeAt",
    "assign",
    "freeze",
    "defineProperty",
    "getOwnPropertyNames",
    "hasOwnProperty",
    "create",
    "is",
    "fromEntries",
    "log",
    "warn",
    "error",
    "info",
    "debug",
    "trace",
    "dir",
    "table",
    "time",
    "timeEnd",
    "assert",
    "clear",
    "count",
    "then",
    "catch",
    "finally",
    "resolve",
    "reject",
    "all",
    "allSettled",
    "race",
    "any",
    "parse",
    "stringify",
    "floor",
    "ceil",
    "round",
    "random",
    "max",
    "min",
    "abs",
    "pow",
    "sqrt",
    "addEventListener",
    "removeEventListener",
    "querySelector",
    "querySelectorAll",
    "getElementById",
    "createElement",
    "appendChild",
    "removeChild",
    "setAttribute",
    "getAttribute",
    "preventDefault",
    "stopPropagation",
    "setTimeout",
    "clearTimeout",
    "setInterval",
    "clearInterval",
    "toString",
    "valueOf",
    "toJSON",
    "toISOString",
    "getTime",
    "getFullYear",
    "now",
    "isNaN",
    "parseInt",
    "parseFloat",
    "toFixed",
    "encodeURIComponent",
    "decodeURIComponent",
    "call",
    "apply",
    "bind",
    "next",
    "emit",
    "on",
    "off",
    "once",
    "pipe",
    "write",
    "read",
    "end",
    "close",
    "destroy",
    "send",
    "status",
    "json",
    "redirect",
    "set",
    "get",
    "delete",
    "has",
    "findUnique",
    "findFirst",
    "findMany",
    "createMany",
    "update",
    "updateMany",
    "deleteMany",
    "upsert",
    "aggregate",
    "groupBy",
    "transaction",
    "describe",
    "it",
    "test",
    "expect",
    "beforeEach",
    "afterEach",
    "beforeAll",
    "afterAll",
    "mock",
    "spyOn",
    "require",
    "fetch",
}


def _validate_repo_root(path: Path) -> Path:
    """Validate that a path is a plausible project root.

    Ensures the path is an existing directory that contains a ``.git`` /
    ``.svn`` checkout or a ``.dagayn/graph.db`` graph, preventing arbitrary
    file-system traversal via the ``repo_root`` parameter. An empty
    ``.dagayn`` directory is not enough. See: #127
    """
    from ..paths import is_project_root

    resolved = path.resolve()
    if not resolved.is_dir():
        raise ValueError(f"repo_root is not an existing directory: {resolved}")
    if not is_project_root(resolved):
        raise ValueError(
            f"repo_root does not look like a project root (no .git or "
            f".dagayn/graph.db found): {resolved}"
        )
    return resolved


def resolve_contained_path(rel_path: str, repo_root: Path) -> Path | None:
    """Resolve *rel_path* under *repo_root*, or ``None`` when it escapes.

    Caller-supplied file lists (``changed_files`` on the review tools) reach
    the filesystem, so they need the same containment guarantee the edit path
    in :func:`dagayn.refactor.apply.apply_refactor` has. ``root / rel_path``
    alone provides none: ``Path.__truediv__`` discards ``root`` when the right
    operand is absolute, and ``..`` segments are not normalised until
    ``resolve()``.
    """
    candidate = Path(rel_path)
    if not candidate.is_absolute():
        candidate = repo_root / candidate
    try:
        resolved = candidate.resolve()
    except OSError:
        return None
    root = repo_root.resolve()
    if resolved != root and not resolved.is_relative_to(root):
        return None
    return resolved


# --- Process-level GraphStore cache (Section 2.3 in PERFORMANCE-IMPROVEMENTS) -
#
# Concurrent read-only MCP tool calls reuse a single :class:`GraphStore`
# instance per database file while any of them still holds a lease.  The cache
# key is the resolved :class:`Path` to the SQLite file; staleness is detected
# via ``(st_mtime, PRAGMA data_version)``.  When either moves, the previous
# instance is force-closed and a fresh one is created so that any mutation
# done by another connection (e.g. a write tool, the watch daemon,
# ``dagayn build``) is reflected -- including the derived caches
# (``_nxg_cache``, hub/bridge scores) that only a new instance resets.
#
# ``st_mtime`` alone was the original check and is not sufficient: the
# journal_mode is WAL, so another process's commit lands in ``graph.db-wal``
# and leaves the main file's mtime untouched until a checkpoint.  A long-lived
# ``dagayn serve`` therefore answered impact/topology questions from a
# NetworkX snapshot taken before the write, indefinitely.
#
# The last ``close()`` (leases → 0) closes the SQLite connection and drops the
# shared flock. Idle MCP must not keep a reader handle open: that is what
# overlapped ``dagayn build`` and tore ``sqlite_master`` under mmap+WAL.
# Concurrent overlapping ``_get_store`` calls still share one connection.
#
# Set ``DAGAYN_DISABLE_STORE_CACHE=1`` (or call :func:`_get_store` with
# ``cached=False``) to disable this and revert to a fresh ``GraphStore``
# per call — primarily useful for write tools (``build``, incremental
# update) that already manage their own short-lived connection.
_store_cache: dict[Path, tuple[GraphStore, tuple[float, int | None]]] = {}
_store_lock = threading.Lock()


def _cache_disabled() -> bool:
    return os.environ.get("DAGAYN_DISABLE_STORE_CACHE") == "1"


def _evict_store_cache(db_path: Path | None = None) -> None:
    """Evict cached stores, closing each one only when safe to do so.

    With *db_path* unset, every cached store is evicted.  Otherwise only
    the entry for *db_path* is dropped.

    Stores that are currently in use (``_leases > 0``) have their ``_pinned``
    flag cleared so that the last outstanding :meth:`~GraphStore.close` call
    performs the real cleanup — avoiding the race where a long-running read
    tool hits ``sqlite3`` errors on a connection closed by a concurrent
    build/postprocess tool.
    """
    with _store_lock:
        if db_path is None:
            entries = list(_store_cache.values())
            _store_cache.clear()
        else:
            entry = _store_cache.pop(db_path, None)
            entries = [entry] if entry is not None else []
        for store, _ in entries:
            store._pinned = False
            if store._leases == 0:
                # No in-flight callers — safe to close immediately.
                try:
                    store._force_close()
                except Exception:  # noqa: BLE001 — defensive cleanup  # nosec B110
                    pass
            # else: last close() will call _force_close when _leases reaches 0.


def _data_version(store: Any) -> int | None:
    """Return SQLite's ``data_version`` for *store*'s connection.

    ``PRAGMA data_version`` increments whenever *another* connection commits to
    the database, which is exactly the signal the cache needs and the one the
    file mtime does not give: in WAL mode a commit is written to ``-wal`` and
    the main database file's mtime does not move until a checkpoint.
    """
    conn = getattr(store, "_conn", None)
    if conn is None:
        return None
    try:
        row = conn.execute("PRAGMA data_version").fetchone()
    except sqlite3.Error:
        return None
    if row is None:
        return None
    return int(row[0])


def _get_store(
    repo_root: str | None = None,
    *,
    cached: bool = True,
    python_api: bool = False,
) -> tuple[GraphStore, Path]:
    """Resolve repo root and return a (possibly cached) graph store.

    Pass ``python_api=True`` for callers that need the full Python
    :class:`~dagayn.graph.GraphStore` surface (raw ``_conn`` access or query
    helpers the native store does not implement). The native store is a strict
    subset, so those callers would otherwise fail with ``AttributeError`` under
    the default Rust backend. See: #153
    """
    root = _validate_repo_root(Path(repo_root)) if repo_root else find_project_root()
    if not repo_root:
        # Before ``get_db_path``, which would create ``.dagayn`` there.
        reason = unsafe_root_reason(root)
        if reason is not None:
            raise ValueError(
                f"auto-detected repo_root is {reason} ({root}). Pass repo_root explicitly,"
                f" set CRG_REPO_ROOT, or give the MCP server entry a cwd/--repo for the"
                f" project; set {ALLOW_WIDE_ROOT_ENV}=1 to index it anyway."
            )
    db_path = get_db_path(root)
    set_repo_context(root, db_path, explicit=bool(repo_root))
    logger.debug(
        "resolved repo_root=%s (%s) graph=%s",
        root,
        "explicit" if repo_root else "auto-detected",
        db_path,
    )
    _assert_graph_matches_repo(db_path, root)
    owns_read_lock = not write_lock_is_held(db_path)
    if owns_read_lock:
        # A reader has to hold the shared lock for as long as its connection is
        # open (a connection left open across a writer's WAL checkpoint is what
        # tore sqlite_master), but it should not spend the writer's full budget
        # finding out that a build owns the graph right now.
        try:
            acquire_graph_lock(db_path, exclusive=False, timeout=DEFAULT_READ_LOCK_TIMEOUT)
        except WriteLockUnavailableError as exc:
            holder = lock_holder_pid(db_path)
            raise WriteLockUnavailableError(
                f"the graph at {db_path} is being written"
                f"{f' by pid {holder}' if holder else ''}"
                f" and did not become readable within {DEFAULT_READ_LOCK_TIMEOUT:g}s."
                " A build or embedding pass is in progress — retry shortly, or"
                " raise DAGAYN_READ_LOCK_TIMEOUT to wait longer."
            ) from exc
    try:
        store, root = _open_store(
            root,
            db_path,
            cached=cached,
            python_api=python_api,
        )
    except BaseException:
        if owns_read_lock:
            release_graph_lock(db_path)
        raise
    if owns_read_lock:
        # Bind after wrapping so native stores unbind on the same object
        # identity ``close()`` sees. Caching the sqlite handle is fine;
        # the flock itself must not outlive this caller's close().
        if not hasattr(store, "_conn"):
            store = wrap_store_close_to_unbind(store)
        bind_store_read_lock(store, db_path)
    return store, root


def _open_store(
    root: Path,
    db_path: Path,
    *,
    cached: bool,
    python_api: bool = False,
) -> tuple[GraphStore, Path]:
    store_cls = GraphStore if python_api else _selected_graph_store()
    if store_cls is not GraphStore:
        store = store_cls(db_path)
        register_live_store(store, db_path)
        return store, root

    if not cached or _cache_disabled():
        store = store_cls(db_path)
        store._leases = 1  # caller holds the only lease; close() will close
        return store, root

    try:
        mtime = db_path.stat().st_mtime
    except FileNotFoundError:
        # First-time use: nothing to cache yet, fall back to a fresh
        # transient store.  The next call will populate the cache once
        # the DB has been created.
        store = store_cls(db_path)
        store._leases = 1
        return store, root

    with _store_lock:
        entry = _store_cache.get(db_path)
        if entry is not None:
            cached_store, cached_token = entry
            cached_mtime, cached_version = cached_token
            # mtime alone misses WAL commits (see :func:`_data_version`); it is
            # still checked because a replaced file (worktree seeding) gives the
            # open handle no new data_version at all.
            current_version = _data_version(cached_store)
            if (
                cached_store._leases > 0
                and cached_mtime == mtime
                and cached_version == current_version
            ):
                # Acquire a lease atomically while holding the lock so
                # a concurrent _evict_store_cache cannot race between
                # the lookup and the increment.
                cached_store._leases += 1
                return cached_store, root
            # Stale or idle: drop and re-open.
            cached_store._pinned = False
            if cached_store._leases == 0:
                try:
                    cached_store._force_close()
                except Exception:  # noqa: BLE001 — defensive cleanup  # nosec B110
                    pass
            # else: last close() will _force_close when _leases reaches 0.
            _store_cache.pop(db_path, None)

        store = store_cls(db_path)
        store._pinned = True
        store._leases = 1  # set inside the lock before inserting into cache
        _store_cache[db_path] = (store, (mtime, _data_version(store)))
    return store, root


def _selected_graph_store() -> type:
    """Return the graph store selected by ``DAGAYN_BACKEND``.

    Rust is the default; source checkouts require the native extension.
    """
    if _backend_selection() != "rust":
        return GraphStore
    try:
        from dagayn._core import GraphStore as RustGraphStore

        return RustGraphStore
    except ImportError as exc:
        raise RuntimeError(
            "DAGAYN_BACKEND=rust requires dagayn._core. "
            "Install a wheel with the native extension or rebuild from source."
        ) from exc


def _answerability_sqlite_connection(store: Any) -> tuple[sqlite3.Connection | None, bool]:
    """Return a SQLite connection for answerability queries and whether to close it."""
    conn = getattr(store, "_conn", None)
    if conn is not None:
        return conn, False
    db_path = getattr(store, "db_path", None)
    if db_path is None and type(store).__module__ == "builtins":
        ctx = repo_context_snapshot()
        if ctx is not None:
            db_path = ctx.get("db_path")
    if not db_path:
        return None, False
    return sqlite3.connect(str(db_path), timeout=30), True


def _hints_from_next_tool_suggestions(
    next_tool_suggestions: list[str] | None,
) -> ToolHintsRecord | None:
    """Build a minimal ``_hints`` payload from plain-text tool suggestions."""
    if not next_tool_suggestions:
        return None

    from ..tool_surface import filter_suggestions

    next_steps: list[ToolHintStep] = []
    for suggestion in filter_suggestions(next_tool_suggestions)[:3]:
        head, _, tail = suggestion.partition(" -- ")
        tool = head.split(" ", 1)[0].split("(", 1)[0]
        next_steps.append(
            {
                "tool": tool,
                "suggestion": tail or head,
            }
        )
    return {
        "next_steps": next_steps,
        "related": [],
        "warnings": [],
    }


def make_guidance_item(
    *,
    claim: str,
    action: str | Mapping[str, object],
    evidence: Sequence[Mapping[str, object]] | Mapping[str, object] | None = None,
    confidence: str = "unknown",
    missingness: Sequence[Mapping[str, object]] | Mapping[str, object] | None = None,
    reason_codes: list[str] | None = None,
    counts: Mapping[str, object] | None = None,
    **extra: Any,
) -> ToolPayload:
    """Build and validate the shared guidance item shape used by workflow tools."""
    item: dict[str, object] = {
        "claim": claim,
        "evidence": evidence,
        "confidence": confidence,
        "missingness": missingness,
        "action": action,
        "reason_codes": list(reason_codes or []),
        "counts": dict(counts or {}),
    }
    item.update(extra)
    return cast(ToolPayload, seal_guidance_item(item))


def guidance_actions_to_hints(
    guidance: Sequence[Mapping[str, object]], *, limit: int = 3
) -> ToolHintsRecord:
    """Convert guidance actions into the existing ``_hints.next_steps`` shape."""
    next_steps: list[ToolHintStep] = []
    warnings: list[str] = []
    for item in guidance:
        action = item.get("action")
        if isinstance(action, Mapping):
            tool = str(action.get("tool") or "manual")
            suggestion = str(action.get("suggestion") or action.get("command") or tool)
        else:
            action_text = str(action or "")
            head, _, tail = action_text.partition(" -- ")
            tool = head.split(" ", 1)[0].split("(", 1)[0] if head else "manual"
            suggestion = tail or action_text
        if not suggestion:
            continue
        next_steps.append({"tool": tool, "suggestion": suggestion})
        raw_missingness = item.get("missingness") or []
        if isinstance(raw_missingness, Mapping):
            raw_missingness = [raw_missingness]
        for missing in cast(Sequence[Mapping[str, object]], raw_missingness):
            severity = str(missing.get("severity", "info"))
            code = missing.get("reason_code")
            if severity in {"medium", "high"} and code:
                warnings.append(str(code))
        if len(next_steps) >= limit:
            break
    return {"next_steps": next_steps, "related": [], "warnings": warnings}


def _freshness_reason_codes(store: Any) -> tuple[list[str], ToolPayload]:
    """Return freshness reason codes for the graph behind *store*.

    A graph that answers for the wrong commit, or that predates the edits in
    the working tree, produces confidently wrong answers -- ``callers_of`` on a
    new symbol returns "not found in the current graph", and a blast radius is
    computed from stale line ranges. Neither was visible on any read tool
    before: freshness lived only in ``session prepare`` and
    ``get_minimal_context``.
    """
    try:
        root = store.get_repo_root()
    except Exception:  # noqa: BLE001 — disclosure must never break a response
        return [], {}
    if root is None:
        return [], {}
    try:
        from .sync_status import commit_tier_freshness

        freshness = commit_tier_freshness(store, root)
    except Exception:  # noqa: BLE001
        return [], {}

    state = freshness.get("state")
    if state is None:
        return [], {}
    codes: list[str] = []
    if state == "commit_drift":
        codes.append("graph_describes_another_commit")
    elif freshness.get("worktree_dirty"):
        codes.append("uncommitted_changes_may_be_unindexed")
    counts = {
        "graph_head_sha": freshness.get("git_head_sha"),
        "current_head_sha": freshness.get("current_head_sha"),
        "worktree_dirty": bool(freshness.get("worktree_dirty")),
    }
    return codes, counts


def graph_answerability_summary(store: Any, stats: Any | None = None) -> AnswerabilityRecord:
    """Summarize whether the current graph can support calibrated claims."""
    if stats is None:
        try:
            stats = store.get_stats()
        except (AttributeError, sqlite3.Error):
            return seal_answerability_summary(
                {
                    "status": "unknown",
                    "score": 0.0,
                    "reason_codes": ["missing_graph_stats"],
                    "parse": [0, 0, False],
                }
            )
    conn, owns_conn = _answerability_sqlite_connection(store)
    if conn is None:
        return seal_answerability_summary(
            {
                "status": "unknown",
                "score": 0.0,
                "reason_codes": ["no_sqlite_connection"],
                "parse": [stats.files_count, len(stats.languages), bool(stats.last_updated)],
            }
        )

    query_failures: list[str] = []

    def _count(sql: str, params: tuple[Any, ...] = (), *, failure_code: str) -> int:
        try:
            row = conn.execute(sql, params).fetchone()
        except sqlite3.Error:
            query_failures.append(failure_code)
            return 0
        return int(row[0] if row else 0)

    try:
        flow_count = _count("SELECT COUNT(*) FROM flows", failure_code="missing_flows_table")
        community_count = _count(
            "SELECT COUNT(*) FROM communities",
            failure_code="missing_communities_table",
        )
        test_edge_count = int(stats.edges_by_kind.get("TESTED_BY", 0))
        cross_artifact_count = int(stats.edges_by_kind.get("CROSS_ARTIFACT", 0))
        unresolved_markdown_code_span_count = _count(
            "SELECT COUNT(*) FROM edges "
            "WHERE kind = 'CROSS_ARTIFACT' "
            "AND target_qualified LIKE '<unresolved:%' "
            "AND extra LIKE '%markdown_code_span%' "
            "AND extra LIKE '%code_span%'",
            failure_code="missing_cross_artifact_edge_metadata",
        )
        unresolved_cross_artifact_count = _count(
            "SELECT COUNT(*) FROM edges "
            "WHERE kind = 'CROSS_ARTIFACT' AND target_qualified LIKE '<unresolved:%'",
            failure_code="missing_cross_artifact_edges",
        )
        reportable_cross_artifact_count = max(
            0,
            cross_artifact_count - unresolved_markdown_code_span_count,
        )
        reportable_unresolved_cross_artifact_count = max(
            0,
            unresolved_cross_artifact_count - unresolved_markdown_code_span_count,
        )
        unresolved_ratio = (
            reportable_unresolved_cross_artifact_count / reportable_cross_artifact_count
            if reportable_cross_artifact_count
            else 0.0
        )

        stale_flow_membership_count = _count(
            "SELECT COUNT(*) FROM flow_memberships fm "
            "WHERE NOT EXISTS (SELECT 1 FROM nodes n WHERE n.id = fm.node_id)",
            failure_code="missing_flow_memberships_table",
        )
        unassigned_node_count = _count(
            "SELECT COUNT(*) FROM nodes n "
            "WHERE n.community_id IS NULL AND n.kind != 'File' "
            "AND EXISTS (SELECT 1 FROM communities LIMIT 1)",
            failure_code="missing_community_assignment_metadata",
        )

        reason_codes: list[str] = []
        score = 1.0
        if query_failures:
            reason_codes.extend(dict.fromkeys(query_failures))
            score -= 0.2
        if stats.total_nodes == 0 or stats.files_count == 0:
            reason_codes.append("empty_graph")
            score = 0.0
        if flow_count == 0:
            reason_codes.append("missing_flows")
            score -= 0.15
        if community_count == 0:
            reason_codes.append("missing_communities")
            score -= 0.15
        if test_edge_count == 0:
            reason_codes.append("missing_test_edges")
            score -= 0.1
        if cross_artifact_count and unresolved_ratio > 0.35:
            reason_codes.append("many_unresolved_cross_artifact_edges")
            score -= 0.15
        if not stats.last_updated:
            reason_codes.append("missing_last_updated")
            score -= 0.1
        if stale_flow_membership_count > 0 or unassigned_node_count > 0:
            reason_codes.append("stale_derived_structures")
            score -= 0.15
        freshness_codes, freshness_counts = _freshness_reason_codes(store)
        for code in freshness_codes:
            reason_codes.append(code)
            score -= 0.25 if code == "graph_describes_another_commit" else 0.1

        score = max(0.0, round(score, 4))
        status = "ok" if score >= 0.75 else "degraded" if score > 0 else "empty"
        health = {
            "status": status,
            "score": score,
            "reason_codes": reason_codes,
            "parse": [stats.files_count, len(stats.languages), bool(stats.last_updated)],
            "answerability": [
                flow_count,
                community_count,
                test_edge_count,
                reportable_cross_artifact_count,
                round(unresolved_ratio, 4),
            ],
            "counts": {
                "flows": flow_count,
                "communities": community_count,
                "test_edges": test_edge_count,
                "reportable_cross_artifact_edges": reportable_cross_artifact_count,
                "reportable_unresolved_cross_artifact_edges": (
                    reportable_unresolved_cross_artifact_count
                ),
                "stale_flow_memberships": stale_flow_membership_count,
                "unassigned_nodes": unassigned_node_count,
                **freshness_counts,
            },
        }
        if reportable_unresolved_cross_artifact_count:
            health["unresolved_edges"] = reportable_unresolved_cross_artifact_count
        return seal_answerability_summary(health)
    finally:
        if owns_conn:
            conn.close()


def attach_answerability(
    payload: ToolPayload,
    repo_root: str | None = None,
) -> ToolPayload:
    """Ensure a tool response carries answerability and missingness metadata."""
    attach_runtime_metadata(payload)
    if "answerability" in payload and "missingness" in payload:
        payload["answerability"] = seal_answerability_summary(
            cast(Mapping[str, object], payload["answerability"])
        )
        return payload

    try:
        store, _root = _get_store(repo_root)
    except Exception:
        answerability = seal_answerability_summary(
            {
                "status": "unknown",
                "score": 0.0,
                "reason_codes": ["answerability_unavailable"],
                "parse": [0, 0, False],
            }
        )
        if "answerability" not in payload:
            payload["answerability"] = answerability
        payload.setdefault(
            "missingness",
            missingness_from_answerability(answerability),
        )
        return payload

    try:
        answerability = graph_answerability_summary(store)
    finally:
        store.close()

    if "answerability" not in payload:
        payload["answerability"] = answerability
    else:
        payload["answerability"] = seal_answerability_summary(
            cast(Mapping[str, object], payload["answerability"])
        )
    payload.setdefault(
        "missingness",
        missingness_from_answerability(cast(Mapping[str, object], payload["answerability"])),
    )
    return payload


def missingness_from_answerability(
    answerability: Mapping[str, object],
) -> list[MissingnessRecord]:
    """Convert answerability reason codes into response-level missingness items."""
    severity_by_code = {
        "empty_graph": "high",
        "missing_flows": "medium",
        "missing_communities": "medium",
        "missing_test_edges": "medium",
        "many_unresolved_cross_artifact_edges": "medium",
        "missing_last_updated": "low",
        "no_sqlite_connection": "high",
        "missing_graph_stats": "medium",
        "missing_flows_table": "medium",
        "missing_communities_table": "medium",
        "missing_cross_artifact_edge_metadata": "medium",
        "missing_cross_artifact_edges": "medium",
        "answerability_unavailable": "medium",
        "stale_derived_structures": "medium",
        "graph_describes_another_commit": "high",
        "uncommitted_changes_may_be_unindexed": "medium",
    }
    claim_effect_by_code = {
        "graph_describes_another_commit": (
            "the graph answers for a different commit -- absence, line numbers and blast"
            " radius may all be wrong; run dagayn update before concluding anything"
        ),
        "uncommitted_changes_may_be_unindexed": (
            "working-tree edits may not be indexed -- a symbol reported missing may exist on disk"
        ),
    }
    items: list[MissingnessRecord] = []
    for code in cast(Sequence[object], answerability.get("reason_codes") or []):
        severity = severity_by_code.get(str(code), "low")
        items.append(
            seal_missingness_item(
                {
                    "reason_code": str(code),
                    "severity": severity,
                    "claim_effect": claim_effect_by_code.get(
                        str(code),
                        "claims should be treated as graph-limited until this is resolved",
                    ),
                }
            )
        )
    return items


def make_response(
    status: str,
    summary: str,
    *,
    hints: object | None = None,
    next_tool_suggestions: list[str] | None = None,
    **fields: object,
) -> ToolPayload:
    """Standard envelope: status / summary / fields / _hints / next_tool_suggestions.

    Ensures status and summary are always present and consistently ordered.
    """
    resp: ToolPayload = {"status": status, "summary": summary}
    resp.update(fields)
    if hints:
        resp["_hints"] = hints
    elif next_tool_suggestions:
        resp["_hints"] = _hints_from_next_tool_suggestions(next_tool_suggestions)
    if next_tool_suggestions:
        from ..tool_surface import filter_suggestions

        resp["next_tool_suggestions"] = filter_suggestions(next_tool_suggestions)[:3]
    return resp


def _db_path_for_repo(repo_root: str | Path | None) -> Path:
    """Resolve ``graph.db`` for *repo_root*, falling back to the process root."""
    root = Path(repo_root) if repo_root else find_project_root()
    return get_db_path(root)


def recover_corrupt_graph(db_path: str | Path | None = None) -> bool:
    """Drop cached / leaked SQLite handles so the next open is a new connection.

    ``SQLITE_CORRUPT`` sticks to a connection. Long-lived MCP processes also
    leak fds onto unlinked WAL generations; a fresh CLI process can read the
    same path while the server keeps failing. Closing every live handle on
    that path (or every handle, when *db_path* is omitted) is the recovery.

    Returns True when a brand-new connection can ``quick_check`` the file.
    """
    path = Path(db_path) if db_path is not None else None
    _evict_store_cache(path)
    close_live_stores_for(path)
    if path is None or str(path) == ":memory:":
        return True
    return probe_graph_database(path)


def handle_tool_runtime_error(
    exc: BaseException,
    *,
    logger: logging.Logger,
    context: str,
    repo_root: str | Path | None = None,
) -> ToolPayload:
    """Convert a tool failure into a structured MCP error envelope."""
    corrupt = is_sqlite_corrupt_error(exc)
    if corrupt:
        db_path = _db_path_for_repo(repo_root) if repo_root else None
        recovered = recover_corrupt_graph(db_path)
        logger.warning(
            "%s failed with sqlite corrupt (%s); closed live stores, file_ok=%s",
            context,
            exc,
            recovered,
        )
        reason_code = "sqlite_corrupt"
        next_action = (
            "Retry the tool. If it still fails, restart `dagayn serve` / reload "
            "the MCP server so leftover WAL handles are dropped. Rebuild with "
            "`dagayn build --local-embedding none` only when `PRAGMA quick_check` "
            "on `.dagayn/graph.db` is not ok."
        )
        claim_effect = (
            "graph queries are unavailable until poisoned SQLite connections "
            "are closed; the on-disk file may still be healthy"
        )
        payload: ToolPayload = {
            "status": "error",
            "error": str(exc),
            "file_ok": recovered,
            "next_action": next_action,
            "missingness": [
                {
                    "reason_code": reason_code,
                    "severity": "high",
                    "claim_effect": claim_effect,
                }
            ],
        }
        return payload
    if isinstance(exc, _TOOL_RUNTIME_ERRORS):
        logger.warning("%s failed: %s", context, exc)
        reason_code = "tool_runtime_error"
    else:
        logger.exception("%s failed unexpectedly", context)
        reason_code = "unexpected_tool_failure"
    return {
        "status": "error",
        "error": str(exc),
        "missingness": [
            {
                "reason_code": reason_code,
                "severity": "high",
                "claim_effect": (
                    "tool output is unavailable until the underlying failure is resolved"
                ),
            }
        ],
    }


def _get_path(container: dict[str, object], path: str) -> tuple[dict[str, object] | None, str]:
    current: object = container
    parts = path.split(".")
    for part in parts[:-1]:
        if not isinstance(current, dict):
            return None, parts[-1]
        current = current.get(part)
    return current if isinstance(current, dict) else None, parts[-1]


def apply_output_budget(
    payload: MutableMapping[str, DynamicValue],
    budget_tokens: int = 5000,
    list_priorities: list[str] | None = None,
) -> ToolPayload:
    """Trim list-valued fields until JSON size fits within budget_tokens.

    Mutates payload in-place. Sets payload["truncated"] = True and adds
    payload["_truncation"] = {field: {"kept": int, "total": int}} for each
    trimmed field.

    Fields in list_priorities are trimmed last-to-first (lowest priority
    trimmed first). Fields not in list_priorities are never touched.
    """
    mutable_payload = cast(dict[str, object], payload)
    if list_priorities is None:
        list_priorities = []

    def _est_tokens() -> int:
        return len(json.dumps(mutable_payload, default=str)) // 4

    if _est_tokens() <= budget_tokens:
        return cast(ToolPayload, payload)

    truncation: dict[str, dict[str, int]] = {}

    for field in reversed(list_priorities):
        parent, key = _get_path(mutable_payload, field)
        if parent is None or key not in parent:
            continue
        raw_items = parent[key]
        if not isinstance(raw_items, list):
            continue
        items = raw_items
        total = len(items)
        while len(items) > 1 and _est_tokens() > budget_tokens:
            items = items[: len(items) // 2]
            parent[key] = items
        if len(items) == 0:
            items = items[:1]
        if len(items) < total:
            parent[key] = items
            truncation[field] = {"kept": len(items), "total": total}
            mutable_payload["truncated"] = True
        if _est_tokens() <= budget_tokens:
            break

    if truncation:
        mutable_payload["_truncation"] = truncation
    elif _est_tokens() > budget_tokens:
        logger.warning(
            "apply_output_budget: payload still exceeds %d tokens after trimming all lists",
            budget_tokens,
        )
        mutable_payload["truncated"] = True

    return cast(ToolPayload, payload)


def projection_for_detail_level(
    item: Mapping[str, object],
    level: str,
    fields_minimal: list[str],
    fields_standard: list[str] | None = None,
) -> dict[str, object]:
    """Return a subset of item's fields based on detail_level.

    - "minimal": only fields_minimal keys
    - "standard": fields_minimal + fields_standard keys (or all if fields_standard is None)
    - "verbose": all keys
    """
    if level == "verbose":
        return dict(item)
    if level == "minimal":
        return {k: item[k] for k in fields_minimal if k in item}
    # standard
    if fields_standard is None:
        return dict(item)
    return {k: item[k] for k in fields_minimal + fields_standard if k in item}


def compact_response(
    summary: str,
    key_entities: list[str] | None = None,
    risk: str = "unknown",
    communities: list[str] | None = None,
    top_flows: list[str] | None = None,
    flows_affected: list[str] | None = None,
    next_tool_suggestions: list[str] | None = None,
    data: dict[str, object] | None = None,
    detail_level: str = "minimal",
) -> ToolPayload:
    """Standard compact response format for token efficiency."""
    resp: ToolPayload = {
        "status": "ok",
        "summary": summary,
    }
    if key_entities:
        resp["key_entities"] = key_entities[:10]
    if risk != "unknown":
        resp["risk"] = risk
    if communities:
        resp["communities"] = communities[:5]
    if top_flows:
        resp["top_flows"] = top_flows[:5]
    if flows_affected:
        resp["flows_affected"] = flows_affected[:5]
    if next_tool_suggestions:
        from ..tool_surface import filter_suggestions

        filtered = filter_suggestions(next_tool_suggestions)
        resp["next_tool_suggestions"] = filtered[:3]
        resp["_hints"] = _hints_from_next_tool_suggestions(filtered)
    if detail_level != "minimal" and data:
        resp["data"] = data
    return resp
