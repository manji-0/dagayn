from __future__ import annotations

from pathlib import Path

from dagayn.tools.registry_tools import list_repos_func


def test_list_repos_lists_the_registry(monkeypatch, tmp_path: Path) -> None:
    (tmp_path / ".dagayn").mkdir()
    (tmp_path / ".dagayn" / "registry.json").write_text(
        '{"repos": [{"alias": "dagayn", "path": "/repo"}]}'
    )
    monkeypatch.setenv("HOME", str(tmp_path))

    result = list_repos_func()

    assert result["status"] == "ok"
    assert result["repos"] == [{"alias": "dagayn", "path": "/repo"}]
    assert "_hints" not in result
    assert "next_tool_suggestions" not in result


def test_flow_tool_runtime_error_has_missingness(monkeypatch) -> None:
    from dagayn.tools import flow_dispatcher

    def _boom(repo_root):
        raise ValueError("graph unavailable")

    monkeypatch.setattr(flow_dispatcher, "_get_store", _boom)

    result = flow_dispatcher.flow_func(repo_root="/repo")

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
