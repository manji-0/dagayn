"""Standalone helper functions for graph data serialization."""

from __future__ import annotations

import logging
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from typing import Any

from .types import GraphEdge, GraphNode

logger = logging.getLogger(__name__)

_FALLBACK_WRITE_LOCK = threading.RLock()


@contextmanager
def store_write_transaction(store: Any) -> Iterator[Any]:
    """Run an explicit ``BEGIN IMMEDIATE`` region on *store*'s connection.

    Holds the store's write lock for the whole region. Several call sites used
    to open one of these after rolling back "whatever transaction is open",
    which silently destroyed another thread's in-flight work -- and two
    ``BEGIN IMMEDIATE`` on one connection is an error outright. The connection
    is shared across threads (``check_same_thread=False``), so the region needs
    real mutual exclusion, not a recovery heuristic.

    Yields the connection. Commits on success, rolls back on any exception.
    """
    conn = store._conn
    lock = getattr(store, "_write_lock", None) or _FALLBACK_WRITE_LOCK
    with lock:
        if conn.in_transaction:
            # Already inside a transaction on this connection. Holding the lock
            # means it is *ours* (the lock is reentrant per thread), so join it:
            # nesting BEGIN IMMEDIATE is an error, and committing it early --
            # what one caller used to do -- would publish a half-finished write.
            yield conn
            return
        conn.execute("BEGIN IMMEDIATE")
        try:
            yield conn
        except BaseException:
            try:
                conn.rollback()
            except Exception:  # noqa: BLE001 - nothing useful to do here
                logger.debug("Rollback failed", exc_info=True)
            raise
        conn.commit()


def _sanitize_name(s: str, max_len: int = 256) -> str:
    """Strip ASCII control characters and truncate to prevent prompt injection.

    Node names extracted from source code could contain adversarial strings
    (e.g. ``IGNORE_ALL_PREVIOUS_INSTRUCTIONS``).  This function removes control
    characters (0x00-0x1F except tab and newline) and enforces a length limit so
    that names flowing through MCP tool responses cannot easily influence AI
    agent behaviour.
    """
    # Strip control chars 0x00-0x1F except \t (0x09) and \n (0x0A)
    cleaned = "".join(ch for ch in s if ch in ("\t", "\n") or ord(ch) >= 0x20)
    return cleaned[:max_len]


def node_to_dict(n: GraphNode) -> dict:
    return {
        "id": n.id,
        "kind": n.kind,
        "name": _sanitize_name(n.name),
        "qualified_name": _sanitize_name(n.qualified_name),
        "file_path": n.file_path,
        "line_start": n.line_start,
        "line_end": n.line_end,
        "language": n.language,
        "parent_name": _sanitize_name(n.parent_name) if n.parent_name else n.parent_name,
        "is_test": n.is_test,
    }


def edge_to_dict(e: GraphEdge) -> dict:
    payload = {
        "id": e.id,
        "kind": e.kind,
        "source": _sanitize_name(e.source_qualified),
        "target": _sanitize_name(e.target_qualified),
        "file_path": e.file_path,
        "line": e.line,
        "confidence": e.confidence,
        "confidence_tier": e.confidence_tier,
    }
    # Preserve bridge metadata so impact/flow consumers can explain transitions.
    if e.kind == "CROSS_ARTIFACT" and isinstance(e.extra, dict) and e.extra:
        payload["extra"] = {
            key: e.extra[key]
            for key in (
                "relationship_role",
                "bridge_kind",
                "evidence_kind",
                "evidence_source",
                "source_language",
                "target_language",
                "confidence_tier",
            )
            if key in e.extra
        }
    return payload
