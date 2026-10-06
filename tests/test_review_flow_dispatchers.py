from __future__ import annotations

import inspect
import subprocess
from pathlib import Path
from typing import cast

from dagayn.contracts.state_types import ReviewMode
from dagayn.server import main as crg_main
from dagayn.tools import _dispatch, flow_dispatcher, review_dispatcher


def test_review_wrapper_exposes_typed_dispatch_args() -> None:
    params = inspect.signature(crg_main.review_tool).parameters

    for name in (
        "mode",
        "changed_files",
        "base",
        "include_source",
        "max_depth",
        "max_nodes",
        "max_lines_per_file",
        "detail_level",
    ):
        assert name in params
    assert params["mode"].default == "changes"
    assert params["base"].default is None


def test_flow_wrapper_exposes_typed_dispatch_args() -> None:
    params = inspect.signature(crg_main.flow_tool).parameters

    for name in (
        "mode",
        "sort_by",
        "limit",
        "kind",
        "detail_level",
        "flow_id",
        "flow_name",
        "include_source",
    ):
        assert name in params
    assert params["mode"].default == "list"
    assert params["sort_by"].default == "criticality"


def test_review_hands_every_mode_to_rust(monkeypatch) -> None:
    calls: list[tuple[str, dict]] = []
    opened: list[str | None] = []

    class _Store:
        def close(self) -> None:
            pass

    def fake_get_store(repo_root):
        opened.append(repo_root)
        return _Store(), Path("/repo")

    def fake_native(name, **kwargs):
        calls.append((name, kwargs))
        return {"status": "ok", "summary": name}

    monkeypatch.setattr(review_dispatcher, "_get_store", fake_get_store)
    monkeypatch.setattr(review_dispatcher, "native_tool", fake_native)

    for mode in ("changes", "context", "affected_flows", "impact"):
        review_dispatcher.review_func(
            mode=cast(ReviewMode, mode),
            changed_files=["a.py"],
            base="main",
            max_depth=4,
            max_nodes=12,
            max_lines_per_file=25,
            detail_level="minimal",
            repo_root="/repo",
        )
    review_dispatcher.review_func(mode="context", include_source=False, repo_root="/repo")

    assert opened == ["/repo"] * 5
    for (name, kwargs), mode in zip(
        calls, ["changes", "context", "affected_flows", "impact"], strict=False
    ):
        assert name == "review_tool"
        assert kwargs == {
            "mode": mode,
            "base": "main",
            "changed_files": ["a.py"],
            # Left unset, so Rust applies each mode's own default.
            "include_source": None,
            "max_depth": 4,
            "max_nodes": 12,
            "max_lines_per_file": 25,
            "detail_level": "minimal",
            "repo_root": "/repo",
        }
    assert calls[4][1]["include_source"] is False


def test_flow_routes_modes(monkeypatch) -> None:
    calls: list[tuple[str, dict]] = []
    opened: list[str | None] = []

    class _Store:
        def close(self) -> None:
            pass

    def fake_get_store(repo_root):
        opened.append(repo_root)
        return _Store(), Path("/repo")

    def fake_native(name, **kwargs):
        calls.append((name, kwargs))
        return {"status": "ok", "summary": name}

    monkeypatch.setattr(flow_dispatcher, "_get_store", fake_get_store)
    monkeypatch.setattr(flow_dispatcher, "native_tool", fake_native)

    flow_dispatcher.flow_func(
        mode="list",
        sort_by="depth",
        limit=5,
        kind="Function",
        detail_level="minimal",
        repo_root="/repo",
    )
    flow_dispatcher.flow_func(
        mode="get",
        flow_id=7,
        include_source=True,
        repo_root="/repo",
    )
    flow_dispatcher.flow_func(mode="entry_points", target="helper", repo_root="/repo")

    assert opened == ["/repo", "/repo", "/repo"]
    assert calls[0] == (
        "flow_tool",
        {
            "mode": "list",
            "repo_root": "/repo",
            "sort_by": "depth",
            "limit": 5,
            "kind": "Function",
            "detail_level": "minimal",
        },
    )
    assert calls[1] == (
        "flow_tool",
        {
            "mode": "get",
            "repo_root": "/repo",
            "flow_id": 7,
            "flow_name": None,
            "include_source": True,
            "detail_level": "standard",
        },
    )
    # entry_points defaults to 10 results, not list's 50.
    assert calls[2] == (
        "flow_tool",
        {
            "mode": "entry_points",
            "repo_root": "/repo",
            "target": "helper",
            "limit": 10,
            "detail_level": "standard",
        },
    )


def test_flow_entry_points_requires_target() -> None:
    result = flow_dispatcher.flow_func(mode="entry_points")
    assert result["status"] == "error"
    assert result["error"] == 'Value error, mode="entry_points" requires target.'


def test_flow_get_requires_selector() -> None:
    result = flow_dispatcher.flow_func(mode="get")

    assert result["status"] == "error"
    assert result["mode"] == "get"
    assert "flow_id or flow_name" in result["summary"]
    assert "answerability" in result


def test_dispatcher_error_paths_use_requested_repo_root(monkeypatch) -> None:
    calls: list[tuple[str, str | None]] = []

    def fake_attach(name):
        def _inner(payload: dict, repo_root: str | None = None) -> dict:
            calls.append((name, repo_root))
            payload["answerability"] = {"status": "ok", "repo_root": repo_root}
            return payload

        return _inner

    # Both dispatchers build their error envelope through _dispatch, so
    # swap the recorder before each call to attribute the lookup.
    monkeypatch.setattr(_dispatch, "attach_answerability", fake_attach("review"))
    review = review_dispatcher.review_func(mode=cast(ReviewMode, "unknown"), repo_root="/repo")
    monkeypatch.setattr(_dispatch, "attach_answerability", fake_attach("flow"))
    flow = flow_dispatcher.flow_func(mode="get", repo_root="/repo")

    assert review["answerability"]["repo_root"] == "/repo"
    assert flow["answerability"]["repo_root"] == "/repo"
    assert calls == [("review", "/repo"), ("flow", "/repo")]


def _init_single_commit_repo(tmp_path) -> str:
    repo = tmp_path / "repo"
    repo.mkdir()
    subprocess.run(["git", "init"], cwd=repo, check=True, capture_output=True)
    subprocess.run(
        ["git", "config", "user.email", "test@example.com"],
        cwd=repo,
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "config", "user.name", "Test"],
        cwd=repo,
        check=True,
        capture_output=True,
    )
    app_py = repo / "app.py"
    app_py.write_text("def alpha():\n    return 1\n", encoding="utf-8")
    subprocess.run(["git", "add", "."], cwd=repo, check=True, capture_output=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=repo, check=True, capture_output=True)
    app_py.write_text("def alpha():\n    return 2\n", encoding="utf-8")
    return str(repo)


def test_review_changes_single_commit_repo_returns_graceful_error(tmp_path) -> None:
    repo_root = _init_single_commit_repo(tmp_path)

    result = review_dispatcher.review_func(mode="changes", repo_root=repo_root, base="HEAD~1")

    assert result["status"] == "error"
    assert result["mode"] == "changes"
    assert result["called_subtool"] == "detect_changes_func"
    assert "HEAD~1" in result["summary"]
    assert result["diff_parse_status"] == "base_unresolved"
    reason_codes = [item["reason_code"] for item in result["missingness"]]
    assert "diff_base_unreachable" in reason_codes


def test_review_changes_defaults_to_head_on_a_dirty_tree(tmp_path) -> None:
    repo_root = _init_single_commit_repo(tmp_path)

    result = review_dispatcher.review_func(mode="changes", repo_root=repo_root)

    assert result["status"] == "ok"
    assert result["change_entity_summary"]["base"] == "HEAD"
    assert result["changed_files"] == ["app.py"]


def test_review_dispatcher_routes_store_errors_into_its_envelope(monkeypatch) -> None:
    def _boom(repo_root):
        raise ValueError("graph unavailable")

    monkeypatch.setattr(review_dispatcher, "_get_store", _boom)

    result = review_dispatcher.review_func(mode="impact", repo_root="/repo")

    assert result["status"] == "error"
    assert result["mode"] == "impact"
    assert result["called_subtool"] == "get_impact_radius"
    assert result["error"] == "graph unavailable"
    assert result["missingness"][0]["reason_code"] == "tool_runtime_error"
    assert "answerability" in result


def test_flow_dispatcher_routes_store_errors_into_its_envelope(monkeypatch) -> None:
    def _boom(repo_root):
        raise ValueError("graph unavailable")

    monkeypatch.setattr(flow_dispatcher, "_get_store", _boom)

    result = flow_dispatcher.flow_func(mode="get", flow_name="nope", repo_root="/repo")

    assert result["status"] == "error"
    assert result["mode"] == "get"
    assert result["called_subtool"] == "get_flow"
    assert result["error"] == "graph unavailable"
    assert result["missingness"][0]["reason_code"] == "tool_runtime_error"
