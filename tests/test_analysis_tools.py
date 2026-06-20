"""Tests for dagayn.tools.analysis_tools MCP wrappers."""

from __future__ import annotations

from pathlib import Path
from unittest.mock import MagicMock

import pytest

from dagayn.graph import GraphStore
from dagayn.parser import EdgeInfo, NodeInfo
from dagayn.tools import analysis_tools


@pytest.fixture
def analysis_store(tmp_path):
    db_path = tmp_path / "analysis_tools.db"
    store = GraphStore(db_path)

    def _node(kind: str, name: str, file_path: str) -> NodeInfo:
        return NodeInfo(
            kind=kind,
            name=name,
            file_path=file_path,
            line_start=1,
            line_end=10,
            language="python",
        )

    nodes = [
        _node("File", "core.py", "src/core.py"),
        _node("Function", "process", "src/core.py"),
        _node("Function", "helper_a", "src/core.py"),
        _node("Function", "helper_b", "src/core.py"),
        _node("Function", "helper_c", "src/core.py"),
    ]
    for node in nodes:
        store.upsert_node(node)

    edges = [
        EdgeInfo(
            kind="CALLS",
            source="src/core.py::helper_a",
            target="src/core.py::process",
            file_path="src/core.py",
            line=1,
        ),
        EdgeInfo(
            kind="CALLS",
            source="src/core.py::helper_b",
            target="src/core.py::process",
            file_path="src/core.py",
            line=2,
        ),
        EdgeInfo(
            kind="CALLS",
            source="src/core.py::helper_c",
            target="src/core.py::process",
            file_path="src/core.py",
            line=3,
        ),
    ]
    for edge in edges:
        store.upsert_edge(edge)
    store.commit()
    return store


def _patch_store(monkeypatch, store: GraphStore, root: Path) -> MagicMock:
    close_mock = MagicMock(wraps=store.close)
    store.close = close_mock
    monkeypatch.setattr(analysis_tools, "_get_store", lambda repo_root: (store, root))
    return close_mock


class TestAnalysisToolWrappers:
    def test_get_hub_nodes_func_returns_guidance_and_closes_store(
        self, monkeypatch, analysis_store, tmp_path
    ):
        close_mock = _patch_store(monkeypatch, analysis_store, tmp_path)

        result = analysis_tools.get_hub_nodes_func(repo_root=str(tmp_path), top_n=3)

        assert result["status"] == "ok"
        assert result["count"] >= 1
        assert "hub_nodes" in result
        assert "guidance" in result
        assert result["guidance"][0]["claim"]
        assert result["guidance"][0]["confidence"] in {"low", "medium", "high"}
        assert "answerability" in result
        assert "_hints" in result
        close_mock.assert_called_once()

    def test_get_bridge_nodes_func_returns_guidance_and_closes_store(
        self, monkeypatch, analysis_store, tmp_path
    ):
        close_mock = _patch_store(monkeypatch, analysis_store, tmp_path)

        result = analysis_tools.get_bridge_nodes_func(repo_root=str(tmp_path), top_n=3)

        assert result["status"] == "ok"
        assert "bridge_nodes" in result
        assert result["guidance"][0]["reason_codes"] == ["bridge_nodes"]
        close_mock.assert_called_once()

    def test_get_knowledge_gaps_func_returns_categories_and_closes_store(
        self, monkeypatch, analysis_store, tmp_path
    ):
        close_mock = _patch_store(monkeypatch, analysis_store, tmp_path)

        result = analysis_tools.get_knowledge_gaps_func(repo_root=str(tmp_path), top_n=5)

        assert result["status"] == "ok"
        assert "gaps" in result
        for key in (
            "untested_hotspots",
            "single_file_communities",
            "isolated_nodes",
            "thin_communities",
        ):
            assert key in result["gaps"]
        assert result["guidance"][0]["reason_codes"] == ["knowledge_gaps"]
        close_mock.assert_called_once()

    def test_get_surprising_connections_func_closes_store(
        self, monkeypatch, analysis_store, tmp_path
    ):
        close_mock = _patch_store(monkeypatch, analysis_store, tmp_path)

        result = analysis_tools.get_surprising_connections_func(repo_root=str(tmp_path), top_n=5)

        assert result["status"] == "ok"
        assert "surprising_connections" in result
        assert result["guidance"][0]["reason_codes"] == ["surprising_connections"]
        close_mock.assert_called_once()

    def test_get_suggested_questions_func_closes_store(self, monkeypatch, analysis_store, tmp_path):
        close_mock = _patch_store(monkeypatch, analysis_store, tmp_path)

        result = analysis_tools.get_suggested_questions_func(repo_root=str(tmp_path), top_n=5)

        assert result["status"] == "ok"
        assert "questions" in result
        assert result["guidance"][0]["reason_codes"] == ["suggested_questions"]
        close_mock.assert_called_once()
