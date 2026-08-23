"""Tests for FTS5 content sync robustness."""

import sqlite3
import tempfile
from pathlib import Path

import pytest

from dagayn.graph import GraphStore
from dagayn.graph._fts_content import build_node_fts_values
from dagayn.parser import NodeInfo
from dagayn.search import hybrid_search, rebuild_fts_index


@pytest.fixture
def store():
    """Create a temporary GraphStore for testing."""
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
        db_path = tmp.name
    store = GraphStore(db_path)
    yield store
    store.close()
    Path(db_path).unlink(missing_ok=True)


class TestFTSSync:
    def test_fts_rebuild_syncs_with_nodes(self, store):
        """Test that rebuild_fts_index properly populates from nodes table."""
        node1 = NodeInfo(
            kind="Function",
            name="calculate_total",
            file_path="app.py",
            line_start=1,
            line_end=5,
            language="python",
        )
        node2 = NodeInfo(
            kind="Class",
            name="OrderProcessor",
            file_path="app.py",
            line_start=10,
            line_end=50,
            language="python",
        )
        store.store_file_nodes_edges("app.py", [node1, node2], [])

        count = rebuild_fts_index(store)
        assert count == 2

        fts_rows = store._conn.execute(
            "SELECT name FROM nodes_fts WHERE name MATCH 'calculate*'"
        ).fetchall()
        assert len(fts_rows) == 1
        assert fts_rows[0]["name"] == "calculate_total"

    def test_fts_rebuild_clears_old_data(self, store):
        """Test that rebuild_fts_index clears existing FTS data before repopulating."""
        node1 = NodeInfo(
            kind="Function",
            name="old_func",
            file_path="old.py",
            line_start=1,
            line_end=5,
            language="python",
        )
        store.store_file_nodes_edges("old.py", [node1], [])
        rebuild_fts_index(store)

        store.remove_file_data("old.py")
        store.commit()

        node2 = NodeInfo(
            kind="Function",
            name="new_func",
            file_path="new.py",
            line_start=1,
            line_end=5,
            language="python",
        )
        store.store_file_nodes_edges("new.py", [node2], [])

        rebuild_fts_index(store)

        fts_rows = store._conn.execute("SELECT name FROM nodes_fts").fetchall()
        assert len(fts_rows) == 1
        assert fts_rows[0]["name"] == "new_func"

    def test_incremental_writes_keep_fts_in_sync(self, store):
        """Issue #41: node writes must maintain FTS without post-processing."""
        alpha = NodeInfo(
            kind="Function",
            name="alpha_widget",
            file_path="src/a.py",
            line_start=1,
            line_end=2,
            language="python",
        )
        beta = NodeInfo(
            kind="Function",
            name="beta_gadget",
            file_path="src/b.py",
            line_start=1,
            line_end=2,
            language="python",
        )
        store.store_file_nodes_edges("src/a.py", [alpha], [])
        store.store_file_nodes_edges("src/b.py", [beta], [])

        store.remove_file_data("src/a.py")
        store.commit()
        gamma = NodeInfo(
            kind="Function",
            name="gamma_thing",
            file_path="src/c.py",
            line_start=1,
            line_end=2,
            language="python",
        )
        store.store_file_nodes_edges("src/c.py", [gamma], [])

        node_rows = store._conn.execute(
            "SELECT id, file_path, name FROM nodes ORDER BY id"
        ).fetchall()
        fts_rows = store._conn.execute(
            "SELECT rowid, file_path, name FROM nodes_fts ORDER BY rowid"
        ).fetchall()

        assert [(row["id"], row["file_path"]) for row in node_rows] == [
            (row["rowid"], row["file_path"]) for row in fts_rows
        ]
        assert {row["name"] for row in node_rows} == {row["name"] for row in fts_rows}

        hs = hybrid_search(store, "src/c.py")
        assert any(result["file_path"] == "src/c.py" for result in hs["results"])
        assert hs["fts_health"]["status"] == "synced"

        hs_alpha = hybrid_search(store, "alpha_widget")
        assert hs_alpha["results"] == []
        assert hs_alpha["mode"] in {"empty", "keyword_fallback"}

    def test_sync_fts_for_changed_files_keeps_other_rows(self, store):
        """Incremental FTS must not DROP/rebuild the whole nodes_fts table."""
        alpha = NodeInfo(
            kind="Function",
            name="alpha_widget",
            file_path="src/a.py",
            line_start=1,
            line_end=2,
            language="python",
        )
        beta = NodeInfo(
            kind="Function",
            name="beta_gadget",
            file_path="src/b.py",
            line_start=1,
            line_end=2,
            language="python",
        )
        store.store_file_nodes_edges("src/a.py", [alpha], [])
        store.store_file_nodes_edges("src/b.py", [beta], [])
        rebuild_fts_index(store)

        rust_sync = getattr(store, "sync_fts_for_file_paths", None)
        if callable(rust_sync):
            rust_sync(["src/a.py"])
        else:
            from dagayn.graph._fts_sync import sync_fts_for_file_paths

            sync_fts_for_file_paths(store._conn, ["src/a.py"], None)

        names = {row["name"] for row in store._conn.execute("SELECT name FROM nodes_fts")}
        assert names >= {"alpha_widget", "beta_gadget"}

    def test_v13_migration_rebuilds_empty_generated_columns(self, tmp_path):
        """Upgrading through v13 must not leave empty identifier_tokens/doc_text."""
        db_path = tmp_path / "legacy.db"
        conn = sqlite3.connect(db_path)
        conn.row_factory = sqlite3.Row
        conn.executescript(
            """
            CREATE TABLE nodes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                qualified_name TEXT NOT NULL UNIQUE,
                file_path TEXT NOT NULL,
                line_start INTEGER,
                line_end INTEGER,
                language TEXT,
                parent_name TEXT,
                params TEXT,
                return_type TEXT,
                modifiers TEXT,
                is_test INTEGER DEFAULT 0,
                file_hash TEXT,
                mtime_ns INTEGER DEFAULT 0,
                extra TEXT DEFAULT '{}',
                signature TEXT,
                updated_at REAL NOT NULL
            );
            CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE VIRTUAL TABLE nodes_fts USING fts5(
                name, qualified_name, file_path, signature, identifier_tokens, doc_text,
                tokenize='porter unicode61'
            );
            INSERT INTO metadata VALUES ('schema_version', '13');
            INSERT INTO nodes (
                kind, name, qualified_name, file_path, line_start, line_end,
                language, extra, signature, updated_at
            ) VALUES (
                'Function', 'user_info', 'docs.py::user_info', 'docs.py', 1, 3,
                'python', '{"display_name":"ユーザー情報"}', 'def user_info()', 1.0
            );
            INSERT INTO nodes_fts(rowid, name, qualified_name, file_path, signature,
                                  identifier_tokens, doc_text)
            SELECT rowid, name, qualified_name, file_path, COALESCE(signature, ''), '', ''
            FROM nodes;
            """
        )
        conn.commit()
        conn.close()

        store = GraphStore(db_path)
        try:
            doc_text = store._conn.execute(
                "SELECT doc_text FROM nodes_fts WHERE name = 'user_info'"
            ).fetchone()["doc_text"]
            identifier_tokens = store._conn.execute(
                "SELECT identifier_tokens FROM nodes_fts WHERE name = 'user_info'"
            ).fetchone()["identifier_tokens"]
            assert doc_text
            assert identifier_tokens
            assert "ユーザー" in doc_text or "user" in identifier_tokens
        finally:
            store.close()

    def test_doc_text_builder_matches_rust_shape(self):
        """Cross-backend golden test for structured doc_text content."""
        values = build_node_fts_values(
            kind="Function",
            name="handle_failure",
            qualified_name="service.py::handle_failure",
            file_path="service.py",
            line_start=1,
            line_end=3,
            signature="def handle_failure(retry_budget) -> bool",
            extra={"display_name": "Retry failure handler"},
            repo_root=None,
        )
        _name, _qualified, _path, _signature, identifier_tokens, doc_text = values
        assert "kind: Function" in doc_text
        assert "qualified: service.py::handle_failure" in doc_text
        assert "signature: def handle_failure" in doc_text
        assert "Retry failure handler" in doc_text
        assert "handle" in identifier_tokens
        assert "failure" in identifier_tokens


def test_rust_doc_text_builder_matches_python_shape(tmp_path):
    pytest.importorskip("dagayn._core")
    from dagayn._core import GraphStore as RustGraphStore
    from dagayn.parser import NodeInfo

    db_path = tmp_path / "rust-fts-golden.db"
    store = RustGraphStore(db_path)
    node = NodeInfo(
        kind="Function",
        name="handle_failure",
        file_path="service.py",
        line_start=1,
        line_end=3,
        language="python",
        params="(retry_budget)",
        return_type="bool",
        extra={"display_name": "Retry failure handler"},
    )
    store.store_file_nodes_edges("service.py", [node], [], "hash", 0)
    conn = sqlite3.connect(db_path)
    conn.execute(
        "UPDATE nodes SET signature = ? WHERE qualified_name = ?",
        (
            "def handle_failure(retry_budget) -> bool",
            "service.py::handle_failure",
        ),
    )
    conn.commit()
    store.rebuild_fts_index()
    doc_text = conn.execute(
        "SELECT doc_text FROM nodes_fts WHERE name = 'handle_failure'"
    ).fetchone()[0]
    conn.close()
    store.close()

    python_values = build_node_fts_values(
        kind="Function",
        name="handle_failure",
        qualified_name="service.py::handle_failure",
        file_path="service.py",
        line_start=1,
        line_end=3,
        signature="def handle_failure(retry_budget) -> bool",
        extra={"display_name": "Retry failure handler"},
        repo_root=None,
    )
    assert python_values[5] == doc_text
