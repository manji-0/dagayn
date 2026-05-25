from __future__ import annotations

import inspect

from dagayn import main as crg_main
from dagayn.tools import architecture_analysis, flow_dispatcher, review_dispatcher


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
            mode=mode,  # type: ignore[arg-type]
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

    def fake(name):
        def _inner(**kwargs):
            calls.append((name, kwargs))
            return {"status": "ok", "summary": name}

        return _inner

    monkeypatch.setattr(flow_dispatcher, "list_flows", fake("list_flows"))
    monkeypatch.setattr(flow_dispatcher, "get_flow", fake("get_flow"))

    list_result = flow_dispatcher.flow_func(
        mode="list",
        sort_by="depth",
        limit=5,
        kind="Function",
        detail_level="minimal",
        repo_root="/repo",
    )
    get_result = flow_dispatcher.flow_func(
        mode="get",
        flow_id=7,
        include_source=True,
        repo_root="/repo",
    )

    assert list_result["called_subtool"] == "list_flows"
    assert get_result["called_subtool"] == "get_flow"
    assert "answerability" in list_result
    assert "answerability" in get_result
    assert calls[0] == (
        "list_flows",
        {
            "repo_root": "/repo",
            "sort_by": "depth",
            "limit": 5,
            "kind": "Function",
            "detail_level": "minimal",
        },
    )
    assert calls[1] == (
        "get_flow",
        {
            "flow_id": 7,
            "flow_name": None,
            "include_source": True,
            "repo_root": "/repo",
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

    monkeypatch.setattr(review_dispatcher, "attach_answerability", fake_attach("review"))
    monkeypatch.setattr(flow_dispatcher, "attach_answerability", fake_attach("flow"))
    monkeypatch.setattr(
        architecture_analysis,
        "attach_answerability",
        fake_attach("architecture"),
    )

    review = review_dispatcher.review_func(mode="unknown", repo_root="/repo")  # type: ignore[arg-type]
    flow = flow_dispatcher.flow_func(mode="get", repo_root="/repo")
    architecture = architecture_analysis.architecture_analysis_func(
        mode="community",
        repo_root="/repo",
    )

    assert review["answerability"]["repo_root"] == "/repo"
    assert flow["answerability"]["repo_root"] == "/repo"
    assert architecture["answerability"]["repo_root"] == "/repo"
    assert calls == [("review", "/repo"), ("flow", "/repo"), ("architecture", "/repo")]


def test_architecture_dispatcher_preserves_guidance_hints(monkeypatch) -> None:
    expected_hints = {
        "next_steps": [{"tool": "architecture_analysis_tool", "suggestion": "from guidance"}]
    }
    monkeypatch.setattr(
        architecture_analysis,
        "get_architecture_overview_func",
        lambda **_kwargs: {"status": "ok", "summary": "overview", "_hints": expected_hints},
    )

    result = architecture_analysis.architecture_analysis_func(mode="overview", repo_root="/repo")

    assert result["_hints"] == expected_hints
    assert "answerability" in result
