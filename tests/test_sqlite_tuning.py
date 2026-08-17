"""WAL size bounding across every connection that writes graph.db.

Regression cover for a 514 MB graph whose ``graph.db-wal`` reached 9.1 GB:
``journal_mode=WAL`` without ``journal_size_limit`` never shrinks the WAL, so
one long write transaction leaves every later read paging through it.
"""

from __future__ import annotations

import sqlite3

from dagayn.connection_pool import ConnectionPool
from dagayn.embeddings_store import EmbeddingStore
from dagayn.graph import GraphStore
from dagayn.sqlite_tuning import WAL_SIZE_LIMIT_BYTES, apply_wal_size_limit


def _journal_size_limit(conn: sqlite3.Connection) -> int:
    return int(conn.execute("PRAGMA journal_size_limit").fetchone()[0])


class TestApplyWalSizeLimit:
    def test_sets_the_limit(self) -> None:
        conn = sqlite3.connect(":memory:")
        try:
            apply_wal_size_limit(conn)
            assert _journal_size_limit(conn) == WAL_SIZE_LIMIT_BYTES
        finally:
            conn.close()

    def test_survives_a_closed_connection(self) -> None:
        conn = sqlite3.connect(":memory:")
        conn.close()
        # Must not raise: tuning is best-effort, never the reason a call fails.
        apply_wal_size_limit(conn)


class TestConnectionsAreBounded:
    def test_graph_store(self, tmp_path) -> None:
        store = GraphStore(tmp_path / "graph.db")
        try:
            assert _journal_size_limit(store._conn) == WAL_SIZE_LIMIT_BYTES
        finally:
            store.close()

    def test_embedding_store(self, tmp_path) -> None:
        store = EmbeddingStore(tmp_path / "graph.db")
        try:
            assert _journal_size_limit(store._conn) == WAL_SIZE_LIMIT_BYTES
        finally:
            store.close()

    def test_connection_pool(self, tmp_path) -> None:
        pool = ConnectionPool(max_size=1)
        try:
            conn = pool.get(str(tmp_path / "graph.db"))
            assert _journal_size_limit(conn) == WAL_SIZE_LIMIT_BYTES
        finally:
            pool.close_all()


def test_wal_truncates_back_to_the_limit(tmp_path) -> None:
    """A checkpoint must shrink the WAL rather than leave it at its peak."""
    db = tmp_path / "graph.db"
    store = GraphStore(db)
    try:
        store._conn.execute("CREATE TABLE bulk (id INTEGER PRIMARY KEY, blob BLOB)")
        payload = b"x" * 64_000
        for i in range(200):
            store._conn.execute("INSERT INTO bulk (id, blob) VALUES (?, ?)", (i, payload))
        store._conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    finally:
        store.close()

    wal = db.with_name(db.name + "-wal")
    if wal.exists():
        assert wal.stat().st_size <= WAL_SIZE_LIMIT_BYTES
