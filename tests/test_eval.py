"""Tests for the evaluation framework (scorer, reporter, runner, benchmarks)."""

import csv
import os
import subprocess
import tempfile
from pathlib import Path

import pytest

from dagayn.eval.reporter import (
    generate_full_report,
    generate_markdown_report,
    generate_readme_tables,
)

try:
    import yaml as _yaml  # noqa: F401

    from dagayn.eval.runner import write_csv

    _HAS_YAML = True
except ImportError:
    _HAS_YAML = False
    write_csv = None  # type: ignore[assignment]
from dagayn.eval.scorer import (
    compute_mrr,
    compute_precision_at_k,
    compute_precision_recall,
    compute_token_efficiency,
)

# --- Existing scorer tests ---


def test_token_efficiency():
    result = compute_token_efficiency(10000, 3000)
    assert result["raw_tokens"] == 10000
    assert result["graph_tokens"] == 3000
    assert result["ratio"] == 0.3
    assert result["reduction_percent"] == 70.0


def test_token_efficiency_zero_raw():
    result = compute_token_efficiency(0, 100)
    assert result["ratio"] == 0.0
    assert result["reduction_percent"] == 0.0


def test_mrr_found_at_rank_2():
    result = compute_mrr("b", ["a", "b", "c"])
    assert result == 0.5


def test_mrr_found_at_rank_1():
    result = compute_mrr("a", ["a", "b", "c"])
    assert result == 1.0


def test_mrr_not_found():
    result = compute_mrr("z", ["a", "b", "c"])
    assert result == 0.0


def test_precision_recall():
    predicted = {"a", "b", "c", "d"}
    actual = {"b", "c", "e"}
    result = compute_precision_recall(predicted, actual)
    assert result["precision"] == 0.5
    assert result["recall"] == round(2 / 3, 4)
    expected_f1 = round(2 * 0.5 * (2 / 3) / (0.5 + 2 / 3), 4)
    assert result["f1"] == expected_f1


def test_precision_recall_empty_sets():
    result = compute_precision_recall(set(), set())
    assert result["status"] == "skipped"
    assert result["precision"] is None
    assert result["recall"] is None
    assert result["f1"] is None


def test_precision_recall_no_overlap():
    result = compute_precision_recall({"a"}, {"b"})
    assert result["precision"] == 0.0
    assert result["recall"] == 0.0
    assert result["f1"] == 0.0


def test_precision_at_k():
    result = compute_precision_at_k(["a", "b", "c"], {"b", "z"}, k=2)
    assert result["precision_at_k"] == 0.5
    assert result["hits"] == 1
    assert result["k"] == 2


def test_register_command_lists_guidance_precision():
    import argparse

    from dagayn.cli.commands.eval_cmd import register_command

    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command")
    eval_parser = register_command(sub)

    help_text = eval_parser.format_help()
    assert "guidance_precision" in help_text


def test_generate_markdown_report():
    results = [
        {
            "benchmark": "token_efficiency",
            "ratio": 0.3,
            "reduction_percent": 70.0,
        },
        {
            "benchmark": "search_mrr",
            "ratio": "-",
            "reduction_percent": "-",
        },
    ]
    report = generate_markdown_report(results)
    assert "# Evaluation Report" in report
    assert "## Summary" in report
    assert "token_efficiency" in report
    assert "search_mrr" in report
    assert "70.0" in report
    assert "| Benchmark |" in report


def test_generate_markdown_report_empty():
    report = generate_markdown_report([])
    assert "No benchmark results" in report


# --- New tests ---


@pytest.mark.skipif(not _HAS_YAML, reason="pyyaml not installed")
def test_load_config():
    """Load a temp YAML config and verify structure."""
    import yaml

    with tempfile.NamedTemporaryFile(mode="w", suffix=".yaml", delete=False) as f:
        yaml.dump(
            {
                "name": "test-repo",
                "url": "https://example.com/repo.git",
                "commit": "HEAD",
                "language": "python",
                "size_category": "small",
                "test_commits": [{"sha": "abc123", "description": "test"}],
                "entry_points": ["main.py::main"],
                "search_queries": [{"query": "hello", "expected": "main.py::greet"}],
            },
            f,
        )
        tmp_path = f.name

    try:
        import yaml as _yaml

        with open(tmp_path) as fh:
            config = _yaml.safe_load(fh)

        assert config["name"] == "test-repo"
        assert config["language"] == "python"
        assert len(config["test_commits"]) == 1
        assert len(config["entry_points"]) == 1
        assert len(config["search_queries"]) == 1
    finally:
        os.unlink(tmp_path)


@pytest.mark.skipif(not _HAS_YAML, reason="pyyaml not installed")
def test_write_csv():
    """Write results to CSV and read back."""
    with tempfile.TemporaryDirectory() as tmpdir:
        path = Path(tmpdir) / "results" / "test.csv"
        results = [
            {"repo": "foo", "tokens": 100, "ratio": 2.5},
            {"repo": "bar", "tokens": 200, "ratio": 1.5},
        ]
        write_csv(results, path)

        assert path.exists()
        with open(path, newline="") as f:
            reader = csv.DictReader(f)
            rows = list(reader)

        assert len(rows) == 2
        assert rows[0]["repo"] == "foo"
        assert rows[1]["tokens"] == "200"


@pytest.mark.skipif(not _HAS_YAML, reason="pyyaml not installed")
def test_write_csv_empty():
    """Writing empty results should be a no-op."""
    with tempfile.TemporaryDirectory() as tmpdir:
        path = Path(tmpdir) / "empty.csv"
        write_csv([], path)
        assert not path.exists()


def test_generate_readme_tables():
    """Feed sample CSV data and verify table format."""
    with tempfile.TemporaryDirectory() as tmpdir:
        results_dir = Path(tmpdir)

        # Write token efficiency CSV
        te_path = results_dir / "test_token_efficiency_2026-01-01.csv"
        with open(te_path, "w", newline="") as f:
            w = csv.DictWriter(
                f,
                fieldnames=[
                    "repo",
                    "commit",
                    "description",
                    "changed_files",
                    "naive_tokens",
                    "standard_tokens",
                    "graph_tokens",
                    "naive_to_graph_ratio",
                    "standard_to_graph_ratio",
                ],
            )
            w.writeheader()
            w.writerow(
                {
                    "repo": "myrepo",
                    "commit": "abc",
                    "description": "test",
                    "changed_files": "3",
                    "naive_tokens": "1000",
                    "standard_tokens": "500",
                    "graph_tokens": "200",
                    "naive_to_graph_ratio": "5.0",
                    "standard_to_graph_ratio": "2.5",
                }
            )

        tables = generate_readme_tables(results_dir)
        assert "### Token Efficiency" in tables
        assert "myrepo" in tables
        assert "1000" in tables


def test_generate_full_report():
    """Feed sample CSV data and verify report sections."""
    with tempfile.TemporaryDirectory() as tmpdir:
        results_dir = Path(tmpdir)

        # Write a build_performance CSV
        bp_path = results_dir / "test_build_performance_2026-01-01.csv"
        with open(bp_path, "w", newline="") as f:
            w = csv.DictWriter(
                f,
                fieldnames=[
                    "repo",
                    "file_count",
                    "node_count",
                    "edge_count",
                    "flow_detection_seconds",
                    "community_detection_seconds",
                    "search_avg_ms",
                    "nodes_per_second",
                ],
            )
            w.writeheader()
            w.writerow(
                {
                    "repo": "testrepo",
                    "file_count": "10",
                    "node_count": "50",
                    "edge_count": "30",
                    "flow_detection_seconds": "0.1",
                    "community_detection_seconds": "0.2",
                    "search_avg_ms": "5.0",
                    "nodes_per_second": "500",
                }
            )

        report = generate_full_report(results_dir)
        assert "# Evaluation Report" in report
        assert "## Methodology" in report
        assert "## Build Performance" in report
        assert "testrepo" in report


def test_generate_full_report_includes_registered_benchmarks(tmp_path):
    for benchmark in [
        "guidance_precision",
        "fts_quality",
        "nplusone_count",
        "mcp_latency",
        "recent_changes_effects",
    ]:
        path = tmp_path / f"repo_{benchmark}_2026-01-01.csv"
        with open(path, "w", newline="") as f:
            w = csv.DictWriter(f, fieldnames=["benchmark", "repo", "status"])
            w.writeheader()
            w.writerow({"benchmark": benchmark, "repo": "repo", "status": "ok"})

    report = generate_full_report(tmp_path)
    assert "## Guidance Precision" in report
    assert "## Fts Quality" in report
    assert "## Nplusone Count" in report
    assert "## Mcp Latency" in report
    assert "## Recent Changes Effects" in report


def test_markdown_table_escapes_special_cells():
    report = generate_markdown_report(
        [{"benchmark": "search_quality", "query": "a|b", "error": "bad\n`cell`"}]
    )
    assert "a\\|b" in report
    assert "bad<br>\\`cell\\`" in report


@pytest.mark.skipif(not _HAS_YAML, reason="pyyaml not installed")
def test_runner_with_mock_repo():
    """Create a tiny git repo with 2 Python files, run benchmarks, verify output."""
    with tempfile.TemporaryDirectory() as tmpdir:
        repo_path = Path(tmpdir) / "mock_repo"
        repo_path.mkdir()

        # Init git repo
        subprocess.run(["git", "init"], cwd=str(repo_path), capture_output=True)
        subprocess.run(
            ["git", "config", "user.email", "test@test.com"],
            cwd=str(repo_path),
            capture_output=True,
        )
        subprocess.run(
            ["git", "config", "user.name", "Test"],
            cwd=str(repo_path),
            capture_output=True,
        )

        # Create two Python files
        (repo_path / "main.py").write_text(
            'from helper import greet\n\ndef main():\n    greet("world")\n',
            encoding="utf-8",
        )
        (repo_path / "helper.py").write_text(
            'def greet(name):\n    print(f"Hello {name}")\n',
            encoding="utf-8",
        )

        subprocess.run(["git", "add", "."], cwd=str(repo_path), capture_output=True)
        subprocess.run(
            ["git", "commit", "-m", "initial"],
            cwd=str(repo_path),
            capture_output=True,
        )

        # Second commit: modify helper.py
        (repo_path / "helper.py").write_text(
            'def greet(name):\n    print(f"Hi {name}!")\n',
            encoding="utf-8",
        )
        subprocess.run(["git", "add", "."], cwd=str(repo_path), capture_output=True)
        subprocess.run(
            ["git", "commit", "-m", "update greeting"],
            cwd=str(repo_path),
            capture_output=True,
        )

        # Build graph
        from dagayn.graph import GraphStore
        from dagayn.incremental import full_build, get_db_path

        db_path = get_db_path(repo_path)
        store = GraphStore(db_path)
        full_build(repo_path, store)

        config = {
            "name": "mock",
            "language": "python",
            "test_commits": [
                {"sha": "HEAD", "description": "update greeting"},
            ],
            "entry_points": ["main.py::main"],
            "search_queries": [
                {"query": "greet", "expected": "helper.py::greet"},
            ],
        }

        # Run token_efficiency
        from dagayn.eval.benchmarks import token_efficiency

        te_results = token_efficiency.run(repo_path, store, config)
        assert len(te_results) >= 1
        assert "naive_tokens" in te_results[0]
        assert "graph_tokens" in te_results[0]

        # Run impact_accuracy
        from dagayn.eval.benchmarks import impact_accuracy

        ia_results = impact_accuracy.run(repo_path, store, config)
        assert len(ia_results) >= 1
        assert ia_results[0]["status"] == "proxy"
        assert "graph_proxy_precision" in ia_results[0]
        assert "graph_proxy_f1" in ia_results[0]

        # Run search_quality
        from dagayn.eval.benchmarks import search_quality

        sq_results = search_quality.run(repo_path, store, config)
        assert len(sq_results) == 1
        assert "reciprocal_rank" in sq_results[0]
        assert "hit_at_5" in sq_results[0]
        assert "ndcg_at_20" in sq_results[0]
        assert "search_mode" in sq_results[0]
        assert "embedding_status" in sq_results[0]
        assert "latency_ms" in sq_results[0]

        # Run build_performance
        from dagayn.eval.benchmarks import build_performance

        bp_results = build_performance.run(repo_path, store, config)
        assert len(bp_results) == 1
        assert "node_count" in bp_results[0]
        assert bp_results[0]["node_count"] > 0

        # Run mcp_latency as a local baseline generator
        from dagayn.eval.benchmarks import mcp_latency

        latency_results = mcp_latency.run(
            repo_path,
            store,
            {**config, "latency_repeat": 1},
        )
        assert len(latency_results) >= 1
        assert all(row["benchmark"] == "mcp_latency" for row in latency_results)
        assert all(row["status"] in {"baseline", "error"} for row in latency_results)

        # Run recent-change effect measurements with small local inputs.
        from dagayn.eval.benchmarks import recent_changes_effects

        effect_results = recent_changes_effects.run(
            repo_path,
            store,
            {
                **config,
                "latency_repeat": 1,
                "effect_repeat": 1,
                "centrality_repeat": 1,
                "effect_dfs_depth": 2,
                "effect_remove_files": 5,
                "effect_store_files": 5,
            },
        )
        scenarios = {row["scenario"] for row in effect_results}
        assert "parse_diff_ranges_cache" in scenarios
        assert "bridge_centrality_persisted_read" in scenarios
        assert "dfs_lazy_fetch" in scenarios
        assert "remove_files_data_batch" in scenarios
        assert "store_file_batch_bulk_replace" in scenarios
        assert any(scenario.startswith("mcp_latency:") for scenario in scenarios)
        assert all(row["benchmark"] == "recent_changes_effects" for row in effect_results)

        from dagayn.eval.benchmarks import guidance_precision

        guidance_results = guidance_precision.run(repo_path, store, config)
        assert guidance_results[0]["benchmark"] == "guidance_precision"
        assert guidance_results[0]["status"] == "skipped"

        store.close()


@pytest.mark.skipif(not _HAS_YAML, reason="pyyaml not installed")
def test_write_csv_heterogeneous_rows_preserves_all_columns(tmp_path):
    path = tmp_path / "results.csv"
    write_csv(
        [
            {"benchmark": "a", "repo": "r", "status": "ok", "only_a": 1},
            {"benchmark": "b", "repo": "r", "status": "error", "error": "bad", "only_b": 2},
        ],
        path,
    )
    with open(path, newline="") as f:
        reader = csv.DictReader(f)
        rows = list(reader)
    assert reader.fieldnames[:4] == ["benchmark", "repo", "status", "error"]
    assert "only_a" in reader.fieldnames
    assert "only_b" in reader.fieldnames
    assert rows[0]["only_b"] == ""
    assert rows[1]["only_a"] == ""


def test_token_efficiency_context_failure_is_status_error(tmp_path, monkeypatch):
    repo_path = tmp_path / "repo"
    repo_path.mkdir()
    subprocess.run(["git", "init"], cwd=repo_path, check=True, capture_output=True)
    subprocess.run(["git", "config", "user.email", "test@test.com"], cwd=repo_path, check=True)
    subprocess.run(["git", "config", "user.name", "Test"], cwd=repo_path, check=True)
    (repo_path / "a.py").write_text("def a():\n    return 1\n", encoding="utf-8")
    subprocess.run(["git", "add", "."], cwd=repo_path, check=True)
    subprocess.run(
        ["git", "commit", "-m", "initial"],
        cwd=repo_path,
        check=True,
        capture_output=True,
    )
    (repo_path / "a.py").write_text("def a():\n    return 2\n", encoding="utf-8")
    subprocess.run(
        ["git", "commit", "-am", "change"],
        cwd=repo_path,
        check=True,
        capture_output=True,
    )

    import dagayn.tools
    from dagayn.eval.benchmarks import token_efficiency

    def fail_context(**_kwargs):
        raise RuntimeError("boom")

    monkeypatch.setattr(dagayn.tools, "get_review_context", fail_context)
    rows = token_efficiency.run(
        repo_path,
        None,
        {"name": "repo", "test_commits": [{"sha": "HEAD"}]},
    )
    assert rows[0]["status"] == "error"
    assert "graph_context_tokens" not in rows[0]
    assert "changed_file_to_graph_ratio" not in rows[0]


def test_identifier_matcher_exact_alias_and_basename_opt_in():
    from dagayn.eval.scorer import IdentifierMatcher

    exact = IdentifierMatcher()
    assert exact.matches("pkg/a.py::Service.run", "pkg/a.py::Service.run")
    assert not exact.matches("pkg/a.py::Service.run", "other.py::Service.run")

    alias = IdentifierMatcher({"pkg/a.py::Service.run": {"Service.run"}})
    assert alias.matches("Service.run", "pkg/a.py::Service.run")

    basename = IdentifierMatcher(allow_basename=True)
    assert basename.matches("pkg/a.py::Service.run", "other.py::Service.run")


def test_guidance_precision_no_cases_skipped():
    from dagayn.eval.benchmarks import guidance_precision

    rows = guidance_precision.run(Path("/tmp"), None, {})
    assert rows == [
        {
            "benchmark": "guidance_precision",
            "case": "no_cases",
            "kind": "none",
            "status": "skipped",
        }
    ]


def test_build_performance_times_full_build(monkeypatch, tmp_path):
    from dagayn.eval.benchmarks import build_performance

    class FakeStats:
        files_count = 2
        total_nodes = 10
        total_edges = 3

    class FakeStore:
        def __init__(self, _path):
            pass

        def get_stats(self):
            return FakeStats()

        def close(self):
            pass

    calls = []

    def fake_full_build(_repo_path, _store):
        calls.append(1)
        return {"files_parsed": 2, "errors": []}

    monkeypatch.setattr("dagayn.graph.GraphStore", FakeStore)
    monkeypatch.setattr("dagayn.incremental.full_build", fake_full_build)
    rows = build_performance.run(tmp_path, None, {"name": "repo"})
    assert calls == [1]
    assert rows[0]["status"] == "ok"
    assert rows[0]["build_total_ms"] >= 0
    assert "flow_detection_seconds" not in rows[0]


def test_embedding_text_modes_benchmark_compares_body_mode(tmp_path):
    from dagayn.eval.benchmarks import embedding_text_modes
    from dagayn.graph import GraphStore
    from dagayn.parser import NodeInfo

    repo_path = tmp_path / "repo"
    repo_path.mkdir()
    (repo_path / "service.py").write_text(
        "def handle_failure():\n"
        "    retry_budget_exhausted = True\n"
        "    return retry_budget_exhausted\n",
        encoding="utf-8",
    )
    (repo_path / "other.py").write_text("def helper():\n    return True\n", encoding="utf-8")
    (repo_path / ".dagayn").mkdir()

    store = GraphStore(repo_path / ".dagayn" / "graph.db")
    store.set_metadata("repo_root", str(repo_path))
    store.upsert_node(
        NodeInfo(
            kind="Function",
            name="handle_failure",
            file_path="service.py",
            line_start=1,
            line_end=3,
            language="python",
        )
    )
    store.upsert_node(
        NodeInfo(
            kind="Function",
            name="helper",
            file_path="other.py",
            line_start=1,
            line_end=2,
            language="python",
        )
    )
    store.commit()

    rows = embedding_text_modes.run(
        repo_path,
        store,
        {
            "name": "text_mode_fixture",
            "embedding_text_mode_queries": [
                {
                    "query": "retry budget exhausted",
                    "expected": "service.py::handle_failure",
                    "label": "source_body",
                }
            ],
        },
    )
    by_mode = {row["text_mode"]: row for row in rows}

    assert set(by_mode) == {"metadata", "body", "structured", "narrative"}
    assert by_mode["body"]["hit_at_5"] == 1
    assert by_mode["structured"]["hit_at_5"] == 1
    assert by_mode["narrative"]["hit_at_5"] == 1
    assert by_mode["body"]["reciprocal_rank"] >= by_mode["metadata"]["reciprocal_rank"]
    assert by_mode["structured"]["reciprocal_rank"] >= by_mode["metadata"]["reciprocal_rank"]
    assert by_mode["narrative"]["reciprocal_rank"] >= by_mode["metadata"]["reciprocal_rank"]
    store.close()


def test_embedding_materials_benchmark_reports_negative_scores(tmp_path):
    from dagayn.eval.benchmarks import embedding_materials
    from dagayn.graph import GraphStore
    from dagayn.parser import NodeInfo

    repo_path = tmp_path / "repo"
    repo_path.mkdir()
    (repo_path / "service.py").write_text(
        "# Retry transient failures with a bounded budget.\n"
        "def handle_failure(retry_budget):\n"
        "    return retry_budget > 0\n",
        encoding="utf-8",
    )
    (repo_path / ".dagayn").mkdir()

    store = GraphStore(repo_path / ".dagayn" / "graph.db")
    store.set_metadata("repo_root", str(repo_path))
    store.upsert_node(
        NodeInfo(
            kind="Function",
            name="handle_failure",
            file_path="service.py",
            line_start=2,
            line_end=3,
            language="python",
            params="(retry_budget)",
        )
    )
    store.commit()

    rows = embedding_materials.run(
        repo_path,
        store,
        {
            "name": "material_fixture",
            "search_queries": [
                {
                    "query": "retry budget",
                    "expected": "service.py::handle_failure",
                    "label": "positive_fixture",
                }
            ],
            "embedding_material_negative_queries": [
                {
                    "query": "watercolor paper texture",
                    "label": "negative_fixture",
                }
            ],
            "embedding_material_strategies": [
                "doc=section|code=predicate|comment=sentence|join=split"
            ],
        },
    )

    negative_rows = [row for row in rows if row["query_type"] == "negative"]
    aggregate_negative = [
        row
        for row in rows
        if row["query"] == "__aggregate__" and row["label"] == "aggregate_negative"
    ]

    assert negative_rows
    assert aggregate_negative
    assert "top_score" in negative_rows[0]
    assert "mean_top_score" in aggregate_negative[0]
    store.close()


# --- Token benchmark tests ---


def test_estimate_tokens_basic():
    """estimate_tokens should return a reasonable approximation."""
    from dagayn.eval.token_benchmark import estimate_tokens

    # Simple string: "hello" => JSON '"hello"' (7 chars) => 7 // 4 = 1
    assert estimate_tokens("hello") == 1

    # Dict: {"a": 1} => '{"a": 1}' (8 chars) => 8 // 4 = 2
    assert estimate_tokens({"a": 1}) == 2

    # Longer content should scale proportionally
    long_text = "x" * 400
    tokens = estimate_tokens(long_text)
    # JSON adds 2 quote chars: (400 + 2) // 4 = 100
    assert tokens == 100


def test_estimate_tokens_nested():
    """estimate_tokens handles nested structures."""
    from dagayn.eval.token_benchmark import estimate_tokens

    nested = {"nodes": [{"name": "foo"}, {"name": "bar"}], "count": 2}
    tokens = estimate_tokens(nested)
    assert tokens > 0
    assert isinstance(tokens, int)


def test_estimate_tokens_non_serializable():
    """estimate_tokens uses default=str for non-serializable objects."""
    from pathlib import Path

    from dagayn.eval.token_benchmark import estimate_tokens

    # Path objects are not JSON-serializable but default=str handles them
    tokens = estimate_tokens({"path": Path("/tmp/test")})
    assert tokens > 0


def test_benchmark_review_workflow():
    """benchmark_review_workflow completes and returns expected structure."""
    from dagayn.eval.token_benchmark import benchmark_review_workflow

    with tempfile.TemporaryDirectory() as tmpdir:
        repo_path = Path(tmpdir) / "bench_repo"
        repo_path.mkdir()

        # Init git repo with two commits
        subprocess.run(
            ["git", "init"],
            cwd=str(repo_path),
            capture_output=True,
        )
        subprocess.run(
            ["git", "config", "user.email", "test@test.com"],
            cwd=str(repo_path),
            capture_output=True,
        )
        subprocess.run(
            ["git", "config", "user.name", "Test"],
            cwd=str(repo_path),
            capture_output=True,
        )

        (repo_path / "main.py").write_text(
            'from helper import greet\n\ndef main():\n    greet("world")\n',
            encoding="utf-8",
        )
        (repo_path / "helper.py").write_text(
            'def greet(name):\n    print(f"Hello {name}")\n',
            encoding="utf-8",
        )

        subprocess.run(
            ["git", "add", "."],
            cwd=str(repo_path),
            capture_output=True,
        )
        subprocess.run(
            ["git", "commit", "-m", "initial"],
            cwd=str(repo_path),
            capture_output=True,
        )

        # Second commit
        (repo_path / "helper.py").write_text(
            'def greet(name):\n    print(f"Hi {name}!")\n',
            encoding="utf-8",
        )
        subprocess.run(
            ["git", "add", "."],
            cwd=str(repo_path),
            capture_output=True,
        )
        subprocess.run(
            ["git", "commit", "-m", "update greeting"],
            cwd=str(repo_path),
            capture_output=True,
        )

        # Build graph
        from dagayn.graph import GraphStore
        from dagayn.incremental import full_build, get_db_path

        db_path = get_db_path(repo_path)
        store = GraphStore(db_path)
        full_build(repo_path, store)
        store.close()

        # Run the review benchmark
        result = benchmark_review_workflow(
            repo_root=str(repo_path),
            base="HEAD~1",
        )

        assert result["workflow"] == "review"
        assert result["total_tokens"] > 0
        assert result["tool_calls"] == 2
        assert len(result["calls"]) == 2
        assert result["calls"][0]["tool"] == "get_minimal_context"
        assert result["calls"][1]["tool"] == "detect_changes_minimal"
        for call in result["calls"]:
            assert call["tokens"] >= 0


def test_run_all_benchmarks():
    """run_all_benchmarks returns results for all workflows."""
    from dagayn.eval.token_benchmark import run_all_benchmarks

    with tempfile.TemporaryDirectory() as tmpdir:
        repo_path = Path(tmpdir) / "all_bench_repo"
        repo_path.mkdir()

        subprocess.run(
            ["git", "init"],
            cwd=str(repo_path),
            capture_output=True,
        )
        subprocess.run(
            ["git", "config", "user.email", "test@test.com"],
            cwd=str(repo_path),
            capture_output=True,
        )
        subprocess.run(
            ["git", "config", "user.name", "Test"],
            cwd=str(repo_path),
            capture_output=True,
        )

        (repo_path / "app.py").write_text(
            'def main():\n    print("hello")\n',
            encoding="utf-8",
        )

        subprocess.run(
            ["git", "add", "."],
            cwd=str(repo_path),
            capture_output=True,
        )
        subprocess.run(
            ["git", "commit", "-m", "initial"],
            cwd=str(repo_path),
            capture_output=True,
        )

        (repo_path / "app.py").write_text(
            'def main():\n    print("hi")\n',
            encoding="utf-8",
        )
        subprocess.run(
            ["git", "add", "."],
            cwd=str(repo_path),
            capture_output=True,
        )
        subprocess.run(
            ["git", "commit", "-m", "update"],
            cwd=str(repo_path),
            capture_output=True,
        )

        from dagayn.graph import GraphStore
        from dagayn.incremental import full_build, get_db_path

        db_path = get_db_path(repo_path)
        store = GraphStore(db_path)
        full_build(repo_path, store)
        store.close()

        results = run_all_benchmarks(repo_root=str(repo_path), base="HEAD~1")

        # Should have one result per workflow (5 total)
        assert len(results) == 5

        workflow_names = {r["workflow"] for r in results}
        assert workflow_names == {
            "review",
            "architecture",
            "debug",
            "onboard",
            "pre_merge",
        }

        # Each successful result should have total_tokens
        for r in results:
            if "error" not in r:
                assert r["total_tokens"] >= 0
                assert "calls" in r
