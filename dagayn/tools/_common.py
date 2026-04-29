"""Shared utilities for tool sub-modules."""

from __future__ import annotations

import os
import threading
from pathlib import Path
from typing import Any

from ..graph import GraphStore
from ..incremental import find_project_root, get_db_path


def _error_response(
    message: str,
    status: str = "error",
    **extra: Any,
) -> dict[str, Any]:
    """Build a standardised error response dict."""
    return {"status": status, "error": message, "summary": message, **extra}


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


# --- Process-level GraphStore cache (Section 2.3 in PERFORMANCE-IMPROVEMENTS) -
#
# Read-only MCP tool calls reuse a single :class:`GraphStore` instance per
# database file across invocations.  The cache key is the resolved
# :class:`Path` to the SQLite file; staleness is detected via ``st_mtime``
# (which changes on every WAL commit).  When the file mtime no longer
# matches what we cached, the previous instance is force-closed and a fresh
# one is created so that any mutation done by another connection (e.g. a
# write tool, the watch daemon, ``dagayn build``) is reflected.
#
# Cached stores have ``_pinned = True``; their :meth:`GraphStore.close`
# becomes a no-op so existing ``finally: store.close()`` blocks in tool
# handlers continue to work but no longer destroy the connection.
#
# Set ``DAGAYN_DISABLE_STORE_CACHE=1`` (or call :func:`_get_store` with
# ``cached=False``) to disable this and revert to a fresh ``GraphStore``
# per call — primarily useful for write tools (``build``, incremental
# update) that already manage their own short-lived connection.
_store_cache: dict[Path, tuple[GraphStore, float]] = {}
_store_lock = threading.Lock()


def _cache_disabled() -> bool:
    return os.environ.get("DAGAYN_DISABLE_STORE_CACHE") == "1"


def _evict_store_cache(db_path: Path | None = None) -> None:
    """Evict and force-close cached stores.

    With *db_path* unset, every cached store is closed and the cache is
    cleared.  Otherwise only the entry for *db_path* is dropped.
    """
    with _store_lock:
        if db_path is None:
            for store, _ in _store_cache.values():
                store._pinned = False
                try:
                    store._force_close()
                except Exception:  # noqa: BLE001 — defensive cleanup
                    pass
            _store_cache.clear()
            return
        entry = _store_cache.pop(db_path, None)
        if entry is not None:
            store, _ = entry
            store._pinned = False
            try:
                store._force_close()
            except Exception:  # noqa: BLE001
                pass


def _get_store(
    repo_root: str | None = None,
    *,
    cached: bool = True,
) -> tuple[GraphStore, Path]:
    """Resolve repo root and return a (possibly cached) graph store."""
    root = _validate_repo_root(Path(repo_root)) if repo_root else find_project_root()
    db_path = get_db_path(root)

    if not cached or _cache_disabled():
        return GraphStore(db_path), root

    try:
        mtime = db_path.stat().st_mtime
    except FileNotFoundError:
        # First-time use: nothing to cache yet, fall back to a fresh
        # transient store.  The next call will populate the cache once
        # the DB has been created.
        return GraphStore(db_path), root

    with _store_lock:
        entry = _store_cache.get(db_path)
        if entry is not None:
            cached_store, cached_mtime = entry
            if cached_mtime == mtime:
                return cached_store, root
            # Stale: drop and re-open.
            cached_store._pinned = False
            try:
                cached_store._force_close()
            except Exception:  # noqa: BLE001
                pass
            _store_cache.pop(db_path, None)

        store = GraphStore(db_path)
        store._pinned = True
        _store_cache[db_path] = (store, mtime)
    return store, root


def compact_response(
    summary: str,
    key_entities: list[str] | None = None,
    risk: str = "unknown",
    communities: list[str] | None = None,
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
    if flows_affected:
        resp["flows_affected"] = flows_affected[:5]
    if next_tool_suggestions:
        resp["next_tool_suggestions"] = next_tool_suggestions[:3]
    if detail_level != "minimal" and data:
        resp["data"] = data
    return resp
