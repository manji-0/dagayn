"""Tests for the hybrid search engine."""

import inspect
import tempfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from dagayn import fts_tokenize
from dagayn.embeddings import _encode_vector
from dagayn.graph import GraphStore
from dagayn.graph import _fts_tokenize as graph_fts_tokenize
from dagayn.graph import search as graph_search
from dagayn.graph._fts_tokenize import FTS_SEGMENTER_METADATA_KEY
from dagayn.graph.types import FtsQueryResult
from dagayn.parser import NodeInfo
from dagayn.search import (
    _emb_cache,
    _emb_failure_cache,
    _embedding_text_mode_for_intent,
    _extract_identifiers,
    _get_cached_emb_store,
    _intent_boost,
    _qualified_name_matches,
    _query_rerank_intent,
    _query_tokens,
    detect_query_kind_boost,
    embedding_health_available,
    hybrid_search,
    rebuild_fts_index,
    rrf_merge,
)


def test_graph_search_uses_graph_local_fts_tokenizer():
    """Keep graph search from reintroducing a root dagayn import cycle."""
    assert graph_search.segment_japanese_fts_text is graph_fts_tokenize.segment_japanese_fts_text
    source = inspect.getsource(graph_search)
    assert "from dagayn import fts_tokenize" not in source
    assert "from .. import fts_tokenize" not in source


def test_fts_tokenize_shim_reexports_graph_impl():
    assert fts_tokenize.segment_japanese_fts_text is graph_fts_tokenize.segment_japanese_fts_text
    assert fts_tokenize.contains_japanese is graph_fts_tokenize.contains_japanese


def test_contains_japanese_includes_hangul():
    """Hangul syllables are segmented like CJK (regression for #139)."""
    assert graph_fts_tokenize.contains_japanese("안녕하세요") is True
    assert graph_fts_tokenize.contains_japanese("Hello") is False
    tokens = graph_fts_tokenize.segment_cjk_identifier_tokens("안녕하세요")
    assert tokens == "안녕 녕하 하세 세요"


def test_embedding_health_available_uses_status_field():
    assert embedding_health_available({"status": "available"}) is True
    assert embedding_health_available({"status": "degraded"}) is True
    assert embedding_health_available({"status": "provider_unavailable"}) is False
    assert embedding_health_available({"status": "not_requested"}) is True
    assert embedding_health_available(None) is True
    assert embedding_health_available({}) is True


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

    def test_fts_search_splits_camel_case_identifiers(self):
        """FTS search matches natural-language words against PascalCase symbols."""
        self.store.upsert_node(
            NodeInfo(
                kind="Class",
                name="OpenAIEmbeddingProvider",
                file_path="embeddings.py",
                line_start=1,
                line_end=10,
                language="python",
            ),
            file_hash="abc123",
        )
        self.store.commit()
        rebuild_fts_index(self.store)

        results = hybrid_search(self.store, "open ai embedding provider")["results"]
        assert results
        assert results[0]["name"] == "OpenAIEmbeddingProvider"

    def test_fts_search_indexes_markdown_section_body(self, tmp_path):
        """Markdown section body text is searchable, not just the heading slug."""
        docs = tmp_path / "docs"
        docs.mkdir()
        (docs / "design.md").write_text(
            "# Search Quality\n\n"
            "Use reciprocal rank fusion to merge ranked lists.\n\n"
            "# Other\n\n"
            "Unrelated text.\n",
            encoding="utf-8",
        )
        self.store.set_metadata("repo_root", str(tmp_path))
        self.store.upsert_node(
            NodeInfo(
                kind="DocSection",
                name="search-quality",
                file_path="docs/design.md",
                line_start=1,
                line_end=1,
                language="markdown",
                extra={"display_name": "Search Quality"},
            ),
            file_hash="abc123",
        )
        self.store.commit()
        rebuild_fts_index(self.store)

        results = hybrid_search(self.store, "reciprocal rank fusion")["results"]
        assert results
        assert results[0]["qualified_name"] == "docs/design.md::search-quality"

    def test_fts_search_segments_japanese_markdown_body(self, tmp_path):
        """Japanese body text is segmented for FTS while embedded English stays searchable."""
        docs = tmp_path / "docs"
        docs.mkdir()
        (docs / "design.md").write_text(
            "# 日本語検索\n\nGraphStoreで自然言語検索を行う。\n\n# Other\n\nUnrelated text.\n",
            encoding="utf-8",
        )
        self.store.set_metadata("repo_root", str(tmp_path))
        self.store.upsert_node(
            NodeInfo(
                kind="DocSection",
                name="japanese-search",
                file_path="docs/design.md",
                line_start=1,
                line_end=1,
                language="markdown",
                extra={"display_name": "日本語検索"},
            ),
            file_hash="abc123",
        )
        self.store.commit()
        rebuild_fts_index(self.store)

        segmented = fts_tokenize.segment_japanese_fts_text("GraphStoreで自然言語検索")
        assert "GraphStore" in segmented

        results = hybrid_search(self.store, "GraphStore 自然言語検索")["results"]
        assert results
        assert results[0]["qualified_name"] == "docs/design.md::japanese-search"

    def test_fts_search_finds_cjk_symbol_without_source(self):
        """CJK symbol names are searchable via identifier_tokens even without source text."""
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="ユーザー取得",
                file_path="jp.py",
                line_start=1,
                line_end=5,
                language="python",
            ),
            file_hash="abc123",
        )
        self.store.commit()
        rebuild_fts_index(self.store)

        assert self.store.get_metadata(FTS_SEGMENTER_METADATA_KEY) is not None

        row = self.store._conn.execute(
            "SELECT identifier_tokens, doc_text FROM nodes_fts WHERE name = ?",
            ("ユーザー取得",),
        ).fetchone()
        assert "ユー" in row["identifier_tokens"]
        assert "取得" in row["identifier_tokens"]

        for query in ("ユーザー取得", "ユーザー"):
            results = self.store.fts_query(query)
            assert results.hits, f"expected hits for {query!r}"

        hs = hybrid_search(self.store, "ユーザー取得")
        assert hs["results"]
        assert hs["results"][0]["name"] == "ユーザー取得"

    def test_fts_query_uses_persisted_segmenter(self, monkeypatch):
        """Queries segment with the index-time segmenter recorded in metadata."""
        self.store.set_metadata(FTS_SEGMENTER_METADATA_KEY, "bigram")
        calls: list[str | None] = []
        original = graph_search.segment_japanese_fts_text

        def recording_segment(text, *, segmenter=None):
            calls.append(segmenter)
            return original(text, segmenter=segmenter)

        monkeypatch.setattr(graph_search, "segment_japanese_fts_text", recording_segment)
        self.store.fts_query("自然言語検索")
        assert calls == ["bigram"]

    def test_cjk_identifier_tokens_include_wakati_when_segmenter_pinned(self, monkeypatch):
        """Wakati-indexed identifier_tokens must still answer wakati-shaped queries."""
        from dagayn.graph import _fts_tokenize as fts_tokenize

        def fake_wakati(text: str) -> str:
            if text == "ユーザー取得":
                return "ユーザー 取得"
            return text

        monkeypatch.setattr(fts_tokenize, "detect_fts_segmenter", lambda: "fugashi")
        monkeypatch.setattr(fts_tokenize, "_get_wakati", lambda _name: fake_wakati)

        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="ユーザー取得",
                file_path="jp.py",
                line_start=1,
                line_end=5,
                language="python",
            ),
            file_hash="abc123",
        )
        self.store.commit()
        rebuild_fts_index(self.store)

        row = self.store._conn.execute(
            "SELECT identifier_tokens FROM nodes_fts WHERE name = ?",
            ("ユーザー取得",),
        ).fetchone()
        assert "ユーザー" in row["identifier_tokens"]
        assert "取得" in row["identifier_tokens"]

        fts = self.store.fts_query("ユーザー")
        assert fts.hits

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
        assert hs["embedding_health"]["status"] == "not_requested"
        assert hs["truncated"] is False
        assert hs["total"] == 0

    def test_whitespace_query_handled(self):
        """Whitespace-only query returns empty results."""
        hs = hybrid_search(self.store, "   ")
        assert hs["mode"] == "empty"
        assert hs["results"] == []
        assert "embedding_health" in hs
        assert hs["truncated"] is False
        assert hs["total"] == 0

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

    def test_junk_path_query_does_not_match_via_or_segments(self):
        """Issue #40: path-shaped junk must not FTS-match on shared segments."""
        for path, name in [("src/a.py", "alpha"), ("src/b.py", "beta")]:
            self.store.upsert_node(
                NodeInfo(
                    kind="Function",
                    name=name,
                    file_path=path,
                    line_start=1,
                    line_end=5,
                    language="python",
                ),
                file_hash="abc123",
            )
        self.store.commit()
        rebuild_fts_index(self.store)

        fts = self.store.fts_query("src/nonexistent_zzz.py")
        assert fts.hits == []
        assert fts.match_mode == "none"

        hs = hybrid_search(self.store, "src/nonexistent_zzz.py")
        assert hs["mode"] in ("empty", "keyword_fallback")
        assert not any(r["name"] in {"alpha", "beta"} for r in hs["results"])

    def test_junk_dotted_query_does_not_match_via_or_segments(self):
        """Issue #40: dotted junk must not FTS-match unrelated alpha symbols."""
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="alpha",
                file_path="src/a.py",
                line_start=1,
                line_end=5,
                language="python",
            ),
            file_hash="abc123",
        )
        self.store.commit()
        rebuild_fts_index(self.store)

        fts = self.store.fts_query("totally.bogus.alpha")
        assert fts.hits == []
        assert fts.match_mode == "none"

        hs = hybrid_search(self.store, "totally.bogus.alpha")
        assert not any(r["name"] == "alpha" for r in hs["results"])

    def test_junk_short_unique_segment_does_not_anchor_on_common_src(self):
        """OR fallback must anchor on uncommon segments, not longest common ones."""
        for path, name in [("src/a.py", "alpha"), ("lib/b.py", "beta")]:
            self.store.upsert_node(
                NodeInfo(
                    kind="Function",
                    name=name,
                    file_path=path,
                    line_start=1,
                    line_end=5,
                    language="python",
                ),
                file_hash="abc123",
            )
        self.store.commit()
        rebuild_fts_index(self.store)

        fts = self.store.fts_query("src/x.py")
        assert fts.hits == []
        assert fts.match_mode == "none"

    def test_kind_filter_with_small_limit_finds_class(self):
        """Issue #40: kind filter must not hide matches behind candidate truncation."""
        for i in range(20):
            self.store.upsert_node(
                NodeInfo(
                    kind="Function",
                    name=f"handler_{i}",
                    file_path="handlers.py",
                    line_start=i,
                    line_end=i + 1,
                    language="python",
                ),
                file_hash="abc123",
            )
        self.store.upsert_node(
            NodeInfo(
                kind="Class",
                name="HandlerThing",
                file_path="z.py",
                line_start=1,
                line_end=20,
                language="python",
            ),
            file_hash="abc123",
        )
        self.store.commit()
        rebuild_fts_index(self.store)

        results = hybrid_search(self.store, "handler", kind="Class", limit=5)["results"]
        assert results
        assert any(r["name"] == "HandlerThing" for r in results)
        assert all(r["kind"] == "Class" for r in results)

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
        assert hs["embedding_health"]["status"] in {
            "provider_unavailable",
            "missing_vectors",
        }

    def test_embedding_health_reports_missing_provider_env(self, monkeypatch):
        rebuild_fts_index(self.store)
        monkeypatch.delenv("CRG_OPENAI_API_KEY", raising=False)
        monkeypatch.delenv("CRG_OPENAI_BASE_URL", raising=False)
        monkeypatch.delenv("CRG_OPENAI_MODEL", raising=False)
        self.store._conn.execute(
            """
            CREATE TABLE IF NOT EXISTS embeddings (
                qualified_name TEXT PRIMARY KEY,
                vector BLOB NOT NULL,
                text_hash TEXT NOT NULL,
                provider TEXT NOT NULL DEFAULT 'unknown'
            )
            """
        )
        self.store._conn.execute(
            "INSERT OR REPLACE INTO embeddings VALUES (?, ?, ?, ?)",
            ("auth.py::authenticate", b"\x00\x00\x00\x00", "hash", "openai:qwen3@localhost"),
        )
        self.store._conn.commit()

        hs = hybrid_search(self.store, "authenticate", provider="openai", model="qwen3")

        assert hs["mode"] == "fts_only"
        assert hs["embedding_health"]["status"] == "missing_provider_env"
        assert hs["embedding_health"]["provider_counts"] == {"openai:qwen3@localhost": 1}

    def test_auto_resolves_single_localhost_openai_provider(self, monkeypatch):
        rebuild_fts_index(self.store)
        monkeypatch.delenv("CRG_OPENAI_API_KEY", raising=False)
        monkeypatch.delenv("CRG_OPENAI_BASE_URL", raising=False)
        monkeypatch.delenv("CRG_OPENAI_MODEL", raising=False)
        _emb_failure_cache.clear()
        _emb_cache.clear()
        provider_name = "openai:qwen@http://127.0.0.1:18080/v1#dim=2"
        node = self.store.get_node("auth.py::authenticate")
        assert node is not None
        self.store._conn.execute(
            """
            CREATE TABLE IF NOT EXISTS embeddings (
                qualified_name TEXT PRIMARY KEY,
                vector BLOB NOT NULL,
                text_hash TEXT NOT NULL,
                provider TEXT NOT NULL DEFAULT 'unknown'
            )
            """
        )
        self.store._conn.execute(
            "INSERT OR REPLACE INTO embeddings VALUES (?, ?, ?, ?)",
            (
                "auth.py::authenticate",
                _encode_vector([1.0, 0.0]),
                "hash",
                provider_name,
            ),
        )
        self.store._conn.commit()

        with (
            patch.object(
                self.store,
                "fts_query",
                return_value=FtsQueryResult(hits=[(node.id, 0.5)], match_mode="or"),
            ),
            patch(
                "dagayn.embeddings.OpenAIEmbeddingProvider._call_api",
                return_value=[[1.0, 0.0]],
            ),
        ):
            hs = hybrid_search(self.store, "token validation")

        assert hs["mode"] == "hybrid"
        assert hs["rerank_intent"] == "purpose"
        assert embedding_health_available(hs["embedding_health"])
        assert hs["embedding_health"]["status"] in {"available", "degraded"}
        assert hs["embedding_health"]["resolved_provider"] == provider_name
        assert hs["embedding_health"]["auto_resolved_provider"] == provider_name
        assert any(r["qualified_name"] == "auth.py::authenticate" for r in hs["results"])

    def test_auto_resolves_dominant_provider_when_multiple_exist(self, monkeypatch):
        rebuild_fts_index(self.store)
        monkeypatch.delenv("CRG_OPENAI_API_KEY", raising=False)
        monkeypatch.delenv("CRG_OPENAI_BASE_URL", raising=False)
        monkeypatch.delenv("CRG_OPENAI_MODEL", raising=False)
        _emb_failure_cache.clear()
        _emb_cache.clear()
        dominant = "openai:qwen@http://127.0.0.1:18080/v1"
        abandoned = "openai:old@http://127.0.0.1:18081/v1"
        self.store._conn.execute(
            """
            CREATE TABLE IF NOT EXISTS embeddings (
                qualified_name TEXT PRIMARY KEY,
                vector BLOB NOT NULL,
                text_hash TEXT NOT NULL,
                provider TEXT NOT NULL DEFAULT 'unknown'
            )
            """
        )
        self.store._conn.executemany(
            "INSERT OR REPLACE INTO embeddings VALUES (?, ?, ?, ?)",
            [
                ("auth.py::authenticate", _encode_vector([1.0, 0.0]), "hash", dominant),
                ("auth.py::login", _encode_vector([0.9, 0.1]), "hash2", dominant),
                ("auth.py::logout", _encode_vector([0.8, 0.2]), "hash3", dominant),
                ("old.py::gone", _encode_vector([0.0, 1.0]), "hash4", abandoned),
            ],
        )
        self.store._conn.commit()

        with patch(
            "dagayn.embeddings.OpenAIEmbeddingProvider._call_api",
            return_value=[[1.0, 0.0]],
        ):
            hs = hybrid_search(self.store, "token validation")

        # "token validation" matches no identifier in the fixture, so the FTS
        # arm is legitimately empty and the semantic arm answers alone. What
        # this test is about is which provider got resolved.
        assert hs["mode"] in {"hybrid", "embedding_only"}
        assert embedding_health_available(hs["embedding_health"])
        assert hs["embedding_health"]["status"] in {"available", "degraded"}
        assert hs["embedding_health"]["matching_vector_count"] == 3
        assert hs["embedding_health"]["resolved_provider"] == dominant
        assert hs["embedding_health"]["auto_resolved_provider"] == dominant
        assert any(r["qualified_name"] == "auth.py::authenticate" for r in hs["results"])
        _emb_failure_cache.clear()
        _emb_cache.clear()

    def test_embedding_search_failure_is_cached_briefly(self, monkeypatch):
        rebuild_fts_index(self.store)
        monkeypatch.delenv("CRG_OPENAI_API_KEY", raising=False)
        monkeypatch.delenv("CRG_OPENAI_BASE_URL", raising=False)
        monkeypatch.delenv("CRG_OPENAI_MODEL", raising=False)
        provider_name = "openai:qwen@http://127.0.0.1:18080/v1#dim=2"
        self.store._conn.execute(
            """
            CREATE TABLE IF NOT EXISTS embeddings (
                qualified_name TEXT PRIMARY KEY,
                vector BLOB NOT NULL,
                text_hash TEXT NOT NULL,
                provider TEXT NOT NULL DEFAULT 'unknown'
            )
            """
        )
        self.store._conn.execute(
            "INSERT OR REPLACE INTO embeddings VALUES (?, ?, ?, ?)",
            ("auth.py::authenticate", _encode_vector([1.0, 0.0]), "hash", provider_name),
        )
        self.store._conn.commit()
        _emb_failure_cache.clear()
        _emb_cache.clear()

        with patch(
            "dagayn.embeddings.OpenAIEmbeddingProvider._call_api",
            side_effect=ConnectionError("server down"),
        ) as call_api:
            first = hybrid_search(self.store, "token validation")
            second = hybrid_search(self.store, "token validation again")

        assert first["embedding_health"]["status"] == "search_failed"
        assert second["embedding_health"]["status"] == "search_failed_recent"
        assert call_api.call_count == 1
        _emb_failure_cache.clear()
        _emb_cache.clear()

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
        assert len(results.hits) > 0
        for _nid, score in results.hits:
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

    def test_keyword_query_matches_non_ascii_uppercase(self):
        """keyword_query folds case in Python so Greek/Cyrillic matches.

        SQLite's LOWER() is ASCII-only; an uppercase Greek identifier must
        still be found by its lowercase spelling.
        """
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="\u03a0\u03b1\u03c1\u03ac\u03b4\u03b5\u03b9\u03b3\u03bc\u03b1",  # Παράδειγμα
                file_path="el.py",
                line_start=1,
                line_end=10,
                language="python",
            ),
            file_hash="abc",
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="\u039f\u03c3\u03bf",  # Οσο
                file_path="el2.py",
                line_start=1,
                line_end=10,
                language="python",
            ),
            file_hash="abc",
        )
        self.store._conn.commit()
        try:
            self.store._conn.execute("DROP TABLE IF EXISTS nodes_fts")
            self.store._conn.commit()
        except Exception:
            pass

        results = self.store.keyword_query(
            "\u03c0\u03b1\u03c1\u03ac\u03b4\u03b5\u03b9\u03b3\u03bc\u03b1"
        )  # παράδειγμα
        hit_ids = {nid for nid, _ in results}
        names = {n.name for n in self.store.get_all_nodes(exclude_files=False) if n.id in hit_ids}
        assert "\u03a0\u03b1\u03c1\u03ac\u03b4\u03b5\u03b9\u03b3\u03bc\u03b1" in names

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


# ---------------------------------------------------------------------------
# Intent reranking
# ---------------------------------------------------------------------------


class TestIntentReranking:
    def test_query_rerank_intent_distinguishes_exact_purpose_and_process(self):
        assert _query_rerank_intent("get_users", _query_tokens("get_users")) == "exact"
        assert (
            _query_rerank_intent(
                "function that reads bounded source span",
                _query_tokens("function that reads bounded source span"),
            )
            == "process_pattern"
        )
        assert (
            _query_rerank_intent(
                "code responsible for token validation",
                _query_tokens("code responsible for token validation"),
            )
            == "purpose"
        )
        assert _embedding_text_mode_for_intent("purpose") == "material"
        assert _embedding_text_mode_for_intent("process_pattern") == "narrative"

    def test_code_intent_prefers_code_over_markdown(self):
        tokens = _query_tokens("find the implementation that combines ranked search results")
        function = SimpleNamespace(
            kind="Function",
            name="hybrid_search",
            file_path="dagayn/search.py",
            is_test=False,
        )
        doc = SimpleNamespace(
            kind="DocSection",
            name="hybrid-search",
            file_path="README.md",
            is_test=False,
        )

        assert _intent_boost(tokens, function, None, None, hybrid_mode=True) > _intent_boost(
            tokens, doc, None, None, hybrid_mode=True
        )

    def test_documentation_intent_prefers_docsection(self):
        tokens = _query_tokens("documentation section for starting the mcp server")
        function = SimpleNamespace(
            kind="Function",
            name="start_server",
            file_path="dagayn/main.py",
            is_test=False,
        )
        doc = SimpleNamespace(
            kind="DocSection",
            name="start-the-mcp-server",
            file_path="docs/USAGE.md",
            is_test=False,
        )

        assert _intent_boost(tokens, doc, None, None, hybrid_mode=True) > _intent_boost(
            tokens, function, None, None, hybrid_mode=True
        )

    def test_process_pattern_intent_prefers_embedding_code_hit(self):
        tokens = _query_tokens("function that reads source and returns ranked results")
        function = SimpleNamespace(
            kind="Function",
            name="read_and_rank",
            file_path="dagayn/search.py",
            is_test=False,
        )
        doc = SimpleNamespace(
            kind="DocSection",
            name="search-results",
            file_path="docs/ARCHITECTURE.md",
            is_test=False,
        )

        assert _intent_boost(
            tokens,
            function,
            fts_rank=12,
            emb_rank=2,
            hybrid_mode=True,
            rerank_intent="process_pattern",
        ) > _intent_boost(
            tokens,
            doc,
            fts_rank=1,
            emb_rank=2,
            hybrid_mode=True,
            rerank_intent="process_pattern",
        )

    def test_purpose_intent_prefers_both_arm_code_hit(self):
        tokens = _query_tokens("code responsible for token validation")
        both_arm = SimpleNamespace(
            kind="Function",
            name="authenticate",
            file_path="auth.py",
            is_test=False,
        )
        embedding_only = SimpleNamespace(
            kind="Function",
            name="validate",
            file_path="auth.py",
            is_test=False,
        )

        assert _intent_boost(
            tokens,
            both_arm,
            fts_rank=4,
            emb_rank=3,
            hybrid_mode=True,
            rerank_intent="purpose",
        ) > _intent_boost(
            tokens,
            embedding_only,
            fts_rank=None,
            emb_rank=1,
            hybrid_mode=True,
            rerank_intent="purpose",
        )

    def test_hybrid_process_pattern_rerank_promotes_embedding_code_hit(self):
        tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        store = GraphStore(tmp.name)
        try:
            function = NodeInfo(
                kind="Function",
                name="merge_ranked_results",
                file_path="search.py",
                line_start=1,
                line_end=10,
                language="python",
            )
            doc = NodeInfo(
                kind="DocSection",
                name="ranked-search-results",
                file_path="README.md",
                line_start=1,
                line_end=3,
                language="markdown",
                extra={"display_name": "ranked search results"},
            )
            function_id = store.upsert_node(function, file_hash="rerank")
            doc_id = store.upsert_node(doc, file_hash="rerank")
            store._conn.commit()
            rebuild_fts_index(store)

            with (
                patch.object(
                    store,
                    "fts_query",
                    return_value=FtsQueryResult(hits=[(function_id, 0.5)], match_mode="and"),
                ),
                patch(
                    "dagayn.legacy_py.search._embedding_search_with_health",
                    return_value=(
                        [(function_id, 0.99), (doc_id, 0.8)],
                        {"status": "available", "resolved_provider": "test"},
                    ),
                ) as embedding_search,
            ):
                result = hybrid_search(
                    store,
                    "function that merges ranked search results",
                    limit=5,
                )

            assert result["mode"] == "hybrid"
            assert result["rerank_intent"] == "process_pattern"
            assert embedding_search.call_args.kwargs["text_mode"] == "narrative"
            assert result["results"][0]["qualified_name"] == "search.py::merge_ranked_results"
        finally:
            store.close()
            Path(tmp.name).unlink(missing_ok=True)

    def test_hybrid_process_pattern_uses_narrative_embedding_partition(self):
        tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        store = GraphStore(tmp.name)

        class FakeProvider:
            name = "fake"
            preferred_batch_size = 1

            def embed(self, texts):
                return [[1.0, 0.0] for _ in texts]

            def embed_query(self, text):
                return [1.0, 0.0]

            @property
            def dimension(self):
                return 2

        try:
            node = NodeInfo(
                kind="Function",
                name="read_source_span",
                file_path="search.py",
                line_start=1,
                line_end=10,
                language="python",
            )
            store.upsert_node(node, file_hash="rerank")
            store._conn.execute(
                """
                CREATE TABLE IF NOT EXISTS embeddings (
                    qualified_name TEXT NOT NULL,
                    vector BLOB NOT NULL,
                    text_hash TEXT NOT NULL,
                    provider TEXT NOT NULL DEFAULT 'unknown',
                    PRIMARY KEY (qualified_name, provider)
                )
                """
            )
            store._conn.executemany(
                "INSERT OR REPLACE INTO embeddings VALUES (?, ?, ?, ?)",
                [
                    (
                        "search.py::read_source_span",
                        _encode_vector([0.0, 1.0]),
                        "h1",
                        "fake#text=material",
                    ),
                    (
                        "search.py::read_source_span",
                        _encode_vector([1.0, 0.0]),
                        "h2",
                        "fake#text=narrative",
                    ),
                ],
            )
            store._conn.commit()
            rebuild_fts_index(store)

            with patch("dagayn.embeddings.get_provider", return_value=FakeProvider()):
                result = hybrid_search(
                    store,
                    "function that reads source span and returns text",
                    limit=5,
                )

            assert result["embedding_health"]["status"] == "available"
            assert result["embedding_health"]["requested_text_mode"] == "narrative"
            assert result["embedding_health"]["resolved_provider_key"] == "fake#text=narrative"
            assert result["results"][0]["qualified_name"] == "search.py::read_source_span"
        finally:
            store.close()
            Path(tmp.name).unlink(missing_ok=True)

    def test_hybrid_process_pattern_falls_back_to_material_partition(self):
        tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        store = GraphStore(tmp.name)

        class FakeProvider:
            name = "fake"
            preferred_batch_size = 1

            def embed(self, texts):
                return [[1.0, 0.0] for _ in texts]

            def embed_query(self, text):
                return [1.0, 0.0]

            @property
            def dimension(self):
                return 2

        try:
            node = NodeInfo(
                kind="Function",
                name="read_source_span",
                file_path="search.py",
                line_start=1,
                line_end=10,
                language="python",
            )
            store.upsert_node(node, file_hash="rerank")
            store._conn.execute(
                """
                CREATE TABLE IF NOT EXISTS embeddings (
                    qualified_name TEXT NOT NULL,
                    vector BLOB NOT NULL,
                    text_hash TEXT NOT NULL,
                    provider TEXT NOT NULL DEFAULT 'unknown',
                    PRIMARY KEY (qualified_name, provider)
                )
                """
            )
            store._conn.execute(
                "INSERT OR REPLACE INTO embeddings VALUES (?, ?, ?, ?)",
                (
                    "search.py::read_source_span",
                    _encode_vector([1.0, 0.0]),
                    "h1",
                    "fake#text=material",
                ),
            )
            store._conn.commit()
            rebuild_fts_index(store)

            with patch("dagayn.embeddings.get_provider", return_value=FakeProvider()):
                result = hybrid_search(
                    store,
                    "function that reads source span and returns text",
                    limit=5,
                )

            health = result["embedding_health"]
            assert health["status"] == "available"
            assert health["requested_text_mode"] == "narrative"
            assert health["resolved_text_mode"] == "material"
            assert health["resolved_provider_key"] == "fake#text=material"
            assert health["text_mode_fallback"] == {
                "from": "narrative",
                "to": "material",
                "provider_key": "fake#text=material",
                "vector_count": 1,
            }
            assert result["mode"] == "hybrid"
            assert result["results"][0]["qualified_name"] == "search.py::read_source_span"
        finally:
            store.close()
            Path(tmp.name).unlink(missing_ok=True)


class TestCachedEmbeddingStore:
    def test_get_cached_emb_store_reuses_instance(self, tmp_path, monkeypatch):
        from dagayn.embeddings import EmbeddingStore

        db = tmp_path / "graph.db"
        # Touch an empty SQLite file so mtime is stable.
        GraphStore(db).close()

        class FakeProvider:
            name = "fake"
            preferred_batch_size = 1

            def embed(self, texts):
                return [[1.0, 0.0] for _ in texts]

            def embed_query(self, text):
                return [1.0, 0.0]

            @property
            def dimension(self):
                return 2

        _emb_cache.clear()
        monkeypatch.setenv("DAGAYN_EMBEDDING_SEARCH_BACKEND", "python")

        with patch("dagayn.embeddings.get_provider", return_value=FakeProvider()):
            first = _get_cached_emb_store(db, provider=None, model=None)
            second = _get_cached_emb_store(db, provider=None, model=None)

        assert first is not None
        assert second is first
        assert isinstance(first, EmbeddingStore)
        assert len(_emb_cache) == 1
