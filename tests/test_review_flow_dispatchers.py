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
    assert params["base"].default == "HEAD~1"


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


def test_review_routes_every_mode(monkeypatch) -> None:
    calls: list[tuple[str, dict]] = []

    def fake(name):
        def _inner(**kwargs):
            calls.append((name, kwargs))
            return {"status": "ok", "summary": name}

        return _inner

    mapping = {
        "changes": (
            "detect_changes_func",
            {"include_source": True, "max_depth": 4, "detail_level": "minimal"},
        ),
        "context": (
            "get_review_context",
            {"include_source": True, "max_lines_per_file": 25, "detail_level": "minimal"},
        ),
        "affected_flows": ("get_affected_flows_func", {}),
        "impact": ("get_impact_radius", {"max_depth": 4, "max_results": 12}),
    }

    for subtool, _kwargs in mapping.values():
        monkeypatch.setattr(review_dispatcher, subtool, fake(subtool))

    for mode, (subtool, expected) in mapping.items():
        result = review_dispatcher.review_func(
            mode=cast(ReviewMode, mode),
            changed_files=["a.py"],
            base="main",
            include_source=True,
            max_depth=4,
            max_nodes=12,
            max_lines_per_file=25,
            detail_level="minimal",
            repo_root="/repo",
        )

        assert result["status"] == "ok"
        assert result["mode"] == mode
        assert result["called_subtool"] == subtool
        assert "answerability" in result
        called_name, kwargs = calls.pop(0)
        assert called_name == subtool
        assert kwargs["repo_root"] == "/repo"
        if "changed_files" in kwargs:
            assert kwargs["changed_files"] == ["a.py"]
        if "base" in kwargs:
            assert kwargs["base"] == "main"
        for key, value in expected.items():
            assert kwargs[key] == value


def test_review_dispatcher_preserves_guidance_hints(monkeypatch) -> None:
    expected_hints = {"next_steps": [{"tool": "review_tool", "suggestion": "from guidance"}]}

    monkeypatch.setattr(
        review_dispatcher,
        "detect_changes_func",
        lambda **_kwargs: {"status": "ok", "summary": "changes", "_hints": expected_hints},
    )

    result = review_dispatcher.review_func(mode="changes", repo_root="/repo")

    assert result["_hints"] == expected_hints
    assert "answerability" in result


def test_review_context_defaults_to_source_when_unspecified(monkeypatch) -> None:
    calls: list[dict] = []

    def fake_context(**kwargs):
        calls.append(kwargs)
        return {"status": "ok", "summary": "context"}

    monkeypatch.setattr(review_dispatcher, "get_review_context", fake_context)

    review_dispatcher.review_func(mode="context")

    assert calls[0]["include_source"] is True


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

    assert opened == ["/repo", "/repo"]
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
        },
    )


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

    result = review_dispatcher.review_func(mode="changes", repo_root=repo_root)

    assert result["status"] == "error"
    assert result["mode"] == "changes"
    assert result["called_subtool"] == "detect_changes_func"
    assert "HEAD~1" in result["summary"]
    reason_codes = [item["reason_code"] for item in result["missingness"]]
    assert "diff_base_unreachable" in reason_codes


def test_review_dispatcher_routes_subtool_error_envelopes(monkeypatch) -> None:
    monkeypatch.setattr(
        review_dispatcher,
        "detect_changes_func",
        lambda **_kwargs: {
            "status": "error",
            "summary": "Could not resolve the diff base 'HEAD~1'.",
            "error": "Could not resolve the diff base 'HEAD~1'.",
        },
    )

    result = review_dispatcher.review_func(mode="changes", repo_root="/repo")

    assert result["status"] == "error"
    assert result["mode"] == "changes"
    assert result["called_subtool"] == "detect_changes_func"
    assert "HEAD~1" in result["error"]
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
