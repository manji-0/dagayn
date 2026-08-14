"""Shared utilities for tool sub-modules."""

from __future__ import annotations

import json
import logging
import os
import sqlite3
import sys
import threading
from collections.abc import Mapping
from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as pkg_version
from pathlib import Path
from typing import Any

from ..graph import GraphStore
from ..incremental import (
    _backend_selection,
    _rust_backend_explicitly_requested,
    find_project_root,
    get_db_path,
)
from ..state_types import (
    seal_answerability_summary,
    seal_guidance_item,
    seal_missingness_item,
)

logger = logging.getLogger(__name__)

_TOOL_RUNTIME_ERRORS: tuple[type[BaseException], ...] = (
    OSError,
    sqlite3.Error,
    ValueError,
    KeyError,
    TypeError,
    AttributeError,
)


def _error_response(
    message: str,
    status: str = "error",
    **extra: Any,
) -> dict[str, Any]:
    """Build a standardised error response dict."""
    return {"status": status, "error": message, "summary": message, **extra}


def tool_runtime_summary() -> dict[str, Any]:
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


def attach_runtime_metadata(payload: dict[str, Any]) -> dict[str, Any]:
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

    Ensures the path is an existing directory that contains a ``.git``
    or ``.dagayn`` directory, preventing arbitrary file-system
    traversal via the ``repo_root`` parameter.
    """
    resolved = path.resolve()
    if not resolved.is_dir():
        raise ValueError(f"repo_root is not an existing directory: {resolved}")
    if not (resolved / ".git").exists() and not (resolved / ".dagayn").exists():
        raise ValueError(
            f"repo_root does not look like a project root (no .git or "
            f".dagayn directory found): {resolved}"
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
# Read-only MCP tool calls reuse a single :class:`GraphStore` instance per
# database file across invocations.  The cache key is the resolved
# :class:`Path` to the SQLite file; staleness is detected via
# ``(st_mtime, PRAGMA data_version)``.  When either moves, the previous
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
# Cached stores have ``_pinned = True``; their :meth:`GraphStore.close`
# becomes a no-op so existing ``finally: store.close()`` blocks in tool
# handlers continue to work but no longer destroy the connection.
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
    use_backend_default: bool = False,
) -> tuple[GraphStore, Path]:
    """Resolve repo root and return a (possibly cached) graph store."""
    root = _validate_repo_root(Path(repo_root)) if repo_root else find_project_root()
    db_path = get_db_path(root)
    store_cls = _selected_graph_store(use_backend_default=use_backend_default)
    if store_cls is not GraphStore:
        return store_cls(db_path), root

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
            if cached_mtime == mtime and cached_version == current_version:
                # Acquire a lease atomically while holding the lock so
                # a concurrent _evict_store_cache cannot race between
                # the lookup and the increment.
                cached_store._leases += 1
                return cached_store, root
            # Stale: drop and re-open.
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


def _selected_graph_store(*, use_backend_default: bool = True) -> type:
    """Return the graph store selected by DAGAYN_BACKEND.

    The Rust backend is the default for write-heavy tool flows. Source
    checkouts require the native extension.
    """
    if _backend_selection() != "rust":
        return GraphStore
    if not use_backend_default and not _rust_backend_explicitly_requested():
        return GraphStore
    try:
        from dagayn._core import GraphStore as RustGraphStore

        return RustGraphStore
    except ImportError as exc:
        raise RuntimeError(
            "DAGAYN_BACKEND=rust requires dagayn._core. "
            "Install a wheel with the native extension or rebuild from source."
        ) from exc


def _hints_from_next_tool_suggestions(
    next_tool_suggestions: list[str] | None,
) -> dict[str, Any] | None:
    """Build a minimal ``_hints`` payload from plain-text tool suggestions."""
    if not next_tool_suggestions:
        return None

    from ..tool_surface import filter_suggestions

    next_steps: list[dict[str, str]] = []
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
    action: str | dict[str, Any],
    evidence: list[dict[str, Any]] | dict[str, Any] | None = None,
    confidence: str = "unknown",
    missingness: list[dict[str, Any]] | dict[str, Any] | None = None,
    reason_codes: list[str] | None = None,
    counts: dict[str, Any] | None = None,
    **extra: Any,
) -> dict[str, Any]:
    """Build and validate the shared guidance item shape used by workflow tools."""
    item: dict[str, Any] = {
        "claim": claim,
        "evidence": evidence,
        "confidence": confidence,
        "missingness": missingness,
        "action": action,
        "reason_codes": list(reason_codes or []),
        "counts": dict(counts or {}),
    }
    item.update(extra)
    return seal_guidance_item(item)


def guidance_actions_to_hints(guidance: list[dict[str, Any]], *, limit: int = 3) -> dict[str, Any]:
    """Convert guidance actions into the existing ``_hints.next_steps`` shape."""
    next_steps: list[dict[str, str]] = []
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
        for missing in item.get("missingness", []):
            severity = str(missing.get("severity", "info"))
            code = missing.get("reason_code")
            if severity in {"medium", "high"} and code:
                warnings.append(str(code))
        if len(next_steps) >= limit:
            break
    return {"next_steps": next_steps, "related": [], "warnings": warnings}


def _freshness_reason_codes(store: Any) -> tuple[list[str], dict[str, Any]]:
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


def graph_answerability_summary(store: Any, stats: Any | None = None) -> dict[str, Any]:
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
    conn = getattr(store, "_conn", None)
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


def attach_answerability(
    payload: dict[str, Any],
    repo_root: str | None = None,
) -> dict[str, Any]:
    """Ensure a tool response carries answerability and missingness metadata."""
    attach_runtime_metadata(payload)
    if "answerability" in payload and "missingness" in payload:
        payload["answerability"] = seal_answerability_summary(payload["answerability"])
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
        payload.setdefault("missingness", missingness_from_answerability(answerability))
        return payload

    try:
        answerability = graph_answerability_summary(store)
    finally:
        store.close()

    if "answerability" not in payload:
        payload["answerability"] = answerability
    else:
        payload["answerability"] = seal_answerability_summary(payload["answerability"])
    payload.setdefault("missingness", missingness_from_answerability(payload["answerability"]))
    return payload


def missingness_from_answerability(answerability: dict[str, Any]) -> list[dict[str, Any]]:
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
    items: list[dict[str, Any]] = []
    for code in answerability.get("reason_codes", []):
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
    hints: Any | None = None,
    next_tool_suggestions: list[str] | None = None,
    **fields: Any,
) -> dict[str, Any]:
    """Standard envelope: status / summary / fields / _hints / next_tool_suggestions.

    Ensures status and summary are always present and consistently ordered.
    """
    resp: dict[str, Any] = {"status": status, "summary": summary}
    resp.update(fields)
    if hints:
        resp["_hints"] = hints
    elif next_tool_suggestions:
        resp["_hints"] = _hints_from_next_tool_suggestions(next_tool_suggestions)
    if next_tool_suggestions:
        from ..tool_surface import filter_suggestions

        resp["next_tool_suggestions"] = filter_suggestions(next_tool_suggestions)[:3]
    return resp


def handle_tool_runtime_error(
    exc: BaseException,
    *,
    logger: logging.Logger,
    context: str,
) -> dict[str, Any]:
    """Convert a tool failure into a structured MCP error envelope."""
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


def _get_path(container: dict[str, Any], path: str) -> tuple[dict[str, Any] | None, str]:
    current: Any = container
    parts = path.split(".")
    for part in parts[:-1]:
        if not isinstance(current, dict):
            return None, parts[-1]
        current = current.get(part)
    return current if isinstance(current, dict) else None, parts[-1]


def apply_output_budget(
    payload: dict[str, Any],
    budget_tokens: int = 5000,
    list_priorities: list[str] | None = None,
) -> dict[str, Any]:
    """Trim list-valued fields until JSON size fits within budget_tokens.

    Mutates payload in-place. Sets payload["truncated"] = True and adds
    payload["_truncation"] = {field: {"kept": int, "total": int}} for each
    trimmed field.

    Fields in list_priorities are trimmed last-to-first (lowest priority
    trimmed first). Fields not in list_priorities are never touched.
    """
    if list_priorities is None:
        list_priorities = []

    def _est_tokens() -> int:
        return len(json.dumps(payload, default=str)) // 4

    if _est_tokens() <= budget_tokens:
        return payload

    truncation: dict[str, dict[str, int]] = {}

    for field in reversed(list_priorities):
        parent, key = _get_path(payload, field)
        if parent is None or key not in parent or not isinstance(parent[key], list):
            continue
        items = parent[key]
        total = len(items)
        while len(items) > 1 and _est_tokens() > budget_tokens:
            items = items[: len(items) // 2]
            parent[key] = items
        if len(items) == 0:
            items = parent[key][:1]
        if len(items) < total:
            parent[key] = items
            truncation[field] = {"kept": len(items), "total": total}
            payload["truncated"] = True
        if _est_tokens() <= budget_tokens:
            break

    if truncation:
        payload["_truncation"] = truncation
    elif _est_tokens() > budget_tokens:
        logger.warning(
            "apply_output_budget: payload still exceeds %d tokens after trimming all lists",
            budget_tokens,
        )
        payload["truncated"] = True

    return payload


def projection_for_detail_level(
    item: Mapping[str, Any],
    level: str,
    fields_minimal: list[str],
    fields_standard: list[str] | None = None,
) -> dict[str, Any]:
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
    data: dict[str, Any] | None = None,
    detail_level: str = "minimal",
) -> dict[str, Any]:
    """Standard compact response format for token efficiency."""
    resp: dict[str, Any] = {
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
