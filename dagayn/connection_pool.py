"""SQLite connection pooling for dagayn graph databases."""

from __future__ import annotations

import logging
import sqlite3
import threading
from collections import OrderedDict
from pathlib import Path

from .sqlite_tuning import apply_wal_size_limit

logger = logging.getLogger(__name__)


class ConnectionPool:
    """LRU connection pool for SQLite graph databases.

    Caches open connections keyed by database path, evicting the least
    recently used connection when the pool is full.
    """

    def __init__(self, max_size: int = 10) -> None:
        self._max_size = max_size
        self._pool: OrderedDict[str, sqlite3.Connection] = OrderedDict()
        self._lock = threading.Lock()

    def get(self, db_path: str) -> sqlite3.Connection:
        """Get or create a connection for the given database path.

        Args:
            db_path: Path to the SQLite database file.

        Returns:
            An open SQLite connection.
        """
        key = str(Path(db_path).resolve())
        with self._lock:
            if key in self._pool:
                self._pool.move_to_end(key)
                return self._pool[key]

            while len(self._pool) >= self._max_size:
                evict_key, evict_conn = self._pool.popitem(last=False)
                try:
                    evict_conn.close()
                except sqlite3.Error:
                    logger.debug("Failed to close evicted connection: %s", evict_key)
                logger.debug("Evicted connection: %s", evict_key)

            conn = sqlite3.connect(
                key,
                timeout=30,
                check_same_thread=False,
                isolation_level=None,
            )
            conn.row_factory = sqlite3.Row
            conn.execute("PRAGMA journal_mode=WAL")
            conn.execute("PRAGMA busy_timeout=5000")
            apply_wal_size_limit(conn)
            self._pool[key] = conn
            return conn

    def close_all(self) -> None:
        """Close all connections in the pool."""
        with self._lock:
            for key, conn in self._pool.items():
                try:
                    conn.close()
                except sqlite3.Error:
                    logger.debug("Failed to close connection: %s", key)
            self._pool.clear()

    @property
    def size(self) -> int:
        """Current number of open connections."""
        with self._lock:
            return len(self._pool)
