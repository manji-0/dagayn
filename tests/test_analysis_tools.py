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
    def test_get_suggested_questions_func_closes_store(self, monkeypatch, analysis_store, tmp_path):
        close_mock = _patch_store(monkeypatch, analysis_store, tmp_path)
        monkeypatch.setattr(analysis_tools, "native_tool", lambda name, **_: {"status": "ok"})

        result = analysis_tools.get_suggested_questions_func(repo_root=str(tmp_path), top_n=5)

        assert result == {"status": "ok"}
        close_mock.assert_called_once()
