"""Tests for native GraphStore write atomicity."""

import tempfile
from pathlib import Path
from typing import cast

import pytest

from dagayn.communities import CommunityRecord, store_communities
from dagayn.flows import store_flows
from dagayn.graph import GraphStore
from dagayn.parser import NodeInfo
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


class TestTransactionRobustness:
    def test_store_file_nodes_edges_persists(self, store):
        node = NodeInfo(
            kind="File",
            name="keep",
            file_path="keep.py",
            line_start=1,
            line_end=10,
            language="python",
        )
        store.store_file_nodes_edges("keep.py", [node], [])
        assert len(store.get_nodes_by_file("keep.py")) == 1

    def test_atomic_community_storage(self, store):
        communities = [{"name": "comm1", "size": 1, "members": ["node1"]}]
        store_communities(store, cast(list[CommunityRecord], communities))
        count = store_conn(store).execute("SELECT count(*) FROM communities").fetchone()[0]
        assert count == 1

    def test_atomic_flow_storage(self, store):
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
        store_flows(store, flows)
        count = store_conn(store).execute("SELECT count(*) FROM flows").fetchone()[0]
        assert count == 1
