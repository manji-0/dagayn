"""Tests for dagayn/analysis.py: bridge nodes, knowledge gaps, etc."""

from __future__ import annotations

import json

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
