"""Tests for dagayn/analysis.py: hub detection, bridge nodes, knowledge gaps, etc."""

from __future__ import annotations

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


class TestFindKnowledgeGaps:
    def test_returns_dict_with_expected_keys(self, store):
        from dagayn.analysis import find_knowledge_gaps

        result = find_knowledge_gaps(store)
        assert "isolated_nodes" in result
        assert "thin_communities" in result
        assert "untested_hotspots" in result
        assert "single_file_communities" in result

    def test_isolated_nodes_are_low_degree(self, store):
        from dagayn.analysis import find_knowledge_gaps

        result = find_knowledge_gaps(store)
        for n in result["isolated_nodes"]:
            assert n["degree"] <= 1

    def test_orphan_in_isolated(self, store):
        from dagayn.analysis import find_knowledge_gaps

        result = find_knowledge_gaps(store)
        qns = {n["qualified_name"] for n in result["isolated_nodes"]}
        assert "src/isolated.py::orphan_fn" in qns

    def test_untested_hotspots_have_degree(self, store):
        from dagayn.analysis import find_knowledge_gaps

        result = find_knowledge_gaps(store)
        threshold = result["_meta"]["thresholds"]["untested_hotspot_min_degree"]
        for h in result["untested_hotspots"]:
            assert h["degree"] >= threshold
            assert "evidence" in h

    def test_empty_store_returns_empty_categories(self, empty_store):
        from dagayn.analysis import find_knowledge_gaps

        result = find_knowledge_gaps(empty_store)
        assert result["isolated_nodes"] == []
        assert result["untested_hotspots"] == []
        assert result["_meta"]["thresholds"]["untested_hotspot_min_degree"] == 5

    def test_top_n_and_meta_counts_are_reported(self, store):
        from dagayn.analysis import find_knowledge_gaps

        result = find_knowledge_gaps(store, top_n=1)

        assert result["_meta"]["top_n"] == 1
        assert result["_meta"]["raw_counts"]["isolated_nodes"] >= len(result["isolated_nodes"])
        assert len(result["isolated_nodes"]) <= 1

    def test_untested_hotspots_exclude_docs_and_tests(self, tmp_path):
        from dagayn.analysis import find_knowledge_gaps

        db_path = tmp_path / "gaps.db"
        s = GraphStore(db_path)

        def _node(kind, name, file_path, *, language="python", is_test=False):
            return NodeInfo(
                kind=kind,
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
                file_path="src/service.py",
                line=1,
                extra={},
            )

        candidates = [
            _node("Function", "service", "src/service.py"),
            _node("Class", "design", "docs/design.md", language="markdown"),
            _node("Function", "test_service", "tests/test_service.py", is_test=True),
            _node("Function", "integration_helper", "src/tests.rs"),
        ]
        for idx in range(8):
            candidates.append(_node("Function", f"caller_{idx}", f"src/caller_{idx}.py"))
        for n in candidates:
            s.upsert_node(n)
        for idx in range(8):
            s.upsert_edge(_edge(f"src/caller_{idx}.py::caller_{idx}", "src/service.py::service"))
            s.upsert_edge(_edge(f"src/caller_{idx}.py::caller_{idx}", "docs/design.md::design"))
            s.upsert_edge(
                _edge(f"src/caller_{idx}.py::caller_{idx}", "tests/test_service.py::test_service")
            )
            s.upsert_edge(
                _edge(f"src/caller_{idx}.py::caller_{idx}", "src/tests.rs::integration_helper")
            )
        s.commit()

        result = find_knowledge_gaps(s, top_n=10)
        qns = {h["qualified_name"] for h in result["untested_hotspots"]}

        assert "src/service.py::service" in qns
        assert "docs/design.md::design" not in qns
        assert "tests/test_service.py::test_service" not in qns
        assert "src/tests.rs::integration_helper" not in qns


class TestFindSurprisingConnections:
    def test_returns_list(self, store):
        from dagayn.analysis import find_surprising_connections

        result = find_surprising_connections(store)
        assert isinstance(result, list)

    def test_sorted_by_score_descending(self, store):
        from dagayn.analysis import find_surprising_connections

        result = find_surprising_connections(store)
        scores = [r["surprise_score"] for r in result]
        assert scores == sorted(scores, reverse=True)

    def test_result_fields(self, store):
        from dagayn.analysis import find_surprising_connections

        result = find_surprising_connections(store)
        for item in result:
            assert "source" in item
            assert "target" in item
            assert "surprise_score" in item
            assert "reasons" in item
            assert isinstance(item["reasons"], list)

    def test_empty_store_returns_empty(self, empty_store):
        from dagayn.analysis import find_surprising_connections

        assert find_surprising_connections(empty_store) == []

    def test_top_n_respected(self, store):
        from dagayn.analysis import find_surprising_connections

        result = find_surprising_connections(store, top_n=2)
        assert len(result) <= 2

    def test_scores_are_not_forced_to_identical_values(self, tmp_path):
        from dagayn.analysis import find_surprising_connections

        s = GraphStore(tmp_path / "surprise.db")

        def _node(name: str, file_path: str) -> NodeInfo:
            return NodeInfo(
                kind="Function",
                name=name,
                file_path=file_path,
                line_start=1,
                line_end=5,
                language="python",
                parent_name=None,
                params=None,
                return_type=None,
                modifiers=None,
                is_test=False,
                extra={},
            )

        def _edge(source: str, target: str) -> EdgeInfo:
            return EdgeInfo(
                kind="CALLS",
                source=source,
                target=target,
                file_path="src/a.py",
                line=1,
                extra={},
            )

        for node in (
            _node("src_low", "src/a.py"),
            _node("src_high", "src/c.py"),
            _node("target_regular", "pkg/regular.ts"),
            _node("target_hub", "pkg/hub.ts"),
            _node("helper1", "pkg/helper1.ts"),
            _node("helper2", "pkg/helper2.ts"),
            _node("helper3", "pkg/helper3.ts"),
            _node("helper4", "pkg/helper4.ts"),
        ):
            s.upsert_node(node)

        for edge in (
            _edge("src/a.py::src_low", "pkg/regular.ts::target_regular"),
            _edge("src/c.py::src_high", "pkg/hub.ts::target_hub"),
            _edge("pkg/helper1.ts::helper1", "pkg/hub.ts::target_hub"),
            _edge("pkg/helper2.ts::helper2", "pkg/hub.ts::target_hub"),
            _edge("pkg/helper3.ts::helper3", "pkg/hub.ts::target_hub"),
            _edge("pkg/hub.ts::target_hub", "pkg/helper4.ts::helper4"),
        ):
            s.upsert_edge(edge)

        s.commit()
        s._conn.execute(
            """
            UPDATE nodes
            SET community_id = CASE
                WHEN qualified_name LIKE 'src/%' THEN 1
                ELSE 2
            END
            """
        )
        s.commit()

        result = find_surprising_connections(s, top_n=10)
        scores = [item["surprise_score"] for item in result]

        assert len(scores) >= 2
        assert len(set(scores)) > 1


class TestGenerateSuggestedQuestions:
    def test_returns_list(self, store):
        from dagayn.analysis import generate_suggested_questions

        result = generate_suggested_questions(store)
        assert isinstance(result, list)

    def test_question_fields(self, store):
        from dagayn.analysis import generate_suggested_questions

        result = generate_suggested_questions(store)
        for q in result:
            assert "category" in q
            assert "question" in q
            assert "target" in q
            assert "priority" in q
            assert isinstance(q["question"], str)
            assert len(q["question"]) > 0

    def test_priority_values(self, store):
        from dagayn.analysis import generate_suggested_questions

        result = generate_suggested_questions(store)
        valid = {"high", "medium", "low"}
        for q in result:
            assert q["priority"] in valid

    def test_empty_store_returns_empty(self, empty_store):
        from dagayn.analysis import generate_suggested_questions

        result = generate_suggested_questions(empty_store)
        assert isinstance(result, list)
