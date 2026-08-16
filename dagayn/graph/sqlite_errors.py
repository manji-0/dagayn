"""SQLite corruption detection and live GraphStore tracking.

``SQLITE_CORRUPT`` is sticky on a connection: a long-lived ``dagayn serve``
that keeps a poisoned handle (or leaked fds to an unlinked WAL generation)
keeps returning ``database disk image is malformed`` even after the on-disk
file is healthy. Recovery is to close every live handle on that path so the
next open is a new SQLite connection.
"""

from __future__ import annotations

import logging
import sqlite3
import weakref
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)

_CORRUPT_MESSAGE_MARKERS = (
    "database disk image is malformed",
    "malformed database schema",
    "file is encrypted or is not a database",
    "sqlite_corrupt",
)

_live_stores: weakref.WeakKeyDictionary[Any, Path] = weakref.WeakKeyDictionary()


def is_sqlite_corrupt_error(exc: BaseException) -> bool:
    """True when *exc* is SQLite reporting a corrupt / torn image."""
    message = str(exc).lower()
    if any(marker in message for marker in _CORRUPT_MESSAGE_MARKERS):
        return True
    code = getattr(exc, "sqlite_errorcode", None)
    corrupt_code = getattr(sqlite3, "SQLITE_CORRUPT", 11)
    return code == corrupt_code


def register_live_store(store: Any, db_path: str | Path) -> None:
    """Track an open graph store so corrupt recovery can force-close it."""
    path = Path(db_path)
    if str(path) == ":memory:":
        return
    try:
        resolved = path.resolve()
    except OSError:
        resolved = path
    try:
        _live_stores[store] = resolved
    except TypeError:
        logger.debug("store is not weak-referenceable; corrupt recovery cannot track it")


def close_live_stores_for(db_path: str | Path | None = None) -> int:
    """Force-close live stores, optionally limited to *db_path*.

    Returns the number of stores on which ``_force_close`` / ``close`` was
    attempted. Used when ``SQLITE_CORRUPT`` poisons a long-lived MCP process.
    """
    target: Path | None = None
    if db_path is not None and str(db_path) != ":memory:":
        try:
            target = Path(db_path).resolve()
        except OSError:
            target = Path(db_path)

    closed = 0
    for store, stored_path in list(_live_stores.items()):
        if target is not None and stored_path != target:
            continue
        closer = getattr(store, "_force_close", None) or getattr(store, "close", None)
        if closer is None:
            continue
        try:
            closer()
        except Exception:  # noqa: BLE001 — recovery must not raise
            logger.debug("force-close during corrupt recovery failed", exc_info=True)
        closed += 1
    return closed


def probe_graph_database(db_path: str | Path) -> bool:
    """True when a fresh connection can ``quick_check`` the file."""
    path = Path(db_path)
    if str(path) == ":memory:" or not path.is_file():
        return False
    try:
        conn = sqlite3.connect(str(path), timeout=5)
    except sqlite3.Error:
        return False
    try:
        try:
            conn.execute("PRAGMA wal_checkpoint(PASSIVE)")
        except sqlite3.Error:
            pass
        row = conn.execute("PRAGMA quick_check").fetchone()
        return row is not None and str(row[0]) == "ok"
    except sqlite3.Error:
        return False
    finally:
        conn.close()
