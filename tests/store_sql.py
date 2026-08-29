"""SQLite access for tests that inspect a Rust GraphStore.

The native store does not expose ``_conn``. Tests that need SQL open a
second connection to the same file. WAL commits from either side are
visible after ``commit()``.

Connections are cached by database path. Keying by ``id(store)`` reused a
stale handle when a later store object recycled the same id.
"""

from __future__ import annotations

import sqlite3
from collections.abc import Iterator
from contextlib import contextmanager
from typing import Any

from dagayn.sqlite_tuning import apply_wal_size_limit

_cached: dict[str, sqlite3.Connection] = {}


def store_conn(store: Any) -> sqlite3.Connection:
    """Return a cached sqlite3 connection for *store*'s database file."""
    db_path = getattr(store, "db_path", None)
    if db_path is None:
        raise AttributeError(f"{type(store).__name__} has no db_path")
    key = str(db_path)
    conn = _cached.get(key)
    if conn is not None:
        return conn
    conn = sqlite3.connect(key, timeout=30, isolation_level=None)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA busy_timeout=5000")
    apply_wal_size_limit(conn)
    _cached[key] = conn
    return conn


@contextmanager
def store_sql(store: Any) -> Iterator[sqlite3.Connection]:
    """Yield a short-lived connection and commit on success."""
    conn = sqlite3.connect(str(store.db_path), timeout=30)
    conn.row_factory = sqlite3.Row
    try:
        yield conn
        conn.commit()
    finally:
        conn.close()
