"""Tests for unresolved edge endpoint reporting (issue #33)."""

from __future__ import annotations

import tempfile
from pathlib import Path

import pytest

from dagayn.graph import GraphStore
from dagayn.graph._edge_records import edge_storage_metadata
from dagayn.parser import EdgeInfo, NodeInfo
from dagayn.postprocessing import run_post_processing
from dagayn.tools import query as query_module


@pytest.fixture
def store() -> GraphStore:
    tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
    graph_store = GraphStore(tmp.name)
    yield graph_store
    graph_store.close()
    Path(tmp.name).unlink(missing_ok=True)


def _patch_store(monkeypatch: pytest.MonkeyPatch, graph_store: GraphStore, root: Path) -> None:
    monkeypatch.setattr(
        query_module,
        "_get_store",
        lambda repo_root: (graph_store, root),
    )
    graph_store.close = lambda: None  # type: ignore[method-assign]


def test_callees_of_reports_unresolved_targets(monkeypatch: pytest.MonkeyPatch, store: GraphStore):
    root = Path("/repo")
    store.upsert_node(
        NodeInfo(
            kind="Function",
            name="run",
            file_path="/repo/app.py",
            line_start=1,
            line_end=10,
            language="python",
        )
    )
    store.upsert_edge(
        EdgeInfo(
            kind="CALLS",
            source="/repo/app.py::run",
            target="unique_helper",
            file_path="/repo/app.py",
            line=5,
        )
    )
    store.commit()
    _patch_store(monkeypatch, store, root)

    result = query_module.query_graph(
        pattern="callees_of",
        target="/repo/app.py::run",
        repo_root=str(root),
    )

    assert result["status"] == "ok"
    assert result["result_count"] == 0
    assert result["unresolved_count"] == 1
    assert result["unresolved_targets"] == ["unique_helper"]
    assert result["zero_result_reason"] == "unresolved_endpoints_only"
    assert result["confidence"] == "medium"
    assert result["edges"][0]["target"] == "unique_helper"


def test_imports_of_flags_unresolved_import_target(
    monkeypatch: pytest.MonkeyPatch, store: GraphStore
):
    root = Path("/repo")
    store.upsert_node(
        NodeInfo(
            kind="File",
            name="/repo/app.py",
            file_path="/repo/app.py",
            line_start=1,
            line_end=20,
            language="python",
        )
    )
    store.upsert_edge(
        EdgeInfo(
            kind="IMPORTS_FROM",
            source="/repo/app.py",
            target="/repo/missing_module.py",
            file_path="/repo/app.py",
            line=1,
        )
    )
    store.commit()
    _patch_store(monkeypatch, store, root)

    result = query_module.query_graph(
        pattern="imports_of",
        target="/repo/app.py",
        repo_root=str(root),
    )

    assert result["status"] == "ok"
    assert result["results"] == [{"import_target": "/repo/missing_module.py", "unresolved": True}]
    assert result["unresolved_count"] == 1
    assert result["unresolved_targets"] == ["/repo/missing_module.py"]


def test_traverse_graph_marks_unresolvable_endpoints_incomplete(
    monkeypatch: pytest.MonkeyPatch, store: GraphStore
):
    root = Path("/repo")
    start_qn = "/repo/app.py::run"
    store.upsert_node(
        NodeInfo(
            kind="Function",
            name="run",
            file_path="/repo/app.py",
            line_start=1,
            line_end=10,
            language="python",
        )
    )
    store.upsert_edge(
        EdgeInfo(
            kind="CALLS",
            source=start_qn,
            target="missing_helper",
            file_path="/repo/app.py",
            line=5,
        )
    )
    store.commit()
    _patch_store(monkeypatch, store, root)
    monkeypatch.setattr(
        query_module,
        "hybrid_search",
        lambda *args, **kwargs: {"results": [{"qualified_name": start_qn}]},
    )

    result = query_module.traverse_graph_func(
        query="run",
        mode="bfs",
        depth=2,
        repo_root=str(root),
    )

    assert result["status"] == "ok"
    assert result["reachability"]["state"] == "truncated"
    assert result["reachability"]["truncated"] is True
    assert result["reachability"]["unresolved_count"] == 1
    assert result["reachability"]["unresolved_targets"] == ["missing_helper"]


def test_traverse_dfs_reexpands_when_shorter_path_exists(
    monkeypatch: pytest.MonkeyPatch, store: GraphStore
):
    root = Path("/repo")
    nodes = [
        ("/repo/app.py::start", "start"),
        ("/repo/app.py::via_long", "via_long"),
        ("/repo/app.py::mid", "mid"),
        ("/repo/app.py::target", "target"),
    ]
    for qn, name in nodes:
        store.upsert_node(
            NodeInfo(
                kind="Function",
                name=name,
                file_path="/repo/app.py",
                line_start=1,
                line_end=10,
                language="python",
            )
        )
    store.upsert_edge(
        EdgeInfo(
            kind="CALLS",
            source="/repo/app.py::start",
            target="/repo/app.py::via_long",
            file_path="/repo/app.py",
            line=1,
        )
    )
    store.upsert_edge(
        EdgeInfo(
            kind="CALLS",
            source="/repo/app.py::via_long",
            target="/repo/app.py::mid",
            file_path="/repo/app.py",
            line=2,
        )
    )
    store.upsert_edge(
        EdgeInfo(
            kind="CALLS",
            source="/repo/app.py::mid",
            target="/repo/app.py::target",
            file_path="/repo/app.py",
            line=3,
        )
    )
    store.upsert_edge(
        EdgeInfo(
            kind="CALLS",
            source="/repo/app.py::start",
            target="/repo/app.py::target",
            file_path="/repo/app.py",
            line=4,
        )
    )
    store.commit()
    _patch_store(monkeypatch, store, root)
    monkeypatch.setattr(
        query_module,
        "hybrid_search",
        lambda *args, **kwargs: {"results": [{"qualified_name": "/repo/app.py::start"}]},
    )

    result = query_module.traverse_graph_func(
        query="start",
        mode="dfs",
        depth=3,
        repo_root=str(root),
    )

    depths = {entry["qualified_name"]: entry["depth"] for entry in result["traversal"]}
    assert depths["/repo/app.py::target"] == 1


def test_edge_storage_metadata_demotes_unresolved_prefix_targets():
    _, confidence, tier = edge_storage_metadata(
        EdgeInfo(
            kind="CALLS",
            source="/repo/app.py::run",
            target="<unresolved:helper>",
            file_path="/repo/app.py",
            line=1,
            extra={"confidence": 1.0, "confidence_tier": "EXTRACTED"},
        )
    )
    assert confidence == 0.2
    assert tier == "LOW"


def test_postprocess_demotes_edges_with_missing_endpoints(store: GraphStore):
    store.upsert_node(
        NodeInfo(
            kind="Function",
            name="run",
            file_path="/repo/app.py",
            line_start=1,
            line_end=10,
            language="python",
        )
    )
    store.upsert_edge(
        EdgeInfo(
            kind="CALLS",
            source="/repo/app.py::run",
            target="missing_helper",
            file_path="/repo/app.py",
            line=5,
            extra={"confidence": 1.0, "confidence_tier": "EXTRACTED"},
        )
    )
    store.commit()

    result = run_post_processing(store)

    assert result.unresolved_endpoint_edges_demoted == 1
    edge = store.get_edges_by_source("/repo/app.py::run")[0]
    assert edge.confidence == 0.2
    assert edge.confidence_tier == "LOW"
