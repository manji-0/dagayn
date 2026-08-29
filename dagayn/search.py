"""Thin search API. FTS rebuild and hybrid ranking live in ``dagayn.legacy_py.search``.

``rebuild_fts_index`` stays on the native store when that method exists.
Hybrid search (embeddings + RRF) remains a Python product surface and is
loaded from the legacy module on first use.
"""

from __future__ import annotations

import logging
from typing import Any, Callable, cast

from .graph import GraphStore

logger = logging.getLogger(__name__)


def _legacy() -> Any:
    from dagayn.legacy_py import search as impl

    return impl


def rebuild_fts_index(store: GraphStore) -> int:
    """Rebuild the FTS5 index from the nodes table."""
    rust_rebuild = getattr(store, "rebuild_fts_index", None)
    if callable(rust_rebuild):
        count = cast(Callable[[], int], rust_rebuild)()
        logger.info("FTS index rebuilt: %d rows indexed", count)
        return count
    return _legacy().rebuild_fts_index(store)


def __getattr__(name: str) -> Any:
    value = getattr(_legacy(), name)
    globals()[name] = value
    return value


__all__ = [
    "rebuild_fts_index",
]
