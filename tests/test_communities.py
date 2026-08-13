"""Tests for community/cluster detection."""

import tempfile
from pathlib import Path

import pytest

from dagayn.communities import (
    IGRAPH_AVAILABLE,
    _compute_cohesion,
    _compute_cohesion_batch,
    _detect_file_based,
    _generate_community_name,
    count_affected_communities,
    detect_communities,
    get_architecture_overview,
    get_communities,
    incremental_detect_communities,
    refresh_community_stats,
    store_communities,
)
from dagayn.graph import GraphEdge, GraphNode, GraphStore
from dagayn.parser import EdgeInfo, NodeInfo


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

    @pytest.mark.skipif(not IGRAPH_AVAILABLE, reason="igraph not installed")
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
        conn = self.store._conn
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

    def test_fallback_file_communities(self):
        """File-based fallback produces communities grouped by file."""
        self._seed_two_clusters()
        # Gather nodes and edges for file-based detection
        all_edges = self.store.get_all_edges()
        nodes = []
        for fp in self.store.get_all_files():
            nodes.extend(self.store.get_nodes_by_file(fp))

        result = _detect_file_based(nodes, all_edges, min_size=2)
        assert isinstance(result, list)
        assert len(result) >= 2
        for comm in result:
            assert "name" in comm
            assert "size" in comm
            assert comm["size"] >= 2

    def test_community_naming(self):
        """Community naming produces non-empty names."""
        self._seed_two_clusters()
        result = detect_communities(self.store, min_size=2)
        for comm in result:
            assert comm["name"]
            assert len(comm["name"]) > 0

    def test_community_naming_with_dominant_class(self):
        """When a class dominates (>40%), it appears in the name."""
        nodes = [
            GraphNode(
                id=1,
                kind="Class",
                name="AuthService",
                qualified_name="auth.py::AuthService",
                file_path="auth.py",
                line_start=1,
                line_end=100,
                language="python",
                parent_name=None,
                params=None,
                return_type=None,
                is_test=False,
                file_hash="x",
                extra={},
            ),
            GraphNode(
                id=2,
                kind="Function",
                name="login",
                qualified_name="auth.py::AuthService.login",
                file_path="auth.py",
                line_start=10,
                line_end=20,
                language="python",
                parent_name="AuthService",
                params=None,
                return_type=None,
                is_test=False,
                file_hash="x",
                extra={},
            ),
        ]
        name = _generate_community_name(nodes)
        assert name  # non-empty
        assert "authservice" in name.lower() or "auth" in name.lower()

    def test_community_naming_empty(self):
        """Empty member list produces 'empty' name."""
        name = _generate_community_name([])
        assert name == "empty"

    def test_cohesion_computation(self):
        """Cohesion is correctly computed as internal/(internal+external)."""
        member_qns = {"f.py::a", "f.py::b"}
        edges = [
            GraphEdge(
                id=1,
                kind="CALLS",
                source_qualified="f.py::a",
                target_qualified="f.py::b",
                file_path="f.py",
                line=1,
                extra={},
            ),
            GraphEdge(
                id=2,
                kind="CALLS",
                source_qualified="f.py::a",
                target_qualified="g.py::c",  # resolved cross-file target
                file_path="f.py",
                line=2,
                extra={},
            ),
        ]
        cohesion = _compute_cohesion(member_qns, edges)
        # 1 internal (a->b), 1 external (a->g.py::c) => 0.5
        assert cohesion == pytest.approx(0.5)

    def test_cohesion_all_internal(self):
        """All edges internal => cohesion = 1.0."""
        member_qns = {"f.py::a", "f.py::b"}
        edges = [
            GraphEdge(
                id=1,
                kind="CALLS",
                source_qualified="f.py::a",
                target_qualified="f.py::b",
                file_path="f.py",
                line=1,
                extra={},
            ),
        ]
        cohesion = _compute_cohesion(member_qns, edges)
        assert cohesion == pytest.approx(1.0)

    def test_cohesion_no_edges(self):
        """No edges => cohesion = 0.0."""
        member_qns = {"a", "b"}
        cohesion = _compute_cohesion(member_qns, [])
        assert cohesion == pytest.approx(0.0)

    def test_compute_cohesion_batch_matches_single(self):
        """Batch cohesion must produce identical results to calling
        _compute_cohesion once per community. Regression guard for the
        O(files * edges) -> O(edges) refactor.
        """
        edges = [
            # Internal to comm_a
            GraphEdge(
                id=1,
                kind="CALLS",
                source_qualified="a::f1",
                target_qualified="a::f2",
                file_path="a.py",
                line=1,
                extra={},
            ),
            # Cross-community (a <-> b): external to both
            GraphEdge(
                id=2,
                kind="CALLS",
                source_qualified="a::f1",
                target_qualified="b::g1",
                file_path="a.py",
                line=2,
                extra={},
            ),
            # Internal to comm_b
            GraphEdge(
                id=3,
                kind="CALLS",
                source_qualified="b::g1",
                target_qualified="b::g2",
                file_path="b.py",
                line=3,
                extra={},
            ),
            # Half-in (b -> c): cross-community resolved CALLS — counts as external.
            GraphEdge(
                id=4,
                kind="CALLS",
                source_qualified="b::g1",
                target_qualified="c::h1",
                file_path="b.py",
                line=4,
                extra={},
            ),
            # Neither endpoint in any tracked community — fully ignored
            GraphEdge(
                id=5,
                kind="CALLS",
                source_qualified="c::h1",
                target_qualified="d::k1",
                file_path="c.py",
                line=5,
                extra={},
            ),
        ]
        comm_a = {"a::f1", "a::f2"}
        comm_b = {"b::g1", "b::g2"}

        batch = _compute_cohesion_batch([comm_a, comm_b], edges)
        expected = [
            _compute_cohesion(comm_a, edges),
            _compute_cohesion(comm_b, edges),
        ]
        assert batch == expected
        # comm_a: 1 internal (edge 1) + 1 external (edge 2) = 0.5
        # comm_b: 1 internal (edge 3) + 2 external (edges 2, 4) = 1/3
        #   edge 4 has "c::h1" (contains "::") so it is a resolved cross-
        #   community call and correctly counts as external.
        assert batch[0] == pytest.approx(0.5)
        assert batch[1] == pytest.approx(1 / 3)

    def test_calls_to_unresolved_target_not_counted(self):
        """CALLS edges whose target has no community are stdlib/builtin noise
        and must not inflate the external denominator (Part 1 policy).
        """
        # Only comm_a members are tracked; "stdlib::append" has no community.
        comm_a = {"a::f1", "a::f2"}
        edges = [
            GraphEdge(
                id=1,
                kind="CALLS",
                source_qualified="a::f1",
                target_qualified="a::f2",
                file_path="a.py",
                line=1,
                extra={},
            ),
            # Bare stdlib call — should be ignored by cohesion formula.
            GraphEdge(
                id=2,
                kind="CALLS",
                source_qualified="a::f1",
                target_qualified="append",
                file_path="a.py",
                line=2,
                extra={},
            ),
        ]
        cohesion = _compute_cohesion(comm_a, edges)
        # Only edge 1 counts (internal). Edge 2 is filtered (unresolved CALLS).
        assert cohesion == pytest.approx(1.0)

    def test_non_calls_to_unresolved_target_still_counted(self):
        """IMPORTS_FROM / CROSS_ARTIFACT edges to unresolved targets ARE
        counted as external — only CALLS is filtered.
        """
        comm_a = {"a::f1"}
        edges = [
            GraphEdge(
                id=1,
                kind="IMPORTS_FROM",
                source_qualified="a::f1",
                target_qualified="some_external_pkg",
                file_path="a.py",
                line=1,
                extra={},
            ),
        ]
        cohesion = _compute_cohesion(comm_a, edges)
        # 0 internal, 1 external → 0.0
        assert cohesion == pytest.approx(0.0)

    def test_compute_cohesion_batch_empty(self):
        """Batch with empty list returns empty list."""
        assert _compute_cohesion_batch([], []) == []

    def test_compute_cohesion_batch_no_edges(self):
        """Batch with no edges returns 0.0 per community."""
        result = _compute_cohesion_batch([{"a"}, {"b", "c"}], [])
        assert result == [0.0, 0.0]

    def test_detect_file_based_integration(self):
        """End-to-end: _detect_file_based produces correct member sets and
        cohesion values on a hand-built fixture with asymmetric cohesions.

        Guards the batch-cohesion refactor against zip misalignment, wrong
        member_qns passed to the batch helper, and member/cohesion drift.
        Cohesions are deliberately distinct (1.0 vs 0.6667) so a swap would
        fail the assertions.
        """

        def mk_node(nid: int, name: str, fp: str) -> GraphNode:
            return GraphNode(
                id=nid,
                kind="Function",
                name=name,
                qualified_name=f"{fp}::{name}",
                file_path=fp,
                line_start=1,
                line_end=10,
                language="python",
                parent_name=None,
                params=None,
                return_type=None,
                is_test=False,
                file_hash="h",
                extra={},
            )

        def mk_edge(eid: int, src: str, tgt: str, fp: str) -> GraphEdge:
            return GraphEdge(
                id=eid,
                kind="CALLS",
                source_qualified=src,
                target_qualified=tgt,
                file_path=fp,
                line=1,
                extra={},
            )

        nodes = [
            mk_node(1, "login", "auth.py"),
            mk_node(2, "logout", "auth.py"),
            mk_node(3, "check_token", "auth.py"),
            mk_node(4, "connect", "db.py"),
            mk_node(5, "query", "db.py"),
            mk_node(6, "close", "db.py"),
        ]
        edges = [
            # auth.py: 2 internal, 0 external  -> cohesion 1.0
            mk_edge(1, "auth.py::login", "auth.py::check_token", "auth.py"),
            mk_edge(2, "auth.py::logout", "auth.py::check_token", "auth.py"),
            # db.py: 2 internal, 1 external  -> cohesion 2/3 ≈ 0.6667
            mk_edge(3, "db.py::query", "db.py::connect", "db.py"),
            mk_edge(4, "db.py::close", "db.py::connect", "db.py"),
            mk_edge(5, "db.py::close", "external.py::log", "db.py"),
        ]

        result = _detect_file_based(nodes, edges, min_size=2)

        assert len(result) == 2
        by_desc = {c["description"]: c for c in result}
        auth = by_desc["Directory-based community: auth"]
        db = by_desc["Directory-based community: db"]

        # Member sets — catches wrong member_qns being passed to batch helper
        assert set(auth["members"]) == {
            "auth.py::login",
            "auth.py::logout",
            "auth.py::check_token",
        }
        assert set(db["members"]) == {
            "db.py::connect",
            "db.py::query",
            "db.py::close",
        }

        # Cohesions are distinct — zip misalignment would swap these
        assert auth["cohesion"] == pytest.approx(1.0)
        assert db["cohesion"] == pytest.approx(0.6667)

        # Metadata passes through correctly
        assert auth["size"] == 3
        assert db["size"] == 3
        assert auth["dominant_language"] == "python"
        assert db["dominant_language"] == "python"
        assert auth["level"] == 0
        assert db["level"] == 0

    def test_detected_cohesions_match_direct_computation(self):
        """Every stored community cohesion must equal what _compute_cohesion
        produces when called directly on that community's member set and
        the full edge list.

        Algorithm-agnostic: runs against whichever path detect_communities
        takes (Leiden if igraph is available, file-based otherwise). Any
        regression in the batch-cohesion refactor that mis-aligns
        cohesions to communities would fail loudly here with specific
        community names.

        The fixture is deliberately broken out of symmetry (one extra
        internal edge in auth.py) so a swap between auth/db cohesions
        would be visible.
        """
        self._seed_two_clusters()
        # Break cohesion symmetry: add one extra internal edge in auth.py
        # so auth.py cohesion != db.py cohesion. Without this, the seeded
        # fixture has both communities at 2/3 and a zip misalignment
        # would be silent.
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source="auth.py::login",
                target="auth.py::logout",
                file_path="auth.py",
                line=12,
            )
        )
        self.store.commit()

        communities = detect_communities(self.store, min_size=2)
        assert len(communities) > 0

        all_edges = self.store.get_all_edges()
        # Collect the distinct cohesion values we see, to guard against
        # the degenerate case where the fixture somehow produces all-equal
        # cohesions (which would make a swap undetectable).
        seen_cohesions: set[float] = set()
        for comm in communities:
            # Sub-communities (level=1) have cohesion computed against
            # a filtered sub-edge set, so skip them. The fixture is tiny
            # enough that no sub-communities are produced in practice.
            if comm.get("level", 0) != 0:
                continue
            member_qns = set(comm["members"])
            direct = round(_compute_cohesion(member_qns, all_edges), 4)
            assert comm["cohesion"] == direct, (
                f"Community {comm['name']!r} stored cohesion "
                f"{comm['cohesion']} but direct computation gives {direct}"
            )
            seen_cohesions.add(comm["cohesion"])

        # Sanity: the fixture produced communities with distinct cohesions,
        # so the equality check above actually guards against swaps.
        assert len(seen_cohesions) >= 2, (
            "Fixture regression: all detected communities have the same "
            "cohesion, which means a zip misalignment bug would not be "
            f"caught here. seen={seen_cohesions}"
        )

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

    def test_igraph_available_is_bool(self):
        """IGRAPH_AVAILABLE is a boolean."""
        assert isinstance(IGRAPH_AVAILABLE, bool)

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

    def test_incremental_detect_after_reparse_clears_assignments(self):
        """Re-parsed files lose community_id; detection must still re-run."""
        self._seed_two_clusters()
        communities = detect_communities(self.store, min_size=2)
        store_communities(self.store, communities)

        # Simulate a re-parse: nodes keep their QNs but lose community_id.
        self.store._conn.execute(
            "UPDATE nodes SET community_id = NULL WHERE file_path = ?",
            ("auth.py",),
        )
        self.store.commit()

        assert count_affected_communities(self.store, ["auth.py"]) > 0
        result = incremental_detect_communities(self.store, ["auth.py"])
        assert result > 0

        assigned = self.store._conn.execute(
            "SELECT COUNT(*) FROM nodes WHERE file_path = ? AND community_id IS NOT NULL",
            ("auth.py",),
        ).fetchone()[0]
        assert assigned > 0

    def test_count_affected_communities_uses_pre_parse_snapshot(self):
        """Pre-parse affected count still triggers detection after assignments clear."""
        self._seed_two_clusters()
        communities = detect_communities(self.store, min_size=2)
        store_communities(self.store, communities)

        pre_affected = count_affected_communities(self.store, ["auth.py"])
        assert pre_affected > 0

        self.store._conn.execute(
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

    def test_get_communities_reports_assigned_member_count(self):
        """Stored size and live assigned_member_count are both exposed."""
        self._seed_two_clusters()
        communities = detect_communities(self.store, min_size=2)
        store_communities(self.store, communities)

        stored = get_communities(self.store)
        assert stored
        for comm in stored:
            assert "assigned_member_count" in comm
            assert comm["assigned_member_count"] == len(comm["members"])

        # Simulate stale stored size after partial assignment loss.
        comm_id = stored[0]["id"]
        node_id = self.store._conn.execute(
            "SELECT id FROM nodes WHERE community_id = ? LIMIT 1",
            (comm_id,),
        ).fetchone()[0]
        self.store._conn.execute(
            "UPDATE nodes SET community_id = NULL WHERE id = ?",
            (node_id,),
        )
        self.store.commit()

        refreshed = get_communities(self.store)
        stale = next(c for c in refreshed if c["id"] == comm_id)
        assert stale["size"] != stale["assigned_member_count"]

    def test_refresh_community_stats_recomputes_size_and_cohesion(self):
        """Orphan sweep refreshes stored community metrics from live members."""
        self._seed_two_clusters()
        communities = detect_communities(self.store, min_size=2)
        store_communities(self.store, communities)

        comm_id = get_communities(self.store)[0]["id"]
        node_id = self.store._conn.execute(
            "SELECT id FROM nodes WHERE community_id = ? LIMIT 1",
            (comm_id,),
        ).fetchone()[0]
        self.store._conn.execute(
            "UPDATE nodes SET community_id = NULL WHERE id = ?",
            (node_id,),
        )
        self.store.commit()

        stats = refresh_community_stats(self.store)
        assert stats["updated"] >= 1

        row = self.store._conn.execute(
            "SELECT size FROM communities WHERE id = ?",
            (comm_id,),
        ).fetchone()
        live_count = self.store._conn.execute(
            "SELECT COUNT(*) FROM nodes WHERE community_id = ?",
            (comm_id,),
        ).fetchone()[0]
        assert row[0] == live_count

    def test_architecture_overview_warns_on_stale_community_size(self):
        """Architecture overview surfaces stored/live size divergence."""
        self._seed_two_clusters()
        communities = detect_communities(self.store, min_size=2)
        store_communities(self.store, communities)

        comm_id = get_communities(self.store)[0]["id"]
        node_id = self.store._conn.execute(
            "SELECT id FROM nodes WHERE community_id = ? LIMIT 1",
            (comm_id,),
        ).fetchone()[0]
        self.store._conn.execute(
            "UPDATE nodes SET community_id = NULL WHERE id = ?",
            (node_id,),
        )
        self.store.commit()

        overview = get_architecture_overview(self.store)
        assert any("stored size" in warning for warning in overview["warnings"])


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

    def test_same_name_communities_keep_their_own_members(self):
        for i in range(8):
            self._node(f"fn{i}", i * 10 + 1)
        self.store.commit()

        first = [f"svc/user.py::fn{i}" for i in range(4)]
        second = [f"svc/user.py::fn{i}" for i in range(4, 8)]
        count = store_communities(
            self.store,
            [
                {"name": "svc-user", "size": 4, "cohesion": 0.5, "members": first},
                {"name": "svc-user", "size": 4, "cohesion": 0.5, "members": second},
            ],
        )
        assert count == 2

        rows = self.store._conn.execute("SELECT id, name FROM communities ORDER BY id").fetchall()
        assert len(rows) == 2
        # Names are disambiguated so a by-name lookup is unambiguous.
        assert {row["name"] for row in rows} == {"svc-user", "svc-user-2"}

        by_community = {}
        for row in self.store._conn.execute(
            "SELECT community_id, qualified_name FROM nodes WHERE community_id IS NOT NULL"
        ):
            by_community.setdefault(row["community_id"], set()).add(row["qualified_name"])

        assert len(by_community) == 2, f"members collapsed into {by_community}"
        assert set(by_community[rows[0]["id"]]) == set(first)
        assert set(by_community[rows[1]["id"]]) == set(second)

    def test_refresh_stats_does_not_delete_either_community(self):
        for i in range(8):
            self._node(f"fn{i}", i * 10 + 1)
        self.store.commit()
        store_communities(
            self.store,
            [
                {
                    "name": "svc-user",
                    "size": 4,
                    "cohesion": 0.5,
                    "members": [f"svc/user.py::fn{i}" for i in range(4)],
                },
                {
                    "name": "svc-user",
                    "size": 4,
                    "cohesion": 0.5,
                    "members": [f"svc/user.py::fn{i}" for i in range(4, 8)],
                },
            ],
        )
        refresh_community_stats(self.store)
        self.store.commit()

        remaining = self.store._conn.execute(
            "SELECT name, size FROM communities ORDER BY id"
        ).fetchall()
        assert len(remaining) == 2, f"a community was pruned away: {remaining}"
        assert [row["size"] for row in remaining] == [4, 4]
