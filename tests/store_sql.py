"""SQLite access for tests that inspect a Rust GraphStore.

The native store does not expose ``_conn``. Tests that need SQL open a
second connection to the same file. WAL commits from either side are
visible after ``commit()``.

Rust ``GraphStore`` is not weak-referenceable, so connections are cached
by ``id(store)`` for the life of the process.
"""

from __future__ import annotations

import sqlite3
from collections.abc import Iterator
from contextlib import contextmanager
from typing import Any

_cached: dict[int, sqlite3.Connection] = {}


def store_conn(store: Any) -> sqlite3.Connection:
    """Return a cached sqlite3 connection for *store*'s database file."""
    key = id(store)
    conn = _cached.get(key)
    if conn is not None:
        return conn
    db_path = getattr(store, "db_path", None)
    if db_path is None:
        raise AttributeError(f"{type(store).__name__} has no db_path")
    conn = sqlite3.connect(str(db_path), timeout=30, isolation_level=None)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA busy_timeout=5000")
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
