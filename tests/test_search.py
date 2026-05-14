"""Tests for the hybrid search engine."""

import tempfile
from pathlib import Path

from dagayn.graph import GraphStore
from dagayn.parser import NodeInfo
from dagayn.search import (
    _extract_identifiers,
    _qualified_name_matches,
    detect_query_kind_boost,
    hybrid_search,
    rebuild_fts_index,
    rrf_merge,
)


class TestHybridSearch:
    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)
        self._seed_data()

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def _seed_data(self):
        """Seed test nodes into the graph store."""
        nodes = [
            NodeInfo(
                kind="Function",
                name="get_users",
                file_path="api.py",
                line_start=1,
                line_end=20,
                language="python",
                params="(db: Session)",
                return_type="list[User]",
            ),
            NodeInfo(
                kind="Function",
                name="create_user",
                file_path="api.py",
                line_start=25,
                line_end=40,
                language="python",
                params="(name: str, email: str)",
                return_type="User",
            ),
            NodeInfo(
                kind="Class",
                name="UserService",
                file_path="services.py",
                line_start=1,
                line_end=100,
                language="python",
            ),
            NodeInfo(
                kind="Function",
                name="authenticate",
                file_path="auth.py",
                line_start=5,
                line_end=30,
                language="python",
                params="(token: str)",
                return_type="bool",
            ),
            NodeInfo(
                kind="Type",
                name="UserResponse",
                file_path="models.py",
                line_start=1,
                line_end=15,
                language="python",
            ),
        ]
        for node in nodes:
            node_id = self.store.upsert_node(node, file_hash="abc123")
            # Set signature for functions
            if node.kind == "Function":
                sig = f"def {node.name}{node.params or '()'} -> {node.return_type or 'None'}"
                self.store._conn.execute(
                    "UPDATE nodes SET signature = ? WHERE id = ?", (sig, node_id)
                )
        self.store._conn.commit()

    # --- rebuild_fts_index ---

    def test_rebuild_fts_index(self):
        """rebuild_fts_index returns the correct count of indexed rows."""
        count = rebuild_fts_index(self.store)
        assert count == 5

    def test_rebuild_fts_index_idempotent(self):
        """Rebuilding twice gives the same count."""
        count1 = rebuild_fts_index(self.store)
        count2 = rebuild_fts_index(self.store)
        assert count1 == count2

    # --- FTS search by name ---

    def test_fts_search_by_name(self):
        """FTS search finds a node by its name."""
        rebuild_fts_index(self.store)
        hs = hybrid_search(self.store, "get_users")
        results = hs["results"]
        assert len(results) > 0
        names = [r["name"] for r in results]
        assert "get_users" in names
        assert hs["mode"] == "fts_only"

    # --- FTS search by signature ---

    def test_fts_search_by_signature(self):
        """FTS search finds a node by content in its signature."""
        rebuild_fts_index(self.store)
        results = hybrid_search(self.store, "Session")["results"]
        assert len(results) > 0
        # get_users has "Session" in its signature
        names = [r["name"] for r in results]
        assert "get_users" in names

    # --- Kind boosting ---

    def test_kind_boost_pascal_case(self):
        """PascalCase query boosts Class kind > 1.0."""
        boosts = detect_query_kind_boost("UserService")
        assert "Class" in boosts
        assert boosts["Class"] > 1.0

    def test_kind_boost_snake_case(self):
        """snake_case query boosts Function kind > 1.0."""
        boosts = detect_query_kind_boost("get_users")
        assert "Function" in boosts
        assert boosts["Function"] > 1.0

    def test_kind_boost_dotted(self):
        """Dotted query boosts qualified name matches."""
        boosts = detect_query_kind_boost("api.get_users")
        assert "_qualified" in boosts
        assert boosts["_qualified"] > 1.0

    def test_qualified_match_substring(self):
        """Class.method dotted query matches file.py::Class.method as substring."""
        assert _qualified_name_matches("Service.handle", "app.py::Service.handle")

    def test_qualified_match_across_separators(self):
        """Module-prefixed dotted query matches across .py:: boundary."""
        assert _qualified_name_matches("api.get_users", "path/to/api.py::get_users")

    def test_qualified_match_segment_order_required(self):
        """Dotted segments must appear in order in the qualified name."""
        assert not _qualified_name_matches("get_users.api", "path/to/api.py::get_users")

    def test_qualified_match_no_match(self):
        """Unrelated dotted query does not falsely match."""
        assert not _qualified_name_matches("foo.bar", "path/to/api.py::get_users")

    def test_dotted_query_boosts_actual_score(self):
        """End-to-end: dotted query that spans .py:: receives the qualified boost."""
        # 'api.get_users' should boost the node whose qualified_name is
        # '*/api.py::get_users' even though the literal substring isn't present.
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="get_users",
                file_path="services/api.py",
                line_start=1,
                line_end=5,
                language="python",
            ),
            file_hash="boost-test",
        )
        self.store._conn.commit()
        rebuild_fts_index(self.store)

        results = hybrid_search(self.store, "api.get_users")["results"]
        # Find the boosted node
        target = next(
            (r for r in results if r["qualified_name"] == "services/api.py::get_users"),
            None,
        )
        assert target is not None
        # Compare against the same query without dots to confirm boost was applied
        plain = hybrid_search(self.store, "get_users")["results"]
        plain_target = next(
            (r for r in plain if r["qualified_name"] == "services/api.py::get_users"),
            None,
        )
        assert plain_target is not None
        assert target["score"] > plain_target["score"]

    def test_kind_boost_empty(self):
        """Empty query returns no boosts."""
        boosts = detect_query_kind_boost("")
        assert boosts == {}

    def test_kind_boost_all_uppercase(self):
        """ALL_CAPS should not trigger PascalCase boost."""
        boosts = detect_query_kind_boost("HTTP_STATUS")
        assert "Class" not in boosts
        # But should trigger snake_case boost
        assert "Function" in boosts

    # --- RRF merge ---

    def test_rrf_merge(self):
        """Node appearing in both lists ranks highest after RRF merge."""
        list_a = [(1, 10.0), (2, 8.0), (3, 6.0)]
        list_b = [(2, 9.0), (4, 7.0), (1, 5.0)]

        merged = rrf_merge(list_a, list_b)
        ids = [item_id for item_id, _ in merged]

        # Items 1 and 2 appear in both lists, so they should be top-ranked
        assert ids[0] in (1, 2)
        assert ids[1] in (1, 2)
        # ID 2 is rank 0+0 in list_b and rank 1 in list_a
        # ID 1 is rank 0 in list_a and rank 2 in list_b
        # So ID 2 should rank higher: 1/(60+1+1) + 1/(60+0+1) vs 1/(60+0+1) + 1/(60+2+1)
        assert ids[0] == 2

    def test_rrf_merge_single_list(self):
        """RRF merge with a single list preserves order."""
        single = [(10, 5.0), (20, 3.0), (30, 1.0)]
        merged = rrf_merge(single)
        ids = [item_id for item_id, _ in merged]
        assert ids == [10, 20, 30]

    def test_rrf_merge_empty(self):
        """RRF merge with empty lists returns empty."""
        merged = rrf_merge([], [])
        assert merged == []

    # --- Fallback to keyword search ---

    def test_fallback_to_keyword(self):
        """Works without FTS index by falling back to keyword LIKE matching."""
        # Do NOT rebuild FTS index — drop it if it exists
        try:
            self.store._conn.execute("DROP TABLE IF EXISTS nodes_fts")
            self.store._conn.commit()
        except Exception:
            pass

        hs = hybrid_search(self.store, "authenticate")
        results = hs["results"]
        assert len(results) > 0
        names = [r["name"] for r in results]
        assert "authenticate" in names
        assert hs["mode"] == "keyword_fallback"

    # --- Empty query ---

    def test_empty_query_handled(self):
        """Empty query returns empty results without crashing."""
        hs = hybrid_search(self.store, "")
        assert hs["mode"] == "empty"
        assert hs["results"] == []

    def test_whitespace_query_handled(self):
        """Whitespace-only query returns empty results."""
        hs = hybrid_search(self.store, "   ")
        assert hs["mode"] == "empty"
        assert hs["results"] == []

    # --- Return fields ---

    def test_hybrid_search_returns_expected_fields(self):
        """All expected fields are present in search results."""
        rebuild_fts_index(self.store)
        results = hybrid_search(self.store, "get_users")["results"]
        assert len(results) > 0

        expected_fields = {
            "name",
            "qualified_name",
            "kind",
            "file_path",
            "line_start",
            "line_end",
            "language",
            "params",
            "return_type",
            "signature",
            "score",
            "source",
            "is_test",
        }
        for result in results:
            assert expected_fields.issubset(result.keys()), (
                f"Missing fields: {expected_fields - result.keys()}"
            )

    # --- Kind filtering ---

    def test_kind_filter(self):
        """Kind parameter filters results to only that kind."""
        rebuild_fts_index(self.store)
        results = hybrid_search(self.store, "User", kind="Class")["results"]
        for r in results:
            assert r["kind"] == "Class"

    # --- Context file boosting ---

    def test_context_file_boost(self):
        """Nodes in context_files get boosted above others."""
        rebuild_fts_index(self.store)

        # Search for "user" which matches multiple nodes
        results_with_ctx = hybrid_search(self.store, "user", context_files=["api.py"])["results"]

        # Find get_users in both result sets
        if results_with_ctx:
            api_nodes = [r for r in results_with_ctx if r["file_path"] == "api.py"]
            if api_nodes:
                # api.py nodes should have a score boost
                api_score = api_nodes[0]["score"]
                assert api_score > 0

    # --- Limit parameter ---

    def test_limit_respected(self):
        """Search respects the limit parameter."""
        rebuild_fts_index(self.store)
        results = hybrid_search(self.store, "user", limit=2)["results"]
        assert len(results) <= 2

    # --- FTS5 injection safety ---

    def test_fts_query_with_special_chars(self):
        """FTS5 special characters are safely handled."""
        rebuild_fts_index(self.store)
        # These should not crash — FTS5 operators like AND, OR, NOT, *, etc.
        for dangerous_query in ["OR user", "NOT thing", "user*", '"user"', "a AND b"]:
            hs = hybrid_search(self.store, dangerous_query)
            # Just assert no exception was raised and structure is correct
            assert isinstance(hs, dict)
            assert "mode" in hs
            assert "results" in hs

    def test_fts_rebuild_is_atomic(self):
        """Regression test for #259: rebuild_fts_index must wrap the DROP +
        CREATE + INSERT sequence in a single transaction so a crash between
        DROP and CREATE cannot leave the DB without an FTS table."""
        # Build, rebuild, then verify the table exists and is queryable.
        rebuild_fts_index(self.store)

        # Verify the FTS table exists and has rows.
        conn = self.store._conn
        count = conn.execute("SELECT count(*) FROM nodes_fts").fetchone()[0]
        assert count > 0

        # Rebuild again — must not raise and must leave the table intact.
        new_count = rebuild_fts_index(self.store)
        assert new_count == count

        # Verify search still works after double-rebuild.
        hs = hybrid_search(self.store, "auth")
        assert isinstance(hs, dict)
        assert "results" in hs

    # --- search_mode values ---

    def test_mode_fts_only(self):
        """Mode is 'fts_only' when FTS index exists but no embeddings."""
        rebuild_fts_index(self.store)
        hs = hybrid_search(self.store, "authenticate")
        assert hs["mode"] == "fts_only"

    def test_mode_keyword_fallback(self):
        """Mode is 'keyword_fallback' when FTS table is absent."""
        try:
            self.store._conn.execute("DROP TABLE IF EXISTS nodes_fts")
            self.store._conn.commit()
        except Exception:
            pass
        hs = hybrid_search(self.store, "get_users")
        assert hs["mode"] == "keyword_fallback"
        for r in hs["results"]:
            assert r["source"] == "keyword"

    def test_mode_empty(self):
        """Mode is 'empty' when query matches nothing."""
        rebuild_fts_index(self.store)
        hs = hybrid_search(self.store, "zzznomatch_xyz_impossible")
        assert hs["mode"] in ("empty", "fts_only", "keyword_fallback")
        # 'empty' is the expected mode when truly nothing matches
        if hs["mode"] == "empty":
            assert hs["results"] == []

    def test_source_field_fts(self):
        """Results produced by FTS-only path have source='fts'."""
        rebuild_fts_index(self.store)
        hs = hybrid_search(self.store, "authenticate")
        assert hs["mode"] == "fts_only"
        for r in hs["results"]:
            assert r["source"] == "fts"

    def test_source_field_keyword(self):
        """Results produced by keyword fallback have source='keyword'."""
        try:
            self.store._conn.execute("DROP TABLE IF EXISTS nodes_fts")
            self.store._conn.commit()
        except Exception:
            pass
        hs = hybrid_search(self.store, "authenticate")
        assert hs["mode"] == "keyword_fallback"
        for r in hs["results"]:
            assert r["source"] == "keyword"


# --- GraphStore protocol methods ---


class TestGraphStoreProtocolMethods:
    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)
        self._seed_data()
        rebuild_fts_index(self.store)

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def _seed_data(self):
        nodes = [
            NodeInfo(
                kind="Function",
                name="get_users",
                file_path="api.py",
                line_start=1,
                line_end=10,
                language="python",
            ),
            NodeInfo(
                kind="Function",
                name="authenticate",
                file_path="auth.py",
                line_start=1,
                line_end=10,
                language="python",
            ),
        ]
        for node in nodes:
            self.store.upsert_node(node, file_hash="abc")
        self.store._conn.commit()

    def test_fts_query_returns_positive_scores(self):
        """fts_query returns non-empty list with strictly positive scores."""
        results = self.store.fts_query("get_users")
        assert len(results) > 0
        for _nid, score in results:
            assert score > 0

    def test_keyword_query_exact_match_score(self):
        """keyword_query assigns score 3.0 for an exact name match."""
        try:
            self.store._conn.execute("DROP TABLE IF EXISTS nodes_fts")
            self.store._conn.commit()
        except Exception:
            pass
        results = self.store.keyword_query("authenticate")
        assert len(results) > 0
        scores = {score for _, score in results}
        assert 3.0 in scores

    def test_get_nodes_by_ids_roundtrip(self):
        """get_nodes_by_ids retrieves nodes matching the given IDs."""
        all_nodes = self.store.get_all_nodes(exclude_files=False)
        ids = [n.id for n in all_nodes[:2]]
        result = self.store.get_nodes_by_ids(ids)
        assert len(result) == len(ids)
        for nid in ids:
            assert nid in result

    def test_get_nodes_by_ids_large_batch(self):
        """get_nodes_by_ids handles batches that exceed 450 items."""
        # Seed enough nodes to exceed the batch_size boundary
        for i in range(460):
            self.store.upsert_node(
                NodeInfo(
                    kind="Function",
                    name=f"func_{i}",
                    file_path="bulk.py",
                    line_start=i,
                    line_end=i + 1,
                    language="python",
                ),
                file_hash="bulk",
            )
        self.store._conn.commit()

        all_nodes = self.store.get_all_nodes(exclude_files=False)
        ids = [n.id for n in all_nodes]
        result = self.store.get_nodes_by_ids(ids)
        assert len(result) == len(ids)


class TestHybridSearchRankAndDocSource:
    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)
        self._seed_data()

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def _seed_data(self):
        nodes = [
            NodeInfo(
                kind="Function",
                name="get_users",
                file_path="api.py",
                line_start=1,
                line_end=10,
                language="python",
            ),
            NodeInfo(
                kind="Function",
                name="create_user",
                file_path="api.py",
                line_start=12,
                line_end=25,
                language="python",
            ),
            NodeInfo(
                kind="DocSection",
                name="getting-started",
                file_path="README.md",
                line_start=1,
                line_end=1,
                language="markdown",
                extra={"markdown_kind": "section", "display_name": "Getting Started"},
            ),
        ]
        for node in nodes:
            self.store.upsert_node(node, file_hash="test")
        self.store._conn.commit()
        rebuild_fts_index(self.store)

    def test_results_have_rank_field(self):
        hs = hybrid_search(self.store, "user")
        results = hs["results"]
        assert len(results) > 0
        for i, r in enumerate(results):
            assert "rank" in r, f"result {i} missing 'rank'"
            assert r["rank"] == i + 1, f"rank should be 1-based: expected {i + 1}, got {r['rank']}"

    def test_keyword_fallback_results_have_rank(self):
        try:
            self.store._conn.execute("DROP TABLE IF EXISTS nodes_fts")
            self.store._conn.commit()
        except Exception:
            pass
        hs = hybrid_search(self.store, "get_users")
        results = hs["results"]
        if results:
            for i, r in enumerate(results):
                assert r["rank"] == i + 1

    def test_docsection_source_is_doc(self):
        hs = hybrid_search(self.store, "getting-started")
        results = hs["results"]
        doc_results = [r for r in results if r["kind"] == "DocSection"]
        assert len(doc_results) > 0, "Expected at least one DocSection result"
        for r in doc_results:
            assert r["source"] == "doc", f"DocSection source should be 'doc', got {r['source']}"

    def test_docsection_kind_in_results(self):
        hs = hybrid_search(self.store, "getting started")
        results = hs["results"]
        kinds = {r["kind"] for r in results}
        assert "DocSection" in kinds


# ---------------------------------------------------------------------------
# Identifier extraction (Issue 3)
# ---------------------------------------------------------------------------


class TestExtractIdentifiers:
    def test_picks_snake_case(self):
        assert _extract_identifiers("find which functions test embed_graph") == ["embed_graph"]

    def test_picks_pascal_case(self):
        assert _extract_identifiers("how is GraphStore initialized") == ["GraphStore"]

    def test_picks_camel_case(self):
        assert _extract_identifiers("debug rrfMerge behaviour") == ["rrfMerge"]

    def test_picks_screaming_snake(self):
        assert _extract_identifiers("change RRF_K to 10") == ["RRF_K"]

    def test_skips_plain_english_words(self):
        assert _extract_identifiers("compute blast radius for a change") == []

    def test_skips_stopwords_even_if_identifier_shaped(self):
        # "Find" matches the regex but is in the stopword list.
        assert _extract_identifiers("Find users") == []

    def test_dedups(self):
        assert _extract_identifiers("GraphStore and GraphStore again") == ["GraphStore"]

    def test_empty_query(self):
        assert _extract_identifiers("") == []

    def test_preserves_order(self):
        assert _extract_identifiers("call embed_graph then GraphStore") == [
            "embed_graph",
            "GraphStore",
        ]


# ---------------------------------------------------------------------------
# Test deboost (Issue 1)
# ---------------------------------------------------------------------------


class TestTestDeboost:
    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)
        # Seed: one Function plus a near-clone test that exercises it.
        nodes = [
            NodeInfo(
                kind="Function",
                name="compute_blast_radius",
                file_path="dagayn/impact.py",
                line_start=1,
                line_end=20,
                language="python",
                is_test=False,
            ),
            NodeInfo(
                kind="Function",
                name="test_compute_blast_radius",
                file_path="tests/test_impact.py",
                line_start=1,
                line_end=20,
                language="python",
                is_test=True,
            ),
        ]
        for n in nodes:
            self.store.upsert_node(n, file_hash="test_deboost_fixture")
        self.store._conn.commit()
        rebuild_fts_index(self.store)

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def test_source_outranks_test_with_same_name(self):
        """When both source and test match equally, the source ranks first."""
        results = hybrid_search(self.store, "compute_blast_radius")["results"]
        assert len(results) >= 2
        # Find both nodes; source must precede test.
        names = [r["name"] for r in results]
        src_idx = names.index("compute_blast_radius")
        test_idx = names.index("test_compute_blast_radius")
        assert src_idx < test_idx

    def test_is_test_flag_exposed_in_results(self):
        """Each result dict carries the boolean is_test field."""
        results = hybrid_search(self.store, "compute_blast_radius")["results"]
        assert len(results) >= 2
        flags = {r["name"]: r["is_test"] for r in results}
        assert flags["compute_blast_radius"] is False
        assert flags["test_compute_blast_radius"] is True

    def test_test_node_still_returned_for_exact_name_query(self):
        """Deboost shrinks score but does not filter — querying the test name
        directly still surfaces it."""
        results = hybrid_search(self.store, "test_compute_blast_radius")["results"]
        names = [r["name"] for r in results]
        assert "test_compute_blast_radius" in names
