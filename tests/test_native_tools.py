"""Python tool bodies that defer to the Rust implementation answer through it
once ``_get_store`` has resolved and opened the graph."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest

from dagayn.tools.analysis_tools import get_suggested_questions_func
from dagayn.tools.flow_dispatcher import flow_func
from dagayn.tools.query import find_large_functions, list_graph_stats

DAGAYN = Path(sys.executable).with_name("dagayn")


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    root = tmp_path / "repo"
    (root / ".git").mkdir(parents=True)
    (root / "app.py").write_text("def main():\n    return helper()\n\n\ndef helper():\n    pass\n")
    subprocess.run([DAGAYN, "build", "--repo", root], check=True, capture_output=True)
    return root


def test_list_graph_stats_answers_through_rust(repo: Path) -> None:
    result = list_graph_stats(repo_root=str(repo))
    assert result["status"] == "ok"
    assert result["files_count"] == 1
    assert result["nodes_by_kind"]["Function"] == 2
    assert result["embeddings_count"] == 0
    assert result["summary"].startswith("Graph stats for repo: ")
    assert result["_hints"]["next_steps"][0]["tool"] == "architecture_analysis_tool"


def test_a_missing_graph_is_created_before_rust_reads_it(tmp_path: Path) -> None:
    root = tmp_path / "fresh"
    (root / ".git").mkdir(parents=True)
    result = list_graph_stats(repo_root=str(root))
    assert result["status"] == "ok"
    assert result["total_nodes"] == 0
    assert (root / ".dagayn" / "graph.db").is_file()


def test_find_large_functions_answers_through_rust(repo: Path) -> None:
    result = find_large_functions(min_lines=1, kind="Function", repo_root=str(repo))
    assert result["status"] == "ok"
    assert result["total_found"] == 2
    assert {row["name"] for row in result["results"]} == {"main", "helper"}


def test_suggested_questions_answer_through_rust(repo: Path) -> None:
    result = get_suggested_questions_func(repo_root=str(repo), top_n=5)
    assert result["status"] == "ok"
    assert isinstance(result["questions"], list)
    assert result["guidance"][0]["reason_codes"] == ["suggested_questions"]


def test_flow_tool_answers_through_rust(repo: Path) -> None:
    listed = flow_func(mode="list", repo_root=str(repo))
    assert listed["status"] == "ok"
    assert listed["called_subtool"] == "list_flows"
    assert listed["flows"], listed["summary"]
    flow_id = listed["flows"][0]["id"]

    got = flow_func(mode="get", flow_id=flow_id, include_source=True, repo_root=str(repo))
    assert got["status"] == "ok"
    assert got["called_subtool"] == "get_flow"
    assert got["flow"]["id"] == flow_id
    assert any("source" in step for step in got["flow"]["steps"])

    missing = flow_func(mode="get", flow_name="no_such_flow", repo_root=str(repo))
    assert missing["status"] == "not_found"


def test_flow_tool_rejects_an_invalid_request_before_rust(repo: Path) -> None:
    result = flow_func(mode="get", repo_root=str(repo))
    assert result["status"] == "error"
    assert "flow_id or flow_name" in result["summary"]
