"""Tests for dagayn/analysis.py: hub detection, bridge nodes, knowledge gaps, etc."""

from __future__ import annotations

from typing import Any, cast

import pytest

from dagayn.graph import GraphStore
from dagayn.parser import EdgeInfo, NodeInfo
from tests.store_sql import store_conn


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
            _node("DocSection", "design", "docs/design.md", language="markdown"),
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

    def test_hotspot_degree_uses_all_edges_for_scoped_code_nodes(self, tmp_path):
        from dagayn.analysis import find_knowledge_gaps

        s = GraphStore(tmp_path / "hotspot_scope.db")

        def _node(kind, name, file_path, *, language="python"):
            return NodeInfo(
                kind=kind,
                name=name,
                file_path=file_path,
                line_start=1,
                line_end=5,
                language=language,
                parent_name=None,
                params=None,
                return_type=None,
                modifiers=None,
                is_test=False,
                extra={},
            )

        s.upsert_node(_node("Function", "hub", "src/hub.py"))
        for idx in range(9):
            s.upsert_node(_node("Function", f"low_{idx}", f"src/low_{idx}.py"))
        for idx in range(20):
            s.upsert_node(
                _node("DocSection", f"doc_{idx}", f"docs/doc_{idx}.md", language="markdown")
            )
            s.upsert_edge(
                EdgeInfo(
                    kind="CROSS_ARTIFACT",
                    source="src/hub.py::hub",
                    target=f"docs/doc_{idx}.md::doc_{idx}",
                    file_path="src/hub.py",
                    line=1,
                    extra={},
                )
            )
        for idx in range(9):
            s.upsert_edge(
                EdgeInfo(
                    kind="CROSS_ARTIFACT",
                    source=f"src/low_{idx}.py::low_{idx}",
                    target="docs/doc_0.md::doc_0",
                    file_path=f"src/low_{idx}.py",
                    line=1,
                    extra={},
                )
            )
        s.commit()

        result = find_knowledge_gaps(s, top_n=10, artifact_scope="code")
        qns = {item["qualified_name"] for item in result["untested_hotspots"]}

        assert result["_meta"]["thresholds"]["untested_hotspot_min_degree"] == 20
        assert "src/hub.py::hub" in qns

    def test_natural_single_file_doc_communities_are_classified_as_noise(self, tmp_path):
        from dagayn.analysis import find_knowledge_gaps

        db_path = tmp_path / "single_file_noise.db"
        s = GraphStore(db_path)

        def _node(kind, name, file_path, *, language="markdown"):
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
                is_test=False,
                extra={},
            )

        for node in (
            _node("File", "README.ja.md", "README.ja.md"),
            _node("DocSection", "overview", "README.ja.md"),
            _node("DocSection", "usage", "README.ja.md"),
            _node("DocSection", "faq", "README.ja.md"),
            _node("File", "service.py", "src/service.py", language="python"),
            _node("Class", "Service", "src/service.py", language="python"),
            _node("Function", "run", "src/service.py", language="python"),
            _node("Function", "stop", "src/service.py", language="python"),
        ):
            s.upsert_node(node)

        store_conn(s).executemany(
            "INSERT INTO communities (id, name, size) VALUES (?, ?, ?)",
            [(1, "readme-ja", 3), (2, "service", 3)],
        )
        store_conn(s).execute("UPDATE nodes SET community_id = 1 WHERE file_path = 'README.ja.md'")
        store_conn(s).execute(
            "UPDATE nodes SET community_id = 2 WHERE file_path = 'src/service.py'"
        )
        s.commit()

        result = find_knowledge_gaps(s, top_n=10)

        files = {item["file"] for item in result["single_file_communities"]}
        noise = result["_meta"]["classified_noise_examples"]["natural_single_file_communities"]
        small_noise = result["_meta"]["classified_noise_examples"]["small_single_file_communities"]

        assert "src/service.py" not in files
        assert "README.ja.md" not in files
        assert small_noise[0]["classification"] == "small_single_file_cluster"
        assert small_noise[0]["file"] == "src/service.py"
        assert result["_meta"]["classified_noise_counts"]["natural_single_file_communities"] == 1
        assert noise[0]["classification"] == "standalone_readme"

    def test_integrated_single_file_communities_are_classified_as_noise(self, tmp_path):
        from dagayn.analysis import find_knowledge_gaps

        s = GraphStore(tmp_path / "integrated_single_file.db")

        def _node(name, file_path="src/component.py"):
            return NodeInfo(
                kind="Function",
                name=name,
                file_path=file_path,
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

        for idx in range(12):
            s.upsert_node(_node(f"member_{idx}"))
        for idx in range(4):
            s.upsert_node(_node(f"external_{idx}", f"src/external_{idx}.py"))

        store_conn(s).executemany(
            "INSERT INTO communities (id, name, size) VALUES (?, ?, ?)",
            [(1, "component", 12), (2, "external", 4)],
        )
        store_conn(s).execute(
            "UPDATE nodes SET community_id = 1 WHERE file_path = 'src/component.py'"
        )
        store_conn(s).execute(
            "UPDATE nodes SET community_id = 2 WHERE file_path LIKE 'src/external_%'"
        )
        for idx in range(8):
            s.upsert_edge(
                EdgeInfo(
                    kind="CALLS",
                    source=f"src/component.py::member_{idx}",
                    target=f"src/component.py::member_{idx + 1}",
                    file_path="src/component.py",
                    line=1,
                    extra={},
                )
            )
        for idx in range(4):
            s.upsert_edge(
                EdgeInfo(
                    kind="CALLS",
                    source=f"src/external_{idx}.py::external_{idx}",
                    target=f"src/component.py::member_{idx}",
                    file_path=f"src/external_{idx}.py",
                    line=1,
                    extra={},
                )
            )
        s.commit()

        result = find_knowledge_gaps(s, top_n=10, artifact_scope="code")

        assert result["single_file_communities"] == []
        integrated = result["_meta"]["classified_noise_examples"][
            "integrated_single_file_communities"
        ]
        assert integrated[0]["classification"] == "integrated_single_file_component"
        assert integrated[0]["external_degree"] == 4
        assert integrated[0]["external_edge_ratio"] >= 0.25

    def test_single_file_community_reports_edge_shape_metrics(self, tmp_path):
        from dagayn.analysis import find_knowledge_gaps

        s = GraphStore(tmp_path / "single_file_metrics.db")

        for idx in range(12):
            s.upsert_node(
                NodeInfo(
                    kind="Function",
                    name=f"member_{idx}",
                    file_path="src/island.py",
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
        store_conn(s).execute(
            "INSERT INTO communities (id, name, size) VALUES (?, ?, ?)",
            (1, "island", 12),
        )
        store_conn(s).execute("UPDATE nodes SET community_id = 1")
        for idx in range(3):
            s.upsert_edge(
                EdgeInfo(
                    kind="CALLS",
                    source=f"src/island.py::member_{idx}",
                    target=f"src/island.py::member_{idx + 1}",
                    file_path="src/island.py",
                    line=1,
                    extra={},
                )
            )
        s.commit()

        result = find_knowledge_gaps(s, top_n=10, artifact_scope="code")
        item = result["single_file_communities"][0]

        assert item["file"] == "src/island.py"
        assert item["internal_edges"] == 3
        assert item["external_edges"] == 0
        assert item["external_degree"] == 0
        assert item["external_edge_ratio"] == 0.0
        assert "evidence" in item

    def test_public_api_isolated_nodes_are_classified_as_noise(self, tmp_path):
        from dagayn.analysis import find_knowledge_gaps

        source = tmp_path / "lib.rs"
        source.write_text(
            "pub enum Status { Ready }\n\n"
            "pub fn exported_language() -> tree_sitter::Language { todo!() }\n\n"
            "fn internal_helper() {}\n\n"
            "#[cfg(test)]\n"
            "mod tests {\n"
            "    fn loads_language() {}\n"
            "}\n\n"
            "impl Store {\n"
            "    pub fn method(&self) {}\n"
            "}\n",
            encoding="utf-8",
        )
        s = GraphStore(tmp_path / "public_api_noise.db")

        def _node(kind, name, line_start):
            return NodeInfo(
                kind=kind,
                name=name,
                file_path=str(source),
                line_start=line_start,
                line_end=line_start,
                language="rust",
                parent_name=None,
                params=None,
                return_type=None,
                modifiers=None,
                is_test=False,
                extra={},
            )

        for node in (
            _node("Class", "Status", 1),
            _node("Function", "exported_language", 3),
            _node("Function", "internal_helper", 5),
            _node("Function", "loads_language", 9),
            _node("Class", "Store", 12),
        ):
            s.upsert_node(node)
        s.commit()

        result = find_knowledge_gaps(s, top_n=10, artifact_scope="code")
        isolated = {item["qualified_name"] for item in result["isolated_nodes"]}
        noise = result["_meta"]["classified_noise_examples"]["low_signal_isolated_nodes"]
        noise_qns = {item["qualified_name"] for item in noise}

        assert f"{source}::internal_helper" in isolated
        assert f"{source}::Status" not in isolated
        assert f"{source}::exported_language" not in isolated
        assert f"{source}::loads_language" not in isolated
        assert f"{source}::Store" not in isolated
        assert noise_qns == {
            f"{source}::Status",
            f"{source}::exported_language",
            f"{source}::loads_language",
            f"{source}::Store",
        }

    def test_classified_noise_examples_are_bounded(self, tmp_path):
        from dagayn.analysis import find_knowledge_gaps

        source = tmp_path / "lib.rs"
        source.write_text(
            "\n".join(f"pub fn exported_{idx}() {{}}" for idx in range(20)),
            encoding="utf-8",
        )
        s = GraphStore(tmp_path / "bounded_noise.db")

        for idx in range(20):
            s.upsert_node(
                NodeInfo(
                    kind="Function",
                    name=f"exported_{idx}",
                    file_path=str(source),
                    line_start=idx + 1,
                    line_end=idx + 1,
                    language="rust",
                    parent_name=None,
                    params=None,
                    return_type=None,
                    modifiers=None,
                    is_test=False,
                    extra={},
                )
            )
        s.commit()

        result = find_knowledge_gaps(s, top_n=50, artifact_scope="code")
        examples = result["_meta"]["classified_noise_examples"]["low_signal_isolated_nodes"]

        assert result["_meta"]["classified_noise_counts"]["low_signal_isolated_nodes"] == 20
        assert len(examples) == 10

    def test_artifact_scope_filters_isolated_doc_noise(self, tmp_path):
        from dagayn.analysis import find_knowledge_gaps

        s = GraphStore(tmp_path / "scoped_gaps.db")

        def _node(kind, name, file_path, *, language="python"):
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
                is_test=False,
                extra={},
            )

        for node in (
            _node("Function", "orphan", "src/orphan.py"),
            _node("DocBody", "body", "docs/design.md", language="markdown"),
        ):
            s.upsert_node(node)
        s.commit()

        result = find_knowledge_gaps(s, top_n=10, artifact_scope="code")
        qns = {item["qualified_name"] for item in result["isolated_nodes"]}

        assert "src/orphan.py::orphan" in qns
        assert "docs/design.md::body" not in qns
        assert result["_meta"]["artifact_scope"] == "code"


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
        store_conn(s).execute(
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

    def test_artifact_scope_filters_doc_to_code_surprises(self, tmp_path):
        from dagayn.analysis import find_surprising_connections

        s = GraphStore(tmp_path / "scoped_surprises.db")

        def _node(name, file_path, *, language="python"):
            return NodeInfo(
                kind="Function",
                name=name,
                file_path=file_path,
                line_start=1,
                line_end=5,
                language=language,
                parent_name=None,
                params=None,
                return_type=None,
                modifiers=None,
                is_test=False,
                extra={},
            )

        for node in (
            _node("doc", "docs/design.md", language="markdown"),
            _node("prod", "src/prod.py"),
            _node("other", "src/other.py"),
        ):
            s.upsert_node(node)
        s.upsert_edge(
            EdgeInfo(
                kind="CROSS_ARTIFACT",
                source="docs/design.md::doc",
                target="src/prod.py::prod",
                file_path="docs/design.md",
                line=1,
                extra={},
            )
        )
        s.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source="src/other.py::other",
                target="src/prod.py::prod",
                file_path="src/other.py",
                line=1,
                extra={},
            )
        )
        s.commit()
        store_conn(s).execute(
            """
            UPDATE nodes
            SET community_id = CASE
                WHEN file_path LIKE 'docs/%' THEN 1
                WHEN file_path = 'src/prod.py' THEN 2
                ELSE 3
            END
            """
        )
        s.commit()

        result = find_surprising_connections(s, top_n=10, artifact_scope="code")
        pairs = {(item["source_qualified"], item["target_qualified"]) for item in result}

        assert ("docs/design.md::doc", "src/prod.py::prod") not in pairs

    def test_degree_imbalance_alone_is_not_surprising(self, tmp_path):
        from dagayn.analysis import find_surprising_connections

        s = GraphStore(tmp_path / "degree_only_surprises.db")

        def _node(name):
            return NodeInfo(
                kind="Function",
                name=name,
                file_path="src/same.py",
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

        for node in (_node("hub"), _node("low"), _node("helper1"), _node("helper2")):
            s.upsert_node(node)
        for edge in (
            EdgeInfo(
                kind="CALLS",
                source="src/same.py::low",
                target="src/same.py::hub",
                file_path="src/same.py",
                line=1,
                extra={},
            ),
            EdgeInfo(
                kind="CALLS",
                source="src/same.py::helper1",
                target="src/same.py::hub",
                file_path="src/same.py",
                line=1,
                extra={},
            ),
            EdgeInfo(
                kind="CALLS",
                source="src/same.py::helper2",
                target="src/same.py::hub",
                file_path="src/same.py",
                line=1,
                extra={},
            ),
        ):
            s.upsert_edge(edge)
        s.commit()
        store_conn(s).execute("UPDATE nodes SET community_id = 1")
        s.commit()

        assert find_surprising_connections(s, top_n=10) == []

    def test_contains_edges_are_not_surprising_connections(self, tmp_path):
        from dagayn.analysis import find_surprising_connections

        s = GraphStore(tmp_path / "contains_surprises.db")

        def _node(kind, name):
            return NodeInfo(
                kind=kind,
                name=name,
                file_path="src/service.py",
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

        for node in (_node("Class", "Service"), _node("Function", "run")):
            s.upsert_node(node)
        s.upsert_edge(
            EdgeInfo(
                kind="CONTAINS",
                source="src/service.py::Service",
                target="src/service.py::run",
                file_path="src/service.py",
                line=1,
                extra={},
            )
        )
        s.commit()
        store_conn(s).execute(
            """
            UPDATE nodes
            SET community_id = CASE
                WHEN kind = 'Class' THEN 1
                ELSE 2
            END
            """
        )
        s.commit()

        assert find_surprising_connections(s, top_n=10) == []


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
