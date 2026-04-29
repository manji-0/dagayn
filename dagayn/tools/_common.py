"""Shared utilities for tool sub-modules."""

from __future__ import annotations

import json
import logging
import os
import threading
from collections.abc import Mapping
from pathlib import Path
from typing import Any

from ..graph import GraphStore
from ..incremental import find_project_root, get_db_path

logger = logging.getLogger(__name__)


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


def _get_store(
    repo_root: str | None = None,
    *,
    cached: bool = True,
) -> tuple[GraphStore, Path]:
    """Resolve repo root and return a (possibly cached) graph store."""
    root = _validate_repo_root(Path(repo_root)) if repo_root else find_project_root()
    db_path = get_db_path(root)

    if not cached or _cache_disabled():
        store = GraphStore(db_path)
        store._leases = 1  # caller holds the only lease; close() will close
        return store, root

    try:
        mtime = db_path.stat().st_mtime
    except FileNotFoundError:
        # First-time use: nothing to cache yet, fall back to a fresh
        # transient store.  The next call will populate the cache once
        # the DB has been created.
        store = GraphStore(db_path)
        store._leases = 1
        return store, root

    with _store_lock:
        entry = _store_cache.get(db_path)
        if entry is not None:
            cached_store, cached_mtime = entry
            if cached_mtime == mtime:
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

        store = GraphStore(db_path)
        store._pinned = True
        store._leases = 1  # set inside the lock before inserting into cache
        _store_cache[db_path] = (store, mtime)
    return store, root


def make_response(
    status: str,
    summary: str,
    *,
    hints: list[str] | None = None,
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
    if next_tool_suggestions:
        resp["next_tool_suggestions"] = next_tool_suggestions[:3]
    return resp


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
        if field not in payload or not isinstance(payload[field], list):
            continue
        items = payload[field]
        total = len(items)
        while len(items) > 1 and _est_tokens() > budget_tokens:
            items = items[: len(items) // 2]
        if len(items) == 0:
            items = payload[field][:1]
        if len(items) < total:
            payload[field] = items
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
