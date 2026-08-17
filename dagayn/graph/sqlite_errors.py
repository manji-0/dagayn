"""SQLite corruption detection and live GraphStore tracking.

``SQLITE_CORRUPT`` is sticky on a connection: a long-lived ``dagayn serve``
that keeps a poisoned handle (or leaked fds to an unlinked WAL generation)
keeps returning ``database disk image is malformed`` even after the on-disk
file is healthy. Recovery is to close every live handle on that path so the
next open is a new SQLite connection.
"""

from __future__ import annotations

import logging
import os
import sqlite3
import time
import weakref
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)

_CORRUPT_MESSAGE_MARKERS = (
    "database disk image is malformed",
    "malformed database schema",
    # SQLite words SQLITE_NOTADB either as "file is encrypted or is not a
    # database" or, on newer builds, just "file is not a database". Matching the
    # shared tail covers both; the long form alone let a truncated graph.db
    # escape as a traceback out of the CLI.
    "is not a database",
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


#: Suffix pattern for quarantined graph files. Kept on disk rather than deleted
#: so a corrupt image can still be inspected (``sqlite3 .recover``) afterwards.
CORRUPT_SUFFIX_FORMAT = ".corrupt-%Y%m%d%H%M%S"


def quarantine_corrupt_database(db_path: str | Path) -> Path | None:
    """Move a corrupt graph database (plus ``-wal`` / ``-shm``) aside.

    A corrupt image cannot be repaired in place: every subsequent open keeps
    raising ``database disk image is malformed``, so an unattended hook would
    fail on every invocation forever. Renaming it lets the next
    ``ensure_graph`` / ``build`` start from a clean file while keeping the
    broken one for post-mortem.

    Returns:
        Path the database was moved to, or ``None`` when there was nothing to
        move or the rename failed (a read-only directory, for instance).
    """
    path = Path(db_path)
    if str(path) == ":memory:" or not path.exists():
        return None

    close_live_stores_for(path)

    suffix = time.strftime(CORRUPT_SUFFIX_FORMAT)
    dest = path.with_name(path.name + suffix)
    try:
        os.replace(path, dest)
    except OSError:
        logger.debug("could not quarantine corrupt database %s", path, exc_info=True)
        return None

    # -wal / -shm belong to the quarantined image. Leaving them next to a fresh
    # database makes SQLite treat the new file as the same generation.
    for sidecar_suffix in ("-wal", "-shm"):
        sidecar = path.with_name(path.name + sidecar_suffix)
        if not sidecar.exists():
            continue
        try:
            os.replace(sidecar, dest.with_name(dest.name + sidecar_suffix))
        except OSError:
            logger.debug("could not quarantine %s", sidecar, exc_info=True)

    logger.warning("Quarantined corrupt graph database: %s -> %s", path, dest)
    return dest


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
