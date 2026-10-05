"""Python tool bodies that defer to the Rust implementation answer through it
once ``_get_store`` has resolved and opened the graph."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest

from dagayn.tools.analysis_tools import get_suggested_questions_func
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
