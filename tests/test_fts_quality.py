"""Tests for the FTS quality benchmark (dagayn/eval/benchmarks/fts_quality.py)."""

import tempfile
from pathlib import Path

from dagayn.eval.benchmarks.fts_quality import _matches, run
from dagayn.graph import GraphStore
from dagayn.parser import NodeInfo
from dagayn.search import rebuild_fts_index

# ---------------------------------------------------------------------------
# _matches helper
# ---------------------------------------------------------------------------


def test_matches_exact_substring():
    assert _matches("dagayn/search.py::hybrid_search", "dagayn/search.py::hybrid_search")


def test_matches_name_suffix():
    assert _matches("dagayn/search.py::hybrid_search", "somewhere.py::hybrid_search")


def test_matches_no_namespace():
    assert _matches("dagayn/search.py::rrf_merge", "rrf_merge")


def test_matches_expected_in_qn():
    assert _matches("dagayn/embeddings.py::LocalEmbeddingProvider", "LocalEmbeddingProvider")


def test_no_match():
    assert not _matches("dagayn/search.py::hybrid_search", "nowhere.py::nonexistent")


# ---------------------------------------------------------------------------
# Fixture
# ---------------------------------------------------------------------------


class _FtsFixture:
    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)
        self._seed()

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def _seed(self):
        nodes = [
            NodeInfo(
                kind="Function",
                name="hybrid_search",
                file_path="dagayn/search.py",
                line_start=1,
                line_end=50,
                language="python",
                params="(store, query, kind=None, limit=20)",
                return_type="dict",
            ),
            NodeInfo(
                kind="Function",
                name="rrf_merge",
                file_path="dagayn/search.py",
                line_start=55,
                line_end=80,
                language="python",
            ),
            NodeInfo(
                kind="Function",
                name="rebuild_fts_index",
                file_path="dagayn/search.py",
                line_start=85,
                line_end=110,
                language="python",
            ),
            NodeInfo(
                kind="Function",
                name="detect_query_kind_boost",
                file_path="dagayn/search.py",
                line_start=115,
                line_end=140,
                language="python",
            ),
            NodeInfo(
                kind="Class",
                name="GraphStore",
                file_path="dagayn/graph/core.py",
                line_start=1,
                line_end=200,
                language="python",
            ),
            NodeInfo(
                kind="Class",
                name="EmbeddingProvider",
                file_path="dagayn/embeddings.py",
                line_start=1,
                line_end=60,
                language="python",
            ),
            NodeInfo(
                kind="Class",
                name="LocalEmbeddingProvider",
                file_path="dagayn/embeddings.py",
                line_start=65,
                line_end=130,
                language="python",
            ),
            NodeInfo(
                kind="Function",
                name="full_build",
                file_path="dagayn/incremental.py",
                line_start=1,
                line_end=50,
                language="python",
            ),
        ]
        for node in nodes:
            node_id = self.store.upsert_node(node, file_hash="fts_eval_fixture")
            if node.kind == "Function" and node.params:
                sig = f"def {node.name}{node.params} -> {node.return_type or 'None'}"
                self.store._conn.execute(
                    "UPDATE nodes SET signature = ? WHERE id = ?", (sig, node_id)
                )
        self.store._conn.commit()
        rebuild_fts_index(self.store)

    def _config(self, queries: list[dict]) -> dict:
        return {"name": "dagayn_test", "search_queries": queries}


# ---------------------------------------------------------------------------
# Basic run behaviour
# ---------------------------------------------------------------------------


class TestFtsQualityRun(_FtsFixture):
    def test_empty_config_returns_empty(self):
        assert run(Path("/tmp"), self.store, {"name": "x"}) == []

    def test_no_queries_key_returns_empty(self):
        assert run(Path("/tmp"), self.store, {"name": "x", "search_queries": []}) == []

    def test_returns_per_query_plus_aggregate(self):
        config = self._config(
            [
                {"query": "hybrid_search", "expected": "dagayn/search.py::hybrid_search"},
                {"query": "rrf_merge", "expected": "dagayn/search.py::rrf_merge"},
            ]
        )
        rows = run(Path("/tmp"), self.store, config)
        assert len(rows) == 3  # 2 per-query + 1 aggregate

    def test_aggregate_row_marker(self):
        config = self._config(
            [
                {"query": "hybrid_search", "expected": "dagayn/search.py::hybrid_search"},
            ]
        )
        rows = run(Path("/tmp"), self.store, config)
        agg = rows[-1]
        assert agg["query"] == "__aggregate__"
        assert agg["search_mode"] == "aggregate"

    def test_aggregate_contains_metrics(self):
        config = self._config(
            [
                {"query": "hybrid_search", "expected": "dagayn/search.py::hybrid_search"},
            ]
        )
        rows = run(Path("/tmp"), self.store, config)
        agg = rows[-1]
        assert "mean_mrr" in agg
        assert "precision_at_1" in agg
        assert "precision_at_5" in agg
        assert agg["query_count"] == 1

    def test_label_is_preserved(self):
        config = self._config(
            [
                {
                    "query": "rrf_merge",
                    "expected": "dagayn/search.py::rrf_merge",
                    "label": "exact_name",
                },
            ]
        )
        rows = run(Path("/tmp"), self.store, config)
        row = next(r for r in rows if r["query"] == "rrf_merge")
        assert row["label"] == "exact_name"

    def test_missing_label_defaults_empty(self):
        config = self._config(
            [
                {"query": "rrf_merge", "expected": "dagayn/search.py::rrf_merge"},
            ]
        )
        rows = run(Path("/tmp"), self.store, config)
        row = next(r for r in rows if r["query"] == "rrf_merge")
        assert row["label"] == ""


# ---------------------------------------------------------------------------
# Ranking quality: exact name queries (FTS strength)
# ---------------------------------------------------------------------------


class TestFtsExactNameQueries(_FtsFixture):
    def _run_single(self, query: str, expected: str, label: str = "") -> dict:
        config = self._config([{"query": query, "expected": expected, "label": label}])
        rows = run(Path("/tmp"), self.store, config)
        return next(r for r in rows if r["query"] == query)

    def test_hybrid_search_ranks_first(self):
        row = self._run_single("hybrid_search", "dagayn/search.py::hybrid_search")
        assert row["rank"] == 1
        assert row["hit_at_1"] == 1
        assert row["reciprocal_rank"] == 1.0

    def test_rrf_merge_ranks_first(self):
        row = self._run_single("rrf_merge", "dagayn/search.py::rrf_merge")
        assert row["rank"] == 1

    def test_rebuild_fts_index_ranks_first(self):
        row = self._run_single("rebuild_fts_index", "dagayn/search.py::rebuild_fts_index")
        assert row["rank"] == 1

    def test_full_build_found_in_top5(self):
        row = self._run_single("full_build", "dagayn/incremental.py::full_build")
        assert row["hit_at_5"] == 1, f"rank={row['rank']} not in top 5"

    def test_pascal_class_graphstore_found(self):
        row = self._run_single("GraphStore", "dagayn/graph/core.py::GraphStore")
        assert row["rank"] > 0, "GraphStore should be found"

    def test_missing_query_records_zero(self):
        row = self._run_single("zzz_nonexistent_xyz", "nowhere.py::nonexistent")
        assert row["rank"] == 0
        assert row["reciprocal_rank"] == 0.0
        assert row["hit_at_1"] == 0
        assert row["hit_at_5"] == 0


# ---------------------------------------------------------------------------
# Aggregate metrics
# ---------------------------------------------------------------------------


class TestFtsAggregateMetrics(_FtsFixture):
    def test_mean_mrr_between_0_and_1(self):
        config = self._config(
            [
                {"query": "hybrid_search", "expected": "dagayn/search.py::hybrid_search"},
                {"query": "zzz_missing", "expected": "nowhere.py::missing"},
            ]
        )
        rows = run(Path("/tmp"), self.store, config)
        agg = rows[-1]
        assert 0.0 <= agg["mean_mrr"] <= 1.0

    def test_perfect_mean_mrr(self):
        config = self._config(
            [
                {"query": "hybrid_search", "expected": "dagayn/search.py::hybrid_search"},
                {"query": "rrf_merge", "expected": "dagayn/search.py::rrf_merge"},
                {"query": "rebuild_fts_index", "expected": "dagayn/search.py::rebuild_fts_index"},
            ]
        )
        rows = run(Path("/tmp"), self.store, config)
        agg = rows[-1]
        assert agg["mean_mrr"] == 1.0

    def test_zero_mean_mrr_when_all_miss(self):
        config = self._config(
            [
                {"query": "zzz_nothing", "expected": "nowhere.py::zzz"},
                {"query": "aaa_nothing", "expected": "nowhere.py::aaa"},
            ]
        )
        rows = run(Path("/tmp"), self.store, config)
        agg = rows[-1]
        assert agg["mean_mrr"] == 0.0
        assert agg["precision_at_1"] == 0.0
        assert agg["precision_at_5"] == 0.0

    def test_query_count_matches_input(self):
        queries = [
            {"query": "hybrid_search", "expected": "dagayn/search.py::hybrid_search"},
            {"query": "rrf_merge", "expected": "dagayn/search.py::rrf_merge"},
            {"query": "zzz_missing", "expected": "nowhere.py::missing"},
        ]
        rows = run(Path("/tmp"), self.store, self._config(queries))
        agg = rows[-1]
        assert agg["query_count"] == len(queries)

    def test_precision_at_1_correct(self):
        config = self._config(
            [
                {
                    "query": "hybrid_search",
                    "expected": "dagayn/search.py::hybrid_search",
                },  # hit@1=1
                {"query": "zzz_nothing", "expected": "nowhere.py::zzz"},  # hit@1=0
            ]
        )
        rows = run(Path("/tmp"), self.store, config)
        agg = rows[-1]
        assert agg["precision_at_1"] == 0.5


# ---------------------------------------------------------------------------
# Search mode tagging
# ---------------------------------------------------------------------------


class TestFtsSearchMode(_FtsFixture):
    def test_mode_is_fts_only_without_embeddings(self):
        config = self._config(
            [
                {"query": "rebuild_fts_index", "expected": "dagayn/search.py::rebuild_fts_index"},
            ]
        )
        rows = run(Path("/tmp"), self.store, config)
        row = next(r for r in rows if r["query"] == "rebuild_fts_index")
        assert row["search_mode"] in ("fts_only", "keyword_fallback", "hybrid")
