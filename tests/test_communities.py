"""Tests for community/cluster detection."""

import tempfile
from pathlib import Path

from dagayn.communities import (
    count_affected_communities,
    detect_communities,
    get_architecture_overview,
    get_communities,
    incremental_detect_communities,
    store_communities,
)
from dagayn.graph import GraphStore
from dagayn.parser import EdgeInfo, NodeInfo
from tests.store_sql import store_conn


class TestCommunities:
    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def _seed_two_clusters(self):
        """Seed two distinct clusters: auth (auth.py) and db (db.py)."""
        # Auth cluster
        self.store.upsert_node(
            NodeInfo(
                kind="File",
                name="auth.py",
                file_path="auth.py",
                line_start=1,
                line_end=100,
                language="python",
            ),
            file_hash="a1",
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="login",
                file_path="auth.py",
                line_start=5,
                line_end=20,
                language="python",
            ),
            file_hash="a1",
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="logout",
                file_path="auth.py",
                line_start=25,
                line_end=40,
                language="python",
            ),
            file_hash="a1",
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="check_token",
                file_path="auth.py",
                line_start=45,
                line_end=60,
                language="python",
            ),
            file_hash="a1",
        )
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source="auth.py::login",
                target="auth.py::check_token",
                file_path="auth.py",
                line=10,
            )
        )
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source="auth.py::logout",
                target="auth.py::check_token",
                file_path="auth.py",
                line=30,
            )
        )

        # DB cluster
        self.store.upsert_node(
            NodeInfo(
                kind="File",
                name="db.py",
                file_path="db.py",
                line_start=1,
                line_end=100,
                language="python",
            ),
            file_hash="b1",
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="connect",
                file_path="db.py",
                line_start=5,
                line_end=20,
                language="python",
            ),
            file_hash="b1",
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="query",
                file_path="db.py",
                line_start=25,
                line_end=40,
                language="python",
            ),
            file_hash="b1",
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="close",
                file_path="db.py",
                line_start=45,
                line_end=60,
                language="python",
            ),
            file_hash="b1",
        )
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source="db.py::query",
                target="db.py::connect",
                file_path="db.py",
                line=30,
            )
        )
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source="db.py::close",
                target="db.py::connect",
                file_path="db.py",
                line=50,
            )
        )

        # One cross-cluster edge
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source="auth.py::login",
                target="db.py::query",
                file_path="auth.py",
                line=15,
            )
        )
        self.store.commit()

    def test_detect_communities_returns_list(self):
        """detect_communities returns a list."""
        self._seed_two_clusters()
        result = detect_communities(self.store, min_size=2)
        assert isinstance(result, list)

    def test_detect_finds_clusters(self):
        """With clear clusters and igraph, finds >= 2 communities."""
        self._seed_two_clusters()
        result = detect_communities(self.store, min_size=2)
        assert len(result) >= 2

    def test_community_has_required_fields(self):
        """Each community dict has required fields: name, size, cohesion, members."""
        self._seed_two_clusters()
        result = detect_communities(self.store, min_size=2)
        assert len(result) > 0
        for comm in result:
            assert "name" in comm
            assert "size" in comm
            assert "cohesion" in comm
            assert "members" in comm
            assert isinstance(comm["name"], str)
            assert isinstance(comm["size"], int)
            assert isinstance(comm["cohesion"], (int, float))
            assert isinstance(comm["members"], list)

    def test_store_and_retrieve_communities(self):
        """Communities can be stored and retrieved round-trip."""
        self._seed_two_clusters()
        communities = detect_communities(self.store, min_size=2)
        assert len(communities) > 0

        count = store_communities(self.store, communities)
        assert count == len(communities)

        retrieved = get_communities(self.store)
        assert len(retrieved) == len(communities)
        for comm in retrieved:
            assert "id" in comm
            assert "name" in comm
            assert "size" in comm

    def test_architecture_overview(self):
        """Architecture overview has required keys."""
        self._seed_two_clusters()
        communities = detect_communities(self.store, min_size=2)
        store_communities(self.store, communities)

        overview = get_architecture_overview(self.store)
        assert "communities" in overview
        assert "cross_community_coupling" in overview
        assert "warnings" in overview
        assert isinstance(overview["communities"], list)
        assert isinstance(overview["cross_community_coupling"], list)
        assert isinstance(overview["warnings"], list)

    def test_architecture_overview_standard_omits_members(self):
        """Standard detail_level does not include community member lists."""
        self._seed_two_clusters()
        communities = detect_communities(self.store, min_size=2)
        store_communities(self.store, communities)

        overview = get_architecture_overview(self.store, detail_level="standard")
        for comm in overview["communities"]:
            assert "members" not in comm

    def test_architecture_overview_verbose_includes_members_and_raw_edges(self):
        """Verbose detail_level includes member lists and raw cross_community_edges."""
        self._seed_two_clusters()
        communities = detect_communities(self.store, min_size=2)
        store_communities(self.store, communities)

        overview = get_architecture_overview(self.store, detail_level="verbose")
        assert "cross_community_edges" in overview
        assert isinstance(overview["cross_community_edges"], list)
        for comm in overview["communities"]:
            assert "members" in comm

    def test_architecture_overview_minimal_compact(self):
        """Minimal detail_level returns only name/size/cohesion per community."""
        self._seed_two_clusters()
        communities = detect_communities(self.store, min_size=2)
        store_communities(self.store, communities)

        overview = get_architecture_overview(self.store, detail_level="minimal")
        for comm in overview["communities"]:
            assert set(comm.keys()) == {"name", "size", "assigned_member_count", "cohesion"}
        assert len(overview["cross_community_coupling"]) <= 5

    def test_architecture_overview_coupling_has_edge_kinds(self):
        """cross_community_coupling entries include edge_kinds breakdown."""
        self._seed_two_clusters()
        communities = detect_communities(self.store, min_size=2)
        store_communities(self.store, communities)

        overview = get_architecture_overview(self.store)
        for entry in overview["cross_community_coupling"]:
            assert "edge_count" in entry
            assert "edge_kinds" in entry
            assert isinstance(entry["edge_kinds"], dict)

    def test_architecture_overview_excludes_tested_by_coupling(self):
        """TESTED_BY edges do not count toward coupling warnings."""
        self._seed_two_clusters()
        communities = detect_communities(self.store, min_size=2)
        store_communities(self.store, communities)

        # Add many TESTED_BY cross-community edges (well above the threshold of 10)
        for i in range(20):
            self.store.upsert_edge(
                EdgeInfo(
                    kind="TESTED_BY",
                    source="auth.py::login",
                    target="db.py::query",
                    file_path="auth.py",
                    line=i + 100,
                )
            )
        self.store.commit()

        overview = get_architecture_overview(self.store)
        # Warnings should not include any that are purely from TESTED_BY edges
        for w in overview["warnings"]:
            assert "TESTED_BY" not in w

    def test_architecture_overview_excludes_test_community_warnings(self):
        """Warnings involving test-dominated communities are filtered out."""
        self._seed_two_clusters()
        communities = detect_communities(self.store, min_size=2)
        store_communities(self.store, communities)

        # Manually insert a test-named community with high cross-coupling
        conn = store_conn(self.store)
        cursor = conn.execute(
            "INSERT INTO communities (name, level, cohesion, size, dominant_language, description)"
            " VALUES (?, 0, 0.5, 10, 'typescript', 'Test community')",
            ("handler-it:should",),
        )
        test_comm_id = cursor.lastrowid
        # Assign some nodes to this community (reuse existing node)
        conn.execute(
            "UPDATE nodes SET community_id = ? WHERE name = 'login'",
            (test_comm_id,),
        )
        conn.commit()

        overview = get_architecture_overview(self.store)
        for w in overview["warnings"]:
            assert "it:should" not in w, f"Test community should be filtered: {w}"

    def test_get_communities_sort_by(self):
        """get_communities respects sort_by parameter."""
        self._seed_two_clusters()
        communities = detect_communities(self.store, min_size=2)
        store_communities(self.store, communities)

        by_size = get_communities(self.store, sort_by="size")
        assert len(by_size) > 0
        # Sizes should be in descending order
        sizes = [c["size"] for c in by_size]
        assert sizes == sorted(sizes, reverse=True)

        by_name = get_communities(self.store, sort_by="name")
        names = [c["name"] for c in by_name]
        assert names == sorted(names)

    def test_get_communities_min_size_filter(self):
        """get_communities with min_size filters small communities."""
        self._seed_two_clusters()
        communities = detect_communities(self.store, min_size=1)
        store_communities(self.store, communities)

        # With very high min_size, should get empty
        result = get_communities(self.store, min_size=999)
        assert len(result) == 0

    def test_store_communities_clears_previous(self):
        """Storing communities clears previous community data."""
        self._seed_two_clusters()
        communities = detect_communities(self.store, min_size=2)
        store_communities(self.store, communities)

        first_count = len(get_communities(self.store))
        assert first_count > 0

        # Store again with empty list
        store_communities(self.store, [])
        assert len(get_communities(self.store)) == 0

    def test_detect_communities_empty_graph(self):
        """Detect on empty graph returns empty list."""
        result = detect_communities(self.store, min_size=2)
        assert result == []

    def test_leiden_fallback_to_file_based(self):
        """When Leiden produces 0 communities (all < min_size), fall back to file-based."""
        # Seed nodes with only CONTAINS edges (no CALLS/IMPORTS -- sparse graph)
        self.store.upsert_node(
            NodeInfo(
                kind="File",
                name="a.py",
                file_path="a.py",
                line_start=1,
                line_end=100,
                language="python",
            ),
            file_hash="a1",
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="f1",
                file_path="a.py",
                line_start=1,
                line_end=10,
                language="python",
                parent_name=None,
            ),
            file_hash="a1",
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="f2",
                file_path="a.py",
                line_start=11,
                line_end=20,
                language="python",
                parent_name=None,
            ),
            file_hash="a1",
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="f3",
                file_path="a.py",
                line_start=21,
                line_end=30,
                language="python",
                parent_name=None,
            ),
            file_hash="a1",
        )
        self.store.upsert_edge(
            EdgeInfo(kind="CONTAINS", source="a.py", target="a.py::f1", file_path="a.py", line=1)
        )
        self.store.upsert_edge(
            EdgeInfo(kind="CONTAINS", source="a.py", target="a.py::f2", file_path="a.py", line=11)
        )
        self.store.upsert_edge(
            EdgeInfo(kind="CONTAINS", source="a.py", target="a.py::f3", file_path="a.py", line=21)
        )
        # With high min_size, Leiden may produce tiny clusters that get dropped.
        # The fallback to file-based should still produce results.
        result = detect_communities(self.store, min_size=2)
        assert isinstance(result, list)
        assert len(result) >= 1

    def test_incremental_detect_no_affected_communities(self):
        """incremental_detect_communities returns 0 when no communities are affected."""
        self._seed_two_clusters()
        communities = detect_communities(self.store, min_size=2)
        store_communities(self.store, communities)

        # Pass a file that has no nodes in any community
        result = incremental_detect_communities(self.store, ["nonexistent.py"])
        assert result == 0

    def test_incremental_detect_redetects_affected(self):
        """incremental_detect_communities re-detects when communities ARE affected."""
        self._seed_two_clusters()
        communities = detect_communities(self.store, min_size=2)
        stored = store_communities(self.store, communities)
        assert stored > 0

        # Pass a file that IS part of existing communities
        result = incremental_detect_communities(self.store, ["auth.py"])
        assert result > 0

    def test_count_affected_communities_uses_pre_parse_snapshot(self):
        """Pre-parse affected count still triggers detection after assignments clear."""
        self._seed_two_clusters()
        communities = detect_communities(self.store, min_size=2)
        store_communities(self.store, communities)

        pre_affected = count_affected_communities(self.store, ["auth.py"])
        assert pre_affected > 0

        store_conn(self.store).execute(
            "UPDATE nodes SET community_id = NULL WHERE file_path = ?",
            ("auth.py",),
        )
        self.store.commit()
        assert count_affected_communities(self.store, ["auth.py"]) > 0

        result = incremental_detect_communities(
            self.store,
            ["auth.py"],
            pre_affected_count=pre_affected,
        )
        assert result > 0


class TestDuplicateCommunityNames:
    """Two communities that generate the same name must stay two.

    ``_generate_community_name`` is ``{parent-dir}-{top-keyword}`` and is not
    unique. Ids used to be looked up by name after a batch insert, so the second
    community overwrote the first in the mapping: community #1 got zero members
    and community #2 was assigned everyone. The next build's
    ``refresh_community_stats`` then deleted the member-less one, permanently
    merging two distinct communities.
    """

    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def _node(self, name: str, line: int) -> None:
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name=name,
                file_path="svc/user.py",
                line_start=line,
                line_end=line + 1,
                language="python",
            )
        )
