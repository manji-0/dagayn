"""Python tool bodies that defer to the Rust implementation answer through it
once ``_get_store`` has resolved and opened the graph."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest

from dagayn.tools import docs
from dagayn.tools.analysis_tools import get_suggested_questions_func
from dagayn.tools.flow_dispatcher import flow_func
from dagayn.tools.query import find_large_functions, list_graph_stats
from dagayn.tools.refactor_tools import refactor_func

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


def _reference(repo: Path) -> None:
    (repo / "docs").mkdir()
    (repo / "docs" / "LLM-OPTIMIZED-REFERENCE.md").write_text(
        '<section name="usage">from the repo</section>\n', encoding="utf-8"
    )


def test_docs_section_answers_through_rust(repo: Path) -> None:
    _reference(repo)
    result = docs.get_docs_section("usage", repo_root=str(repo))
    assert result["status"] == "ok"
    assert result["content"] == "from the repo"
    assert result["_repo"]["repo_root"] == str(repo.resolve())
    # A section only the package's reference holds.
    trust = docs.get_docs_section("trust", repo_root=str(repo), max_chars=10)
    assert trust["truncated"] is True
    assert trust["content"].endswith("\n... (truncated)")


def test_docs_section_without_a_graph_reads_the_root_and_package(
    repo: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Python answers without a graph, from the validated root and the
    package's reference, where the Rust tool needs the graph for `_repo`."""

    def no_graph(repo_root: str | None) -> None:
        raise RuntimeError("no graph")

    monkeypatch.setattr(docs, "_get_store", no_graph)
    _reference(repo)
    assert docs.get_docs_section("usage", repo_root=str(repo)) == {
        "status": "ok",
        "section": "usage",
        "content": "from the repo",
        "truncated": False,
    }
    assert docs.get_docs_section("trust")["status"] == "ok"
    missing = docs.get_docs_section("no-such-section")
    assert missing["status"] == "not_found"
    assert "trust" in missing["error"]


def test_wiki_page_answers_through_rust(repo: Path) -> None:
    wiki = repo / ".dagayn" / "wiki"
    wiki.mkdir()
    (wiki / "auth-flow.md").write_bytes(b"# Auth\r\nbad \xff byte\r")
    (repo / ".dagayn" / "secret.md").write_text("outside the wiki\n")

    page = docs.get_wiki_page_func("Auth  Flow!", repo_root=str(repo))
    assert page["status"] == "ok"
    assert page["content"] == "# Auth\nbad � byte\n"
    assert page["summary"] == "Wiki page for 'Auth  Flow!' (18 chars)"
    assert page["_repo"]["repo_root"] == str(repo.resolve())
    # The exact file name, and the Kelvin sign `str.lower()` folds to `k`.
    assert docs.get_wiki_page_func("auth-flow.md", repo_root=str(repo))["status"] == "ok"
    (wiki / "k.md").write_text("kelvin\n")
    assert docs.get_wiki_page_func("K", repo_root=str(repo))["content"] == "kelvin\n"

    for name in ("missing", "../secret.md", "../../etc/passwd", "認証"):
        missing = docs.get_wiki_page_func(name, repo_root=str(repo))
        assert missing["status"] == "not_found", name
        assert missing["next_tool_suggestions"] == [
            "generate_wiki_tool -- build wiki pages from communities"
        ]


@pytest.fixture
def unused_repo(tmp_path: Path) -> Path:
    """Three functions nothing refers to, for dead code and remove suggestions."""
    root = tmp_path / "unused"
    (root / ".git").mkdir(parents=True)
    (root / "lib.py").write_text(
        "def orphan_one():\n    pass\n\n\ndef orphan_two():\n    pass\n\n\n"
        "def orphan_three():\n    pass\n"
    )
    subprocess.run([DAGAYN, "build", "--repo", root], check=True, capture_output=True)
    return root


def test_refactor_dead_code_answers_through_rust(unused_repo: Path) -> None:
    result = refactor_func(mode="dead_code", top_n=2, repo_root=str(unused_repo))
    assert result["status"] == "ok"
    assert result["total"] == 3
    assert result["truncated"] is True
    assert len(result["dead_code"]) == 2
    assert result["summary"].endswith("Showing first 2.")
    assert result["verification"]["status"] == "complete"
    assert any(
        item["reason_code"] == "absence_evidence_requires_manual_verification"
        for item in result["missingness"]
    )


def test_refactor_suggest_answers_through_rust(unused_repo: Path) -> None:
    result = refactor_func(mode="suggest", top_n=1, repo_root=str(unused_repo))
    assert result["status"] == "ok"
    assert result["total"] >= 1
    assert len(result["suggestions"]) == 1
    assert set(result["guidance"][0]) >= {
        "claim",
        "evidence",
        "confidence",
        "missingness",
        "action",
        "reason_codes",
        "counts",
    }
    assert result["_hints"]["next_steps"]


def test_refactor_rename_stays_in_python_for_non_ascii_names(repo: Path) -> None:
    result = refactor_func(mode="rename", old_name="helper", new_name="hélper", repo_root=str(repo))
    assert result["status"] == "ok", result
    assert result["edits"]
