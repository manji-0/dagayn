"""Tests for SQLite transaction robustness and nesting scenarios."""

import tempfile
from pathlib import Path
from typing import cast
from unittest.mock import patch

import pytest

from dagayn.communities import CommunityRecord, store_communities
from dagayn.flows import store_flows
from dagayn.graph import GraphStore
from dagayn.parser import NodeInfo


@pytest.fixture
def store():
    """Create a temporary GraphStore for testing."""
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
        db_path = tmp.name
    store = GraphStore(db_path)
    yield store
    store.close()
    Path(db_path).unlink(missing_ok=True)


class TestTransactionRobustness:
    def test_store_file_joins_an_open_transaction(self, store):
        """An already-open transaction is joined, not discarded.

        This used to roll back "whatever transaction is open" before starting
        its own. The connection is shared across threads
        (``check_same_thread=False``), so that recovery destroyed another
        thread's in-flight work -- watch mode's delete handler and its debounced
        flush hit exactly this. The write lock now guarantees any open
        transaction belongs to this thread, so the correct move is to join it.
        """
        store._conn.execute("BEGIN")
        store._conn.execute("INSERT INTO metadata (key, value) VALUES (?, ?)", ("test", "val"))
        assert store._conn.in_transaction

        store.store_file_nodes_edges("test.py", [], [])

        # The write completed and the caller's work was preserved, not dropped.
        assert store.get_metadata("test") == "val"

    def test_atomic_community_storage(self, store):
        """Test that store_communities is atomic and handles existing transactions."""
        communities = [{"name": "comm1", "size": 1, "members": ["node1"]}]

        # Leave a transaction open: it is joined, not discarded (see
        # test_store_file_joins_an_open_transaction).
        store._conn.execute("BEGIN")
        store._conn.execute("INSERT INTO metadata (key, value) VALUES ('leak', 'stale')")

        store_communities(store, cast(list[CommunityRecord], communities))

        assert store.get_metadata("leak") == "stale"

        # Verify communities table
        count = store._conn.execute("SELECT count(*) FROM communities").fetchone()[0]
        assert count == 1

    def test_atomic_flow_storage(self, store):
        """Test that store_flows is atomic and handles existing transactions."""
        flows = [
            {
                "name": "flow1",
                "entry_point_id": 1,
                "depth": 1,
                "node_count": 1,
                "file_count": 1,
                "criticality": 0.5,
                "path": [1],
            }
        ]

        # Leave a transaction open: it is joined, not discarded.
        store._conn.execute("BEGIN")
        store._conn.execute("INSERT INTO metadata (key, value) VALUES ('leak', 'stale')")

        store_flows(store, flows)

        assert store.get_metadata("leak") == "stale"
        count = store._conn.execute("SELECT count(*) FROM flows").fetchone()[0]
        assert count == 1

    def test_rollback_on_failure_in_batch_ops(self, store):
        """Verify that store_file_nodes_edges rolls back if an operation fails inside."""
        # Pre-seed some data
        node_keep = NodeInfo(
            kind="File",
            name="keep",
            file_path="keep.py",
            line_start=1,
            line_end=10,
            language="python",
        )
        store.store_file_nodes_edges("keep.py", [node_keep], [])

        # Attempt to store new file but force a failure
        node_fail = NodeInfo(
            kind="File",
            name="fail",
            file_path="fail.py",
            line_start=1,
            line_end=10,
            language="python",
        )

        with patch.object(store, "_bulk_insert_nodes", side_effect=Exception("Simulated failure")):
            with pytest.raises(Exception, match="Simulated failure"):
                store.store_file_nodes_edges("fail.py", [node_fail], [])

        # Verify 'fail.py' data is NOT present
        assert len(store.get_nodes_by_file("fail.py")) == 0
        # Verify 'keep.py' data IS still present
        assert len(store.get_nodes_by_file("keep.py")) == 1

    def test_public_rollback_api(self, store):
        """Verify the new GraphStore.rollback() public method works."""
        store._conn.execute("BEGIN")
        store._conn.execute("INSERT INTO metadata (key, value) VALUES ('rollback', 'me')")
        assert store._conn.in_transaction

        store.rollback()
        assert not store._conn.in_transaction
        assert store.get_metadata("rollback") is None
