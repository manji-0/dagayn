from __future__ import annotations

from pathlib import Path

from dagayn.tools.query import traverse_graph_func
from dagayn.tools.registry_tools import list_repos_func


class _Closable:
    def close(self) -> None:
        pass


def test_traverse_graph_not_found_has_standard_envelope(monkeypatch) -> None:
    monkeypatch.setattr(
        "dagayn.tools.query._get_store",
        lambda repo_root: (_Closable(), Path("/repo")),
    )
    monkeypatch.setattr(
        "dagayn.tools.query.hybrid_search",
        lambda store, query, **kwargs: {"mode": "empty", "results": []},
    )

    result = traverse_graph_func(query="missing-symbol", repo_root="/repo")

    assert result["status"] == "not_found"
    assert result["summary"] == "No node matching 'missing-symbol'."
    assert result["traversal"] == []
    assert result["reachability"] == {
        "state": "not_found",
        "truncated": False,
        "max_depth": 3,
        "nodes_visited": 0,
    }
    assert result["_hints"]["next_steps"][0]["tool"] == "semantic_search_nodes_tool"


def test_list_repos_has_hints(monkeypatch, tmp_path: Path) -> None:
    (tmp_path / ".dagayn").mkdir()
    (tmp_path / ".dagayn" / "registry.json").write_text(
        '{"repos": [{"alias": "dagayn", "path": "/repo"}]}'
    )
    monkeypatch.setenv("HOME", str(tmp_path))

    result = list_repos_func()

    assert result["status"] == "ok"
    assert result["repos"] == [{"alias": "dagayn", "path": "/repo"}]
    assert result["_hints"]["next_steps"][0]["tool"] == "cross_repo_search_tool"


def test_flow_tool_runtime_error_has_missingness(monkeypatch) -> None:
    from dagayn.tools import flow_dispatcher

    def _boom(repo_root):
        raise ValueError("graph unavailable")

    monkeypatch.setattr(flow_dispatcher, "_get_store", _boom)

    result = flow_dispatcher.flow_func(mode="list", repo_root="/repo")

    assert result["status"] == "error"
    assert result["error"] == "graph unavailable"
    assert result["missingness"][0]["reason_code"] == "tool_runtime_error"


def test_list_repos_runtime_error_has_missingness(monkeypatch, tmp_path: Path) -> None:
    registry = tmp_path / ".dagayn" / "registry.json"
    registry.mkdir(parents=True)  # unreadable as a file
    monkeypatch.setenv("HOME", str(tmp_path))

    result = list_repos_func()

    assert result["status"] == "error"
    assert result["error"] == f"[Errno 21] Is a directory: '{registry}'"
    assert result["missingness"][0]["reason_code"] == "tool_runtime_error"


def test_query_graph_runtime_error_has_missingness(monkeypatch) -> None:
    from dagayn.tools import query as query_module

    def _boom(repo_root):
        raise ValueError("graph unavailable")

    monkeypatch.setattr(query_module, "_get_store", _boom)

    result = query_module.query_graph(pattern="callers_of", target="foo", repo_root="/repo")

    assert result["status"] == "error"
    assert result["error"] == "graph unavailable"
    assert result["missingness"][0]["reason_code"] == "tool_runtime_error"
