"""Tests for semantic evaluation metadata and profile aggregation."""

from __future__ import annotations

import csv
import json

from dagayn.eval.aggregate import (
    normalize_higher_better,
    normalize_lower_better,
    summarize_profile,
)
from dagayn.eval.reporter import generate_full_report
from dagayn.eval.semantics import decorate_metric_row, get_metric_spec


def test_metric_spec_registry_core_metrics():
    search = get_metric_spec("search_quality", "reciprocal_rank")
    assert search is not None
    assert search.family == "retrieval"
    assert search.role == "score"
    assert search.oracle_type == "explicit"
    assert search.construct == "locator_quality"
    assert search.valid_for_headline is True

    proxy = get_metric_spec("impact_accuracy", "graph_proxy_recall")
    assert proxy is not None
    assert proxy.role == "diagnostic"
    assert proxy.oracle_type == "proxy"
    assert proxy.valid_for_headline is False

    coverage = get_metric_spec("guidance_precision", "field_coverage")
    assert coverage is not None
    assert coverage.family == "guidance"
    assert coverage.construct == "schema_completeness"

    token = get_metric_spec("token_efficiency", "diff_to_graph_ratio")
    assert token is not None
    assert token.family == "efficiency"
    assert token.role == "cost"
    assert token.valid_for_headline is False

    build = get_metric_spec("build_performance", "build_total_ms")
    assert build is not None
    assert build.family == "efficiency"
    assert build.role == "cost"


def test_decorated_rows_preserve_original_values_and_mark_single_metric():
    row = {
        "benchmark": "token_efficiency",
        "repo": "dagayn",
        "diff_to_graph_ratio": 2.5,
    }
    decorated = decorate_metric_row(row)
    assert decorated["diff_to_graph_ratio"] == 2.5
    assert decorated["metric_family"] == "efficiency"
    assert decorated["metric_role"] == "cost"
    assert decorated["valid_for_headline"] is False
    assert decorated["higher_is_better"] is False
    assert row.keys() == {"benchmark", "repo", "diff_to_graph_ratio"}


def test_search_quality_multi_metric_row_gets_semantic_notes():
    decorated = decorate_metric_row(
        {
            "benchmark": "search_quality",
            "reciprocal_rank": 1.0,
            "hit_at_5": 1,
            "latency_ms": 12.0,
        }
    )
    assert "multi_metric_row" in decorated["semantic_notes"]
    assert "reciprocal_rank" in decorated["headline_metrics"]
    assert "latency_ms" in decorated["diagnostic_metrics"]


def test_proxy_and_synthetic_metrics_not_headline_valid():
    proxy = decorate_metric_row(
        {
            "benchmark": "impact_accuracy",
            "status": "proxy",
            "graph_proxy_recall": 0.8,
        }
    )
    assert proxy["oracle_type"] == "proxy"
    assert proxy["metric_role"] == "diagnostic"
    assert proxy["valid_for_headline"] is False

    synthetic = decorate_metric_row(
        {
            "benchmark": "embedding_materials",
            "mean_mrr": 0.9,
        }
    )
    assert synthetic["oracle_type"] == "synthetic"
    assert synthetic["valid_for_headline"] is False


def test_guidance_and_build_rows_get_expected_semantics():
    guidance = decorate_metric_row(
        {
            "benchmark": "guidance_precision",
            "precision_at_k": 0.75,
        }
    )
    assert guidance["metric_family"] == "guidance"
    assert guidance["metric_role"] == "score"
    assert guidance["valid_for_headline"] is True

    build = decorate_metric_row(
        {
            "benchmark": "build_performance",
            "build_total_ms": 1200,
        }
    )
    assert build["metric_family"] == "efficiency"
    assert build["metric_role"] == "cost"


def test_normalization_helpers_clamp_and_handle_missing():
    assert normalize_higher_better(0.5, target=1.0) == 0.5
    assert normalize_higher_better(2.0, target=1.0) == 1.0
    assert normalize_higher_better(None, target=1.0) is None
    assert normalize_lower_better(100, budget=100) == 1.0
    assert normalize_lower_better(400, budget=100) == 0.0
    assert normalize_lower_better(None, budget=100) is None


def test_search_profile_computes_score_from_explicit_rows():
    summary = summarize_profile(
        [
            {
                "benchmark": "search_quality",
                "status": "ok",
                "reciprocal_rank": 1.0,
                "hit_at_5": 1,
                "ndcg_at_20": 0.5,
            }
        ],
        "search",
    )
    assert summary.status == "ok"
    assert summary.capability_score == 0.9
    assert summary.efficiency_score is None


def test_review_profile_excludes_graph_proxy_metrics():
    summary = summarize_profile(
        [
            {
                "benchmark": "impact_accuracy",
                "status": "proxy",
                "graph_proxy_recall": 1.0,
                "graph_proxy_f1": 1.0,
            }
        ],
        "review",
    )
    assert summary.status == "insufficient_oracle"
    assert summary.capability_score is None
    assert any("Proxy impact metrics" in note for note in summary.notes)


def test_review_profile_scores_explicit_metrics_and_ignores_proxy():
    summary = summarize_profile(
        [
            {
                "benchmark": "impact_accuracy",
                "status": "ok",
                "recall": 0.8,
                "f1": 0.7,
            },
            {
                "benchmark": "impact_accuracy",
                "status": "proxy",
                "graph_proxy_recall": 0.1,
                "graph_proxy_f1": 0.1,
            },
        ],
        "review",
    )
    assert summary.status == "ok"
    assert summary.components["impact_recall"] == 0.8
    assert summary.capability_score == 0.7357


def test_operability_profile_computes_efficiency_not_capability():
    summary = summarize_profile(
        [
            {
                "benchmark": "build_performance",
                "status": "ok",
                "build_total_ms": 1200,
            },
            {
                "benchmark": "mcp_latency",
                "status": "baseline",
                "median_ms": 25,
            },
        ],
        "operability",
    )
    assert summary.status == "ok"
    assert summary.capability_score is None
    assert summary.efficiency_score == 1.0


def test_error_rows_create_notes_and_do_not_enter_averages():
    summary = summarize_profile(
        [
            {
                "benchmark": "search_quality",
                "status": "error",
                "error": "boom",
                "reciprocal_rank": 1.0,
            }
        ],
        "search",
    )
    assert summary.status == "insufficient_oracle"
    assert summary.capability_score is None
    assert any("boom" in note for note in summary.notes)
    assert summary.gates["search_quality"] == "error"


def test_reporter_includes_semantic_sections_and_writes_json(tmp_path):
    results_dir = tmp_path / "results"
    reports_dir = tmp_path / "reports"
    results_dir.mkdir()
    with open(results_dir / "repo_search_quality_2026-01-01.csv", "w", newline="") as f:
        writer = csv.DictWriter(
            f,
            fieldnames=[
                "benchmark",
                "repo",
                "status",
                "reciprocal_rank",
                "hit_at_5",
                "ndcg_at_20",
            ],
        )
        writer.writeheader()
        writer.writerow(
            {
                "benchmark": "search_quality",
                "repo": "repo",
                "status": "ok",
                "reciprocal_rank": "1.0",
                "hit_at_5": "1",
                "ndcg_at_20": "1.0",
            }
        )

    report = generate_full_report(results_dir, reports_dir=reports_dir)
    assert "## Evaluation Semantics" in report
    assert "## Profile Summary" in report
    assert "## Metric Semantics" in report
    assert (reports_dir / "profile_summary.json").exists()
    assert (reports_dir / "metric_semantics.json").exists()

    profile_data = json.loads((reports_dir / "profile_summary.json").read_text())
    metric_data = json.loads((reports_dir / "metric_semantics.json").read_text())
    assert any(row["profile"] == "search" for row in profile_data)
    assert any(
        row["benchmark"] == "search_quality" and row["metric"] == "reciprocal_rank"
        for row in metric_data
    )
