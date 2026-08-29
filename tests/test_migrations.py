"""Tests for the schema migration framework."""

import sqlite3
import tempfile
from pathlib import Path

from dagayn.graph import GraphStore
from dagayn.migrations import LATEST_VERSION
from tests.store_sql import store_conn


def _schema_version(store) -> int:
    row = store_conn(store).execute(
        "SELECT value FROM metadata WHERE key = 'schema_version'"
    ).fetchone()
    return int(row[0]) if row else 0


class TestMigrations:
    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def test_fresh_db_gets_latest_version(self):
        """A newly created DB should be at the latest schema version."""
        version = _schema_version(self.store)
        assert version == LATEST_VERSION

    def test_v1_db_migrates_to_latest(self):
        """A v1 database should migrate to latest when GraphStore is opened."""
        # Close the store that was already migrated
        self.store.close()

        # Manually create a v1 database (base schema only, version=1)
        conn = sqlite3.connect(self.tmp.name)
        conn.execute("INSERT OR REPLACE INTO metadata (key, value) VALUES ('schema_version', '1')")
        conn.commit()
        # Drop migration artifacts to simulate v1
        conn.execute("DROP TABLE IF EXISTS flows")
        conn.execute("DROP TABLE IF EXISTS flow_memberships")
        conn.execute("DROP TABLE IF EXISTS communities")
        conn.execute("DROP TABLE IF EXISTS nodes_fts")
        conn.execute("DROP TABLE IF EXISTS community_summaries")
        conn.execute("DROP TABLE IF EXISTS flow_snapshots")
        conn.execute("DROP TABLE IF EXISTS risk_index")
        conn.commit()
        conn.close()

        # Re-open with GraphStore — should trigger migrations
        self.store = GraphStore(self.tmp.name)
        assert _schema_version(self.store) == LATEST_VERSION

    def test_migration_is_idempotent(self):
        """Opening GraphStore twice should leave schema at latest version."""
        self.store.close()
        self.store = GraphStore(self.tmp.name)
        assert _schema_version(self.store) == LATEST_VERSION

        self.store.close()
        self.store = GraphStore(self.tmp.name)
        assert _schema_version(self.store) == LATEST_VERSION

    def test_signature_column_exists_after_migration(self):
        """The nodes table should have a 'signature' column after migration."""
        cursor = store_conn(self.store).execute("PRAGMA table_info(nodes)")
        columns = [row[1] if isinstance(row, tuple) else row["name"] for row in cursor]
        assert "signature" in columns

    def test_edge_target_name_column_exists_after_migration(self):
        """The edges table should have a normalized target_name column."""
        cursor = store_conn(self.store).execute("PRAGMA table_info(edges)")
        columns = [row[1] if isinstance(row, tuple) else row["name"] for row in cursor]
        assert "target_name" in columns

    def test_flow_kind_and_truncation_columns_exist_after_migration(self):
        """The flows table records reachable-set kind and truncation disclosure."""
        cursor = store_conn(self.store).execute("PRAGMA table_info(flows)")
        columns = [row[1] if isinstance(row, tuple) else row["name"] for row in cursor]
        assert "kind" in columns
        assert "truncated" in columns
        assert "truncation_reason" in columns

    def test_open_backfills_empty_target_name_at_schema_v14(self, tmp_path):
        """Opening a v14 DB must heal empty target_name rows."""
        db_path = tmp_path / "graph.db"
        conn = sqlite3.connect(db_path)
        try:
            conn.executescript(
                """
                CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                INSERT INTO metadata (key, value) VALUES ('schema_version', '14');
                CREATE TABLE nodes (
                    id INTEGER PRIMARY KEY,
                    kind TEXT NOT NULL,
                    name TEXT NOT NULL,
                    qualified_name TEXT NOT NULL UNIQUE,
                    file_path TEXT NOT NULL,
                    line_start INTEGER,
                    line_end INTEGER,
                    language TEXT,
                    params TEXT,
                    return_type TEXT,
                    signature TEXT,
                    extra TEXT DEFAULT '{}',
                    file_hash TEXT,
                    community_id INTEGER,
                    hub_score REAL DEFAULT 0.0,
                    bridge_score REAL DEFAULT 0.0
                );
                CREATE TABLE edges (
                    id INTEGER PRIMARY KEY,
                    kind TEXT NOT NULL,
                    source_qualified TEXT NOT NULL,
                    target_qualified TEXT NOT NULL,
                    target_name TEXT NOT NULL DEFAULT '',
                    file_path TEXT NOT NULL,
                    line INTEGER,
                    extra TEXT DEFAULT '{}',
                    confidence REAL DEFAULT 1.0,
                    confidence_tier TEXT DEFAULT 'EXTRACTED',
                    updated_at REAL
                );
                INSERT INTO nodes (id, kind, name, qualified_name, file_path)
                VALUES (1, 'Function', 'helper', 'main.py::helper', 'main.py');
                INSERT INTO edges (
                    kind, source_qualified, target_qualified, target_name, file_path
                ) VALUES ('CALLS', 'main.py::main', 'main.py::helper', '', 'main.py');
                """
            )
            conn.commit()
        finally:
            conn.close()

        store = GraphStore(db_path)
        try:
            row = store_conn(store).execute(
                "SELECT target_name FROM edges WHERE kind = 'CALLS'"
            ).fetchone()
            assert row["target_name"] == "helper"
            edges = store.search_edges_by_target_name("helper", kind="CALLS")
            assert len(edges) == 1
        finally:
            store.close()

    def test_flows_table_exists_after_migration(self):
        """The flows and flow_memberships tables should exist after migration."""
        tables = _get_table_names(store_conn(self.store))
        assert "flows" in tables
        assert "flow_memberships" in tables

    def test_communities_table_exists_after_migration(self):
        """The communities table should exist and nodes should have community_id."""
        tables = _get_table_names(store_conn(self.store))
        assert "communities" in tables

        cursor = store_conn(self.store).execute("PRAGMA table_info(nodes)")
        columns = [row[1] if isinstance(row, tuple) else row["name"] for row in cursor]
        assert "community_id" in columns

    def test_fts5_table_exists_after_migration(self):
        """The nodes_fts FTS5 virtual table should exist after migration."""
        tables = _get_table_names(store_conn(self.store))
        assert "nodes_fts" in tables

    def test_v6_summary_tables_exist(self):
        """v6 summary tables should exist after migration."""
        tables = _get_table_names(store_conn(self.store))
        assert "community_summaries" in tables
        assert "flow_snapshots" in tables
        assert "risk_index" in tables

    def test_v7_compound_edge_indexes_exist(self):
        """v7 compound edge indexes should exist after migration."""
        rows = store_conn(self.store).execute("PRAGMA index_list(edges)").fetchall()
        indexes = {row[1] if isinstance(row, tuple) else row["name"] for row in rows}

        assert "idx_edges_target_kind" in indexes
        assert "idx_edges_source_kind" in indexes


def _get_table_names(conn: sqlite3.Connection) -> set[str]:
    """Helper: return all table/view names in the database."""
    rows = conn.execute("SELECT name FROM sqlite_master WHERE type IN ('table', 'view')").fetchall()
    return {row[0] if isinstance(row, (tuple, list)) else row["name"] for row in rows}
