"""Tests for dagayn/analysis.py: hub detection, bridge nodes, knowledge gaps, etc."""

from __future__ import annotations

import json
from typing import Any, cast

import pytest

from dagayn.graph import GraphStore
from dagayn.parser import EdgeInfo, NodeInfo


@pytest.fixture
def store(tmp_path):
    """Graph with a mix of hubs, bridges, isolated nodes, and cross-community edges."""
    db_path = tmp_path / "analysis.db"
    s = GraphStore(db_path)

    def _node(kind, name, file_path, is_test=False):
        return NodeInfo(
            kind=kind,
            name=name,
            file_path=file_path,
            line_start=1,
            line_end=10,
            language="python",
            parent_name=None,
            params=None,
            return_type=None,
            modifiers=None,
            is_test=is_test,
            extra={},
        )

    def _edge(kind, source, target):
        return EdgeInfo(
            kind=kind, source=source, target=target, file_path="src/core.py", line=1, extra={}
        )

    # Hub node: core_service is called by many
    nodes = [
        _node("File", "core.py", "src/core.py"),
        _node("Class", "CoreService", "src/core.py"),
        _node("Function", "process", "src/core.py"),
        _node("Function", "helper_a", "src/core.py"),
        _node("Function", "helper_b", "src/core.py"),
        _node("Function", "helper_c", "src/core.py"),
        _node("File", "util.py", "src/util.py"),
        _node("Function", "format_data", "src/util.py"),
        _node("File", "isolated.py", "src/isolated.py"),
        _node("Function", "orphan_fn", "src/isolated.py"),
        _node("File", "test_core.py", "tests/test_core.py"),
        _node("Test", "test_process", "tests/test_core.py", is_test=True),
    ]
    for n in nodes:
        s.upsert_node(n)

    edges = [
        # Many callers → process is a hub
        _edge("CALLS", "src/core.py::helper_a", "src/core.py::process"),
        _edge("CALLS", "src/core.py::helper_b", "src/core.py::process"),
        _edge("CALLS", "src/core.py::helper_c", "src/core.py::process"),
        _edge("CALLS", "src/util.py::format_data", "src/core.py::process"),
        _edge("CALLS", "src/core.py::process", "src/util.py::format_data"),
        _edge("TESTED_BY", "src/core.py::process", "tests/test_core.py::test_process"),
        _edge("CONTAINS", "src/core.py", "src/core.py::CoreService"),
        _edge("CONTAINS", "src/core.py", "src/core.py::process"),
    ]
    for e in edges:
        s.upsert_edge(e)

    s.commit()
    return s


@pytest.fixture
def empty_store(tmp_path):
    s = GraphStore(tmp_path / "empty.db")
    s.commit()
    return s


class TestFindHubNodes:
    def test_returns_list(self, store):
        from dagayn.analysis import find_hub_nodes

        result = find_hub_nodes(store)
        assert isinstance(result, list)

    def test_sorted_by_degree_descending(self, store):
        from dagayn.analysis import find_hub_nodes

        result = find_hub_nodes(store)
        degrees = [r["total_degree"] for r in result]
        assert degrees == sorted(degrees, reverse=True)

    def test_top_n_respected(self, store):
        from dagayn.analysis import find_hub_nodes

        result = find_hub_nodes(store, top_n=2)
        assert len(result) <= 2

    def test_result_fields(self, store):
        from dagayn.analysis import find_hub_nodes

        result = find_hub_nodes(store)
        for item in result:
            assert "name" in item
            assert "qualified_name" in item
            assert "kind" in item
            assert "total_degree" in item
            assert item["total_degree"] > 0

    def test_no_zero_degree_nodes(self, store):
        from dagayn.analysis import find_hub_nodes

        result = find_hub_nodes(store)
        assert all(r["total_degree"] > 0 for r in result)

    def test_empty_store_returns_empty(self, empty_store):
        from dagayn.analysis import find_hub_nodes

        assert find_hub_nodes(empty_store) == []

    def test_artifact_scope_filters_docs_and_tests(self, tmp_path):
        from dagayn.analysis import find_hub_nodes

        s = GraphStore(tmp_path / "scoped_hubs.db")

        def _node(name, file_path, *, language="python", is_test=False):
            return NodeInfo(
                kind="Function",
                name=name,
                file_path=file_path,
                line_start=1,
                line_end=10,
                language=language,
                parent_name=None,
                params=None,
                return_type=None,
                modifiers=None,
                is_test=is_test,
                extra={},
            )

        def _edge(source, target):
            return EdgeInfo(
                kind="CALLS",
                source=source,
                target=target,
                file_path="src/a.py",
                line=1,
                extra={},
            )

        for node in (
            _node("prod", "src/prod.py"),
            _node("doc", "docs/design.md", language="markdown"),
            _node("test_prod", "tests/test_prod.py", is_test=True),
            _node("caller", "src/caller.py"),
        ):
            s.upsert_node(node)
        for edge in (
            _edge("src/caller.py::caller", "src/prod.py::prod"),
            _edge("src/caller.py::caller", "docs/design.md::doc"),
            _edge("src/caller.py::caller", "tests/test_prod.py::test_prod"),
        ):
            s.upsert_edge(edge)
        s.commit()

        result = find_hub_nodes(s, top_n=10, artifact_scope="code", include_tests=False)
        qns = {item["qualified_name"] for item in result}

        assert "src/prod.py::prod" in qns
        assert "docs/design.md::doc" not in qns
        assert "tests/test_prod.py::test_prod" not in qns


class TestFindBridgeNodes:
    def test_returns_list(self, store):
        from dagayn.analysis import find_bridge_nodes

        result = find_bridge_nodes(store)
        assert isinstance(result, list)

    def test_sorted_by_betweenness_descending(self, store):
        from dagayn.analysis import find_bridge_nodes

        result = find_bridge_nodes(store)
        scores = [r["betweenness"] for r in result]
        assert scores == sorted(scores, reverse=True)

    def test_result_fields(self, store):
        from dagayn.analysis import find_bridge_nodes

        result = find_bridge_nodes(store)
        for item in result:
            assert "name" in item
            assert "qualified_name" in item
            assert "betweenness" in item
            assert item["betweenness"] > 0

    def test_empty_store_returns_empty(self, empty_store):
        from dagayn.analysis import find_bridge_nodes

        assert find_bridge_nodes(empty_store) == []

    def test_top_n_respected(self, store):
        from dagayn.analysis import find_bridge_nodes

        result = find_bridge_nodes(store, top_n=1)
        assert len(result) <= 1

    def test_large_graph_approximation_uses_deterministic_seed(self, store, monkeypatch):
        import networkx as nx

        from dagayn.analysis import find_bridge_nodes

        for idx in range(5001):
            store.upsert_node(
                NodeInfo(
                    kind="Function",
                    name=f"node_{idx}",
                    file_path=f"src/node_{idx}.py",
                    line_start=1,
                    line_end=1,
                    language="python",
                    parent_name=None,
                    params=None,
                    return_type=None,
                    modifiers=None,
                    is_test=False,
                    extra={},
                )
            )
        store.commit()

        calls = []

        def fake_betweenness(graph, *, k=None, normalized=True, seed=None):
            calls.append({"k": k, "normalized": normalized, "seed": seed})
            return {}

        monkeypatch.setattr(nx, "betweenness_centrality", fake_betweenness)

        assert find_bridge_nodes(store) == []
        assert calls == [{"k": 500, "normalized": True, "seed": 0}]

    def test_persisted_bridge_scores_skip_runtime_centrality(self, store, monkeypatch):
        import networkx as nx

        from dagayn.analysis import (
            build_graph_snapshot,
            find_bridge_nodes,
            persist_centrality_scores,
        )

        persisted = persist_centrality_scores(store)
        assert persisted["bridge_scores_persisted"] > 0
        snapshot = build_graph_snapshot(store)

        def fail_runtime_centrality(*args, **kwargs):
            raise AssertionError("betweenness centrality should be read from bridge_scores")

        monkeypatch.setattr(nx, "betweenness_centrality", fail_runtime_centrality)

        result = find_bridge_nodes(store, top_n=1, snapshot=snapshot)

        assert len(result) == 1
        assert result[0]["score_source"] == "persisted"

    def test_persisted_hub_scores_skip_runtime_snapshot(self, store, monkeypatch):
        from dagayn import analysis
        from dagayn.analysis import find_hub_nodes, persist_centrality_scores

        persisted = persist_centrality_scores(store)
        assert persisted["hub_scores_persisted"] > 0

        def fail_runtime_snapshot(*args, **kwargs):
            raise AssertionError("hub scores should be read from hub_scores")

        monkeypatch.setattr(analysis, "build_graph_snapshot", fail_runtime_snapshot)

        result = find_hub_nodes(store, top_n=1, snapshot=cast(Any, object()))

        assert len(result) == 1
        assert result[0]["score_source"] == "persisted"


class TestGenerateSuggestedQuestions:
    def test_returns_list(self, store):
        result = json.loads(store.generate_suggested_questions_json())
        assert isinstance(result, list)

    def test_question_fields(self, store):
        result = json.loads(store.generate_suggested_questions_json())
        for q in result:
            assert "category" in q
            assert "question" in q
            assert "target" in q
            assert "priority" in q
            assert isinstance(q["question"], str)
            assert len(q["question"]) > 0

    def test_priority_values(self, store):
        result = json.loads(store.generate_suggested_questions_json())
        valid = {"high", "medium", "low"}
        for q in result:
            assert q["priority"] in valid

    def test_empty_store_returns_empty(self, empty_store):
        result = json.loads(empty_store.generate_suggested_questions_json())
        assert isinstance(result, list)
