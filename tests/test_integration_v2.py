"""Comprehensive end-to-end integration test for the v2 pipeline.

Exercises: communities, FTS search,
find_dead_code, generate_hints, review_changes_prompt,
generate_wiki, and the Registry API.
"""

import tempfile
from pathlib import Path

from dagayn.communities import (
    detect_communities,
    get_communities,
    store_communities,
)
from dagayn.graph import GraphStore
from dagayn.hints import generate_hints, get_session, reset_session
from dagayn.parser import EdgeInfo, NodeInfo
from dagayn.prompts import review_changes_prompt
from dagayn.refactor import find_dead_code
from dagayn.registry import Registry
from dagayn.search import hybrid_search, rebuild_fts_index
from dagayn.wiki import generate_wiki
from tests.dead_code_candidates import graph_dead_code_candidates


class TestV2Integration:
    """End-to-end integration test exercising the full v2 pipeline."""

    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)
        self._seed_realistic_graph()

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    # -----------------------------------------------------------------
    # Graph seeding helpers
    # -----------------------------------------------------------------

    def _seed_realistic_graph(self):
        """Seed a realistic multi-file graph with auth, db, and API layers."""
        s = self.store

        # --- auth.py: authentication module ---
        s.upsert_node(
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
        s.upsert_node(
            NodeInfo(
                kind="Function",
                name="login",
                file_path="auth.py",
                line_start=5,
                line_end=20,
                language="python",
                params="(username: str, password: str)",
                return_type="Token",
                extra={"decorators": ["route"]},
            ),
            file_hash="a1",
        )
        s.upsert_node(
            NodeInfo(
                kind="Function",
                name="logout",
                file_path="auth.py",
                line_start=25,
                line_end=40,
                language="python",
                params="(token: str)",
                return_type="bool",
                extra={"decorators": ["route"]},
            ),
            file_hash="a1",
        )
        s.upsert_node(
            NodeInfo(
                kind="Function",
                name="verify_token",
                file_path="auth.py",
                line_start=45,
                line_end=60,
                language="python",
                params="(token: str)",
                return_type="bool",
            ),
            file_hash="a1",
        )

        # --- db.py: database layer ---
        s.upsert_node(
            NodeInfo(
                kind="File",
                name="db.py",
                file_path="db.py",
                line_start=1,
                line_end=120,
                language="python",
            ),
            file_hash="b1",
        )
        s.upsert_node(
            NodeInfo(
                kind="Class",
                name="Database",
                file_path="db.py",
                line_start=5,
                line_end=60,
                language="python",
            ),
            file_hash="b1",
        )
        s.upsert_node(
            NodeInfo(
                kind="Function",
                name="connect",
                file_path="db.py",
                line_start=10,
                line_end=25,
                language="python",
                parent_name="Database",
                params="(self, dsn: str)",
                return_type="Connection",
            ),
            file_hash="b1",
        )
        s.upsert_node(
            NodeInfo(
                kind="Function",
                name="query",
                file_path="db.py",
                line_start=30,
                line_end=50,
                language="python",
                parent_name="Database",
                params="(self, sql: str)",
                return_type="list[Row]",
            ),
            file_hash="b1",
        )
        s.upsert_node(
            NodeInfo(
                kind="Function",
                name="close",
                file_path="db.py",
                line_start=55,
                line_end=60,
                language="python",
                parent_name="Database",
                params="(self)",
                return_type="None",
            ),
            file_hash="b1",
        )

        # --- api.py: API handlers ---
        s.upsert_node(
            NodeInfo(
                kind="File",
                name="api.py",
                file_path="api.py",
                line_start=1,
                line_end=80,
                language="python",
            ),
            file_hash="c1",
        )
        s.upsert_node(
            NodeInfo(
                kind="Function",
                name="get_users",
                file_path="api.py",
                line_start=5,
                line_end=20,
                language="python",
                params="(request: Request)",
                return_type="Response",
                extra={"decorators": ["route"]},
            ),
            file_hash="c1",
        )
        s.upsert_node(
            NodeInfo(
                kind="Function",
                name="create_user",
                file_path="api.py",
                line_start=25,
                line_end=45,
                language="python",
                params="(request: Request)",
                return_type="Response",
                extra={"decorators": ["route"]},
            ),
            file_hash="c1",
        )

        # --- utils.py: orphaned helper (dead code candidate) ---
        s.upsert_node(
            NodeInfo(
                kind="File",
                name="utils.py",
                file_path="utils.py",
                line_start=1,
                line_end=30,
                language="python",
            ),
            file_hash="d1",
        )
        s.upsert_node(
            NodeInfo(
                kind="Function",
                name="format_date",
                file_path="utils.py",
                line_start=5,
                line_end=15,
                language="python",
                params="(dt: datetime)",
                return_type="str",
            ),
            file_hash="d1",
        )

        # --- test_auth.py: tests ---
        s.upsert_node(
            NodeInfo(
                kind="File",
                name="test_auth.py",
                file_path="test_auth.py",
                line_start=1,
                line_end=40,
                language="python",
            ),
            file_hash="e1",
        )
        s.upsert_node(
            NodeInfo(
                kind="Test",
                name="test_login",
                file_path="test_auth.py",
                line_start=5,
                line_end=15,
                language="python",
                is_test=True,
            ),
            file_hash="e1",
        )

        # --- Edges: calls ---
        call_edges = [
            ("auth.py::login", "auth.py::verify_token", "auth.py", 10),
            ("auth.py::logout", "auth.py::verify_token", "auth.py", 30),
            ("api.py::get_users", "db.py::Database.query", "api.py", 10),
            ("api.py::get_users", "auth.py::verify_token", "api.py", 8),
            ("api.py::create_user", "db.py::Database.query", "api.py", 30),
            ("api.py::create_user", "auth.py::verify_token", "api.py", 28),
            ("db.py::Database.query", "db.py::Database.connect", "db.py", 35),
        ]
        for source, target, fp, ln in call_edges:
            s.upsert_edge(
                EdgeInfo(
                    kind="CALLS",
                    source=source,
                    target=target,
                    file_path=fp,
                    line=ln,
                )
            )

        # --- Edges: contains ---
        contains_edges = [
            ("auth.py", "auth.py::login", "auth.py"),
            ("auth.py", "auth.py::logout", "auth.py"),
            ("auth.py", "auth.py::verify_token", "auth.py"),
            ("db.py", "db.py::Database", "db.py"),
            ("db.py::Database", "db.py::Database.connect", "db.py"),
            ("db.py::Database", "db.py::Database.query", "db.py"),
            ("db.py::Database", "db.py::Database.close", "db.py"),
            ("api.py", "api.py::get_users", "api.py"),
            ("api.py", "api.py::create_user", "api.py"),
            ("utils.py", "utils.py::format_date", "utils.py"),
        ]
        for source, target, fp in contains_edges:
            s.upsert_edge(
                EdgeInfo(
                    kind="CONTAINS",
                    source=source,
                    target=target,
                    file_path=fp,
                    line=1,
                )
            )

        # --- Edges: tested_by ---
        s.upsert_edge(
            EdgeInfo(
                kind="TESTED_BY",
                source="auth.py::login",
                target="test_auth.py::test_login",
                file_path="test_auth.py",
                line=5,
            )
        )

        s.commit()
        s.compute_missing_signatures()
        s.commit()

    # -----------------------------------------------------------------
    # Integration test
    # -----------------------------------------------------------------

    def test_full_pipeline(self):
        """Exercise the full v2 pipeline end-to-end."""

        # ---- Step 1: Verify graph data was seeded correctly ----
        stats = self.store.get_stats()
        assert stats.total_nodes >= 12, f"Expected >= 12 nodes, got {stats.total_nodes}"
        assert stats.total_edges >= 10, f"Expected >= 10 edges, got {stats.total_edges}"

        # ---- Step 3: detect_communities + store_communities ----
        communities = detect_communities(self.store)
        assert isinstance(communities, list)
        assert len(communities) > 0, "Should detect at least one community"

        comm_count = store_communities(self.store, communities)
        assert comm_count == len(communities)

        # Verify retrieval
        stored_comms = get_communities(self.store)
        assert len(stored_comms) > 0
        # Each community should have name and size
        for comm in stored_comms:
            assert "name" in comm
            assert "size" in comm
            assert comm["size"] > 0

        # ---- Step 4: rebuild_fts_index + hybrid_search ----
        fts_count = rebuild_fts_index(self.store)
        assert fts_count > 0, "FTS should index at least some nodes"

        # Search for known functions
        results = hybrid_search(self.store, "login")["results"]
        assert len(results) > 0, "hybrid_search should find 'login'"
        names = [r["name"] for r in results]
        assert any("login" in n for n in names)

        # Search by kind
        results_func = hybrid_search(self.store, "query", kind="Function")["results"]
        assert len(results_func) > 0

        # ---- Step 6: find_dead_code ----
        candidates = graph_dead_code_candidates(self.store)
        candidate_names = [d["name"] for d in candidates]
        # format_date has no callers, no tests, no importers -- a graph candidate
        assert "format_date" in candidate_names, (
            f"format_date should be a dead-code candidate, got: {candidate_names}"
        )
        # but these nodes have no source on disk, so nothing is claimed dead
        assert find_dead_code(self.store) == []

        # ---- Step 7: generate_hints ----
        reset_session()
        session = get_session()
        # A review_tool(mode="changes") reply for auth.py (the change
        # analysis itself runs in Rust; tests/test_review_changes.py covers it).
        hints = generate_hints(
            "detect_changes",
            {
                "summary": "1 changed file(s), 3 changed symbol(s).",
                "risk_score": 0.85,
                "test_gaps": [{"name": "verify_token"}, {"name": "logout"}],
            },
            session,
        )
        assert "next_steps" in hints
        assert isinstance(hints["next_steps"], list)
        assert "Test coverage gaps: verify_token, logout" in hints["warnings"]
        assert any(w.startswith("High risk score (0.85)") for w in hints["warnings"])

        # ---- Step 8: review_changes_prompt ----
        prompt_messages = review_changes_prompt(base="HEAD~1")
        assert isinstance(prompt_messages, list)
        assert len(prompt_messages) > 0
        assert prompt_messages[0]["role"] == "user"
        assert 'review_tool(mode="changes"' in prompt_messages[0]["content"]

        # ---- Step 9: generate_wiki ----
        with tempfile.TemporaryDirectory() as wiki_dir:
            wiki_result = generate_wiki(self.store, wiki_dir, force=True)
            assert "pages_generated" in wiki_result
            assert "pages_updated" in wiki_result
            assert "pages_unchanged" in wiki_result
            total = (
                wiki_result["pages_generated"]
                + wiki_result["pages_updated"]
                + wiki_result["pages_unchanged"]
            )
            # At least one community page should have been generated
            assert total >= 0  # might be 0 if no communities stored
            if stored_comms:
                assert total > 0, "Wiki should generate pages for communities"
                # Verify index file exists
                index_path = Path(wiki_dir) / "index.md"
                assert index_path.exists(), "Wiki should generate index.md"

        # ---- Step 10: Registry (basic API test) ----
        with tempfile.TemporaryDirectory() as reg_dir:
            reg_path = Path(reg_dir) / "registry.json"
            registry = Registry(path=reg_path)

            # Empty initially
            assert registry.list_repos() == []

            # Register a fake repo (create .git dir so validation passes)
            fake_repo = Path(reg_dir) / "my-project"
            fake_repo.mkdir()
            (fake_repo / ".git").mkdir()

            entry = registry.register(str(fake_repo), alias="myproj")
            assert entry["alias"] == "myproj"
            assert str(fake_repo.resolve()) in entry["path"]

            repos = registry.list_repos()
            assert len(repos) == 1

            # Unregister
            assert registry.unregister("myproj") is True
            assert registry.list_repos() == []

    def test_pipeline_idempotent(self):
        """Running the pipeline twice yields consistent results."""
        # First run
        comms1 = detect_communities(self.store)
        store_communities(self.store, comms1)
        fts1 = rebuild_fts_index(self.store)

        # Second run (should overwrite cleanly)
        comms2 = detect_communities(self.store)
        store_communities(self.store, comms2)
        fts2 = rebuild_fts_index(self.store)

        assert len(comms1) == len(comms2)
        assert fts1 == fts2

    def test_search_after_rebuild(self):
        """FTS search works correctly after index rebuild."""
        rebuild_fts_index(self.store)

        # Exact function name
        results = hybrid_search(self.store, "create_user")["results"]
        assert any(r["name"] == "create_user" for r in results)

        # Class name
        results = hybrid_search(self.store, "Database")["results"]
        assert any(r["name"] == "Database" for r in results)

        # Partial match
        results = hybrid_search(self.store, "user")["results"]
        names = [r["name"] for r in results]
        assert any("user" in n.lower() for n in names)
