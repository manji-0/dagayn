from __future__ import annotations

import json
from pathlib import Path
from types import SimpleNamespace

from dagayn.tools.architecture_tools import (
    detect_adp_violations_func,
    detect_sdp_violations_func,
)
from dagayn.tools.query import traverse_graph_func
from dagayn.tools.refactor_tools import refactor_func
from dagayn.tools.registry_tools import list_repos_func
from dagayn.tools.review import get_review_context


class _Closable:
    def close(self) -> None:
        pass


def test_detect_sdp_violations_truncates(monkeypatch) -> None:
    monkeypatch.setattr(
        "dagayn.tools.architecture_tools._get_store",
        lambda repo_root: (_Closable(), None),
    )
    monkeypatch.setattr(
        "dagayn.tools.architecture_tools.find_sdp_violations",
        lambda store, granularity, artifact_scope, dependency_profile, min_delta: [
            {"source": "a", "target": "b", "instability_gap": 0.9},
            {"source": "c", "target": "d", "instability_gap": 0.8},
        ],
    )

    result = detect_sdp_violations_func(top_n=1)

    assert result["status"] == "ok"
    assert result["count"] == 2
    assert result["total"] == 2
    assert result["truncated"] is True
    assert len(result["violations"]) == 1
    assert result["_hints"]["next_steps"][0]["tool"] == "architecture_analysis_tool"


def test_detect_adp_violations_truncated_suggests_listing_every_cycle(monkeypatch) -> None:
    monkeypatch.setattr(
        "dagayn.tools.architecture_tools._get_store",
        lambda repo_root: (_Closable(), None),
    )
    monkeypatch.setattr(
        "dagayn.tools.architecture_tools.find_adp_violations",
        lambda store, **kwargs: [
            {"nodes": ["a", "b"], "length": 2},
            {"nodes": ["c", "d"], "length": 2},
            {"nodes": ["e", "f"], "length": 2},
        ],
    )

    result = detect_adp_violations_func(top_n=2)

    assert result["truncated"] is True
    assert result["next_tool_suggestions"][0] == (
        'architecture_analysis_tool mode="adp_violations" top_n=3 -- list every cycle'
    )


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


def test_get_review_context_minimal_uses_relative_key_entities(monkeypatch) -> None:
    changed_node = SimpleNamespace(
        qualified_name="/repo/dagayn/tools/_common.py",
        name="/repo/dagayn/tools/_common.py",
        kind="File",
        is_test=False,
    )

    class _Store(_Closable):
        def get_impact_radius(self, abs_files, max_depth):
            return {
                "changed_nodes": [changed_node],
                "impacted_nodes": [],
                "impacted_files": [],
                "edges": [],
            }

    monkeypatch.setattr(
        "dagayn.tools.review_context._get_store",
        lambda repo_root: (_Store(), Path("/repo")),
    )

    result = get_review_context(
        changed_files=["dagayn/tools/_common.py"],
        include_source=False,
        detail_level="minimal",
        repo_root="/repo",
    )

    assert result["status"] == "ok"
    assert result["key_entities"] == ["dagayn/tools/_common.py"]


def test_list_repos_has_hints(monkeypatch) -> None:
    class _Registry:
        def list_repos(self):
            return [{"alias": "dagayn", "path": "/repo"}]

    monkeypatch.setattr("dagayn.registry.Registry", _Registry)

    result = list_repos_func()

    assert result["status"] == "ok"
    assert result["repos"] == [{"alias": "dagayn", "path": "/repo"}]
    assert result["_hints"]["next_steps"][0]["tool"] == "cross_repo_search_tool"


def test_refactor_dead_code_truncates(monkeypatch) -> None:
    dead = [{"qualified_name": f"/repo/a.py::fn_{idx}"} for idx in range(5)]

    monkeypatch.setattr(
        "dagayn.tools.refactor_tools._get_store",
        lambda repo_root: (_Closable(), None),
    )
    monkeypatch.setattr(
        "dagayn.refactor.dead_code_report",
        lambda store, **kwargs: {
            "dead": dead,
            "suppressed": {"name_referenced_in_source": 3},
            "verification": {"status": "complete", "files_scanned": 1, "files_skipped": 0},
        },
    )

    result = refactor_func(mode="dead_code", top_n=2, repo_root="/repo")

    assert result["status"] == "ok"
    assert result["total"] == 5
    assert result["truncated"] is True
    assert len(result["dead_code"]) == 2
    assert result["suppressed"] == {"name_referenced_in_source": 3}
    assert "Left out 3 graph candidate(s)" in result["summary"]
    assert result["missingness"]


def test_refactor_suggest_truncates(monkeypatch) -> None:
    suggestions = [
        {
            "type": "remove",
            "symbols": [f"/repo/a.py::fn_{idx}"],
            "work_pack": {"estimated_size": "small"},
        }
        for idx in range(4)
    ]

    class _SuggestingStore(_Closable):
        def ranked_suggestions_json(self) -> str:
            return json.dumps(suggestions)

    monkeypatch.setattr(
        "dagayn.tools.refactor_tools._get_store",
        lambda repo_root: (_SuggestingStore(), None),
    )

    result = refactor_func(mode="suggest", top_n=2, repo_root="/repo")

    assert result["status"] == "ok"
    assert result["total"] == 4
    assert result["truncated"] is True
    assert len(result["suggestions"]) == 2
    assert result["guidance"]
    assert result["missingness"]


def test_flow_tool_runtime_error_has_missingness(monkeypatch) -> None:
    from dagayn.tools import flows_tools

    def _boom(repo_root):
        raise ValueError("graph unavailable")

    monkeypatch.setattr(flows_tools, "_get_store", _boom)

    result = flows_tools.list_flows(repo_root="/repo")

    assert result["status"] == "error"
    assert result["error"] == "graph unavailable"
    assert result["missingness"][0]["reason_code"] == "tool_runtime_error"


def test_list_repos_runtime_error_has_missingness(monkeypatch) -> None:
    from dagayn.tools import registry_tools

    def _fail_list_repos(self):
        raise ValueError("registry unavailable")

    monkeypatch.setattr("dagayn.registry.Registry.list_repos", _fail_list_repos)

    result = registry_tools.list_repos_func()

    assert result["status"] == "error"
    assert result["error"] == "registry unavailable"
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
