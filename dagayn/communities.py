"""Thin community API. Algorithms live in ``dagayn.legacy_py.communities``."""

from __future__ import annotations

import json
import logging
from typing import Any, Callable, cast

from .graph import GraphStore

logger = logging.getLogger(__name__)


def _legacy() -> Any:
    from dagayn.legacy_py import communities as impl

    return impl


def detect_communities(store: GraphStore, min_size: int = 2) -> list[Any]:
    """Detect communities in the code graph."""
    native = getattr(store, "detect_communities_json", None)
    if callable(native):
        payload = json.loads(cast(str, native(min_size)))
        results: list[Any] = []
        for item in payload:
            if not isinstance(item, dict):
                continue
            results.append(
                {
                    "name": str(item.get("name") or "community"),
                    "level": int(item.get("level") or 0),
                    "size": int(item.get("size") or 0),
                    "cohesion": float(item.get("cohesion") or 0.0),
                    "dominant_language": str(item.get("dominant_language") or ""),
                    "description": str(item.get("description") or ""),
                    "members": [str(member) for member in (item.get("members") or [])],
                }
            )
        return results
    return _legacy().detect_communities(store, min_size=min_size)


def count_affected_communities(store: GraphStore, changed_files: list[str]) -> int:
    """Return how many communities are affected by *changed_files*."""
    if not changed_files:
        return 0
    rust_count = getattr(store, "count_affected_communities", None)
    if callable(rust_count):
        return cast(Callable[[list[str]], int], rust_count)(changed_files)
    return _legacy().count_affected_communities(store, changed_files)


def incremental_detect_communities(
    store: GraphStore,
    changed_files: list[str],
    min_size: int = 2,
    pre_affected_count: int | None = None,
) -> int:
    """Re-detect communities only if changed files affect existing communities."""
    if not changed_files:
        return 0
    native = getattr(store, "incremental_detect_communities", None)
    if callable(native):
        return int(cast(int, native(changed_files, min_size, pre_affected_count)))
    return _legacy().incremental_detect_communities(
        store,
        changed_files,
        min_size=min_size,
        pre_affected_count=pre_affected_count,
    )


def store_communities(store: GraphStore, communities: list[Any]) -> int:
    """Store detected communities in the database."""
    rust_store = getattr(store, "store_communities_json", None)
    if callable(rust_store):
        payload = [
            {
                "name": comm["name"],
                "level": comm.get("level", 0),
                "cohesion": comm.get("cohesion", 0.0),
                "size": comm["size"],
                "dominant_language": comm.get("dominant_language", ""),
                "description": comm.get("description", ""),
                "members": list(comm.get("members", [])),
            }
            for comm in communities
        ]
        return cast(Callable[[str], int], rust_store)(json.dumps(payload))
    return _legacy().store_communities(store, communities)


def get_communities(store: GraphStore, sort_by: str = "size", min_size: int = 0) -> list[Any]:
    """Retrieve stored communities from the database."""
    valid_sorts = {"size", "cohesion", "name"}
    if sort_by not in valid_sorts:
        sort_by = "size"
    rust_get = getattr(store, "get_communities_json", None)
    if callable(rust_get):
        return json.loads(cast(Callable[[str, int], str], rust_get)(sort_by, min_size))
    return _legacy().get_communities(store, sort_by=sort_by, min_size=min_size)


def __getattr__(name: str) -> Any:
    value = getattr(_legacy(), name)
    globals()[name] = value
    return value


__all__ = [
    "count_affected_communities",
    "detect_communities",
    "get_communities",
    "incremental_detect_communities",
    "store_communities",
]
