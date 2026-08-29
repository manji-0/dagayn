"""Tests for FTS5 content sync robustness."""

import tempfile
from pathlib import Path

import pytest

from dagayn.graph import GraphStore
from dagayn.parser import NodeInfo
from dagayn.search import hybrid_search, rebuild_fts_index
from tests.store_sql import store_conn


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

        fts_rows = (
            store_conn(store)
            .execute("SELECT name FROM nodes_fts WHERE name MATCH 'calculate*'")
            .fetchall()
        )
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

        fts_rows = store_conn(store).execute("SELECT name FROM nodes_fts").fetchall()
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

        node_rows = (
            store_conn(store)
            .execute("SELECT id, file_path, name FROM nodes ORDER BY id")
            .fetchall()
        )
        fts_rows = (
            store_conn(store)
            .execute("SELECT rowid, file_path, name FROM nodes_fts ORDER BY rowid")
            .fetchall()
        )

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
        if not callable(rust_sync):
            pytest.skip("GraphStore.sync_fts_for_file_paths is required")
        rust_sync(["src/a.py"])

        names = {row["name"] for row in store_conn(store).execute("SELECT name FROM nodes_fts")}
        assert names >= {"alpha_widget", "beta_gadget"}
