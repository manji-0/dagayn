from __future__ import annotations

import inspect

from dagayn import main as crg_main
from dagayn.tools import architecture_analysis


def test_architecture_analysis_sap_violations_preserves_exclusion_explanation(monkeypatch) -> None:
    monkeypatch.setattr(
        architecture_analysis,
        "detect_sap_violations_func",
        lambda **kwargs: {
            "status": "ok",
            "summary": (
                "Found 1 SAP violation. sap_violations suppresses test and fixture "
                "scopes; inspect sap_metrics notes for raw values."
            ),
            "excluded_scope_categories": ["test-scope", "fixture-scope"],
            "exclusion_reason": (
                "test and fixture scopes are retained in sap_metrics notes but "
                "omitted from sap_violations"
            ),
        },
    )

    result = architecture_analysis.architecture_analysis_func(mode="sap_violations")

    assert result["status"] == "ok"
    assert result["mode"] == "sap_violations"
    assert result["called_subtool"] == "detect_sap_violations_func"
    assert result["excluded_scope_categories"] == ["test-scope", "fixture-scope"]
    assert "suppresses test and fixture scopes" in result["summary"]


def test_architecture_analysis_wrapper_exposes_typed_dispatch_args() -> None:
    params = inspect.signature(crg_main.architecture_analysis_tool).parameters

    for name in (
        "mode",
        "detail_level",
        "top_n",
        "sort_by",
        "community_name",
        "community_id",
        "granularity",
        "scope_kind",
        "artifact_scope",
        "min_delta",
        "min_distance",
    ):
        assert name in params
    assert params["mode"].default == "overview"
    assert params["detail_level"].default == "minimal"
    assert params["top_n"].default == 10
    assert params["artifact_scope"].default == "code"


def test_architecture_analysis_routes_every_mode(monkeypatch) -> None:
    calls: list[tuple[str, dict]] = []

    def fake(name):
        def _inner(**kwargs):
            calls.append((name, kwargs))
            return {"status": "ok", "summary": name}

        return _inner

    mapping = {
        "overview": (
            "get_architecture_overview_func",
            {"detail_level": "verbose", "top_n": 7, "artifact_scope": "docs"},
        ),
        "communities": (
            "list_communities_func",
            {"sort_by": "cohesion", "min_size": 2, "limit": 7},
        ),
        "community": ("get_community_func", {"community_name": "auth", "include_members": True}),
        "hubs": ("get_hub_nodes_func", {"top_n": 7}),
        "bridges": ("get_bridge_nodes_func", {"top_n": 7}),
        "knowledge_gaps": ("get_knowledge_gaps_func", {"top_n": 7}),
        "surprising_connections": ("get_surprising_connections_func", {"top_n": 7}),
        "adp_violations": (
            "detect_adp_violations_func",
            {
                "granularity": "file",
                "artifact_scope": "docs",
                "min_cycle_size": 3,
                "max_cycle_length": 6,
                "top_n": 7,
            },
        ),
        "sdp_metrics": (
            "compute_sdp_metrics_func",
            {"granularity": "file", "artifact_scope": "docs", "top_n": 7},
        ),
        "sdp_violations": (
            "detect_sdp_violations_func",
            {"granularity": "file", "artifact_scope": "docs", "min_delta": 0.2, "top_n": 7},
        ),
        "sap_metrics": (
            "compute_sap_metrics_func",
            {
                "scope_kind": "file",
                "unit_filter": ["pkg"],
                "artifact_scope": "docs",
                "top_n": 7,
            },
        ),
        "sap_violations": (
            "detect_sap_violations_func",
            {"scope_kind": "file", "artifact_scope": "docs", "min_distance": 0.4, "top_n": 7},
        ),
    }

    for subtool, _kwargs in mapping.values():
        monkeypatch.setattr(architecture_analysis, subtool, fake(subtool))

    for mode, (subtool, expected) in mapping.items():
        result = architecture_analysis.architecture_analysis_func(
            mode=mode,  # type: ignore[arg-type]
            repo_root="/repo",
            detail_level="verbose",
            top_n=7,
            sort_by="cohesion",
            min_size=2,
            community_name="auth",
            include_members=True,
            granularity="file",
            scope_kind="file",
            unit_filter=["pkg"],
            artifact_scope="docs",
            min_cycle_size=3,
            max_cycle_length=6,
            min_delta=0.2,
            min_distance=0.4,
        )

        assert result["status"] == "ok"
        assert result["mode"] == mode
        assert result["called_subtool"] == subtool
        called_name, kwargs = calls.pop(0)
        assert called_name == subtool
        assert kwargs["repo_root"] == "/repo"
        for key, value in expected.items():
            assert kwargs[key] == value


def test_architecture_analysis_community_requires_selector() -> None:
    result = architecture_analysis.architecture_analysis_func(mode="community")

    assert result["status"] == "error"
    assert result["mode"] == "community"
    assert "community_id or community_name" in result["summary"]
