"""Python tool bodies that defer to the Rust implementation answer through it
once ``_get_store`` has resolved and opened the graph."""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

import pytest

from dagayn.tools import docs
from dagayn.tools.analysis_tools import get_suggested_questions_func
from dagayn.tools.flow_dispatcher import flow_func
from dagayn.tools.query import find_large_functions, list_graph_stats
from dagayn.tools.refactor_tools import apply_refactor_func, refactor_func

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


@pytest.fixture
def rename_repo(repo: Path) -> Path:
    """``repo`` with one module importing ``helper`` and one importing ``main``."""
    (repo / "consumer.py").write_text(
        "from app import helper\n\n\ndef use():\n    return helper()\n"
    )
    (repo / "other.py").write_text("from app import main\n\n\ndef go():\n    return main()\n")
    subprocess.run([DAGAYN, "build", "--repo", repo], check=True, capture_output=True)
    return repo


def test_refactor_rename_previews_through_rust(rename_repo: Path) -> None:
    from dagayn.refactor import _pending_refactors

    result = refactor_func(
        mode="rename", old_name="helper", new_name="assist", repo_root=str(rename_repo)
    )
    assert result["status"] == "ok", result
    refactor_id = result["refactor_id"]
    assert len(refactor_id) == 8
    assert result["type"] == "rename"
    assert (result["old_name"], result["new_name"]) == ("helper", "assist")
    assert result["target"]["name"] == "helper"
    assert result["target"]["kind"] == "Function"
    assert (result["ambiguous"], result["candidate_count"]) == (False, 1)
    sites = {(Path(e["file"]).name, e["line"], e["source"]) for e in result["edits"]}
    assert ("app.py", 5, "definition") in sites
    assert ("app.py", 2, "call") in sites
    # The import line that names the symbol, not the one importing `main`.
    assert ("consumer.py", 1, "import") in sites
    assert not any(name == "other.py" for name, _, _ in sites)
    assert result["stats"]["high"] >= 3
    assert "rename_edits_graph_limited" in {m["reason_code"] for m in result["missingness"]}
    assert result["next_tool_suggestions"][0].startswith(
        f"apply_refactor_tool(refactor_id='{refactor_id}', dry_run=true)"
    )
    assert result["_repo"]["repo_root"] == str(rename_repo.resolve())
    # The preview waits in the store apply_refactor_tool reads.
    assert _pending_refactors[refactor_id]["edits"] == result["edits"]
    del _pending_refactors[refactor_id]

    missing = refactor_func(
        mode="rename", old_name="no_such_symbol_zz", new_name="x", repo_root=str(rename_repo)
    )
    assert missing["status"] == "not_found"
    assert missing["summary"] == "No node found matching 'no_such_symbol_zz' in the current graph."
    assert "rename_target_not_found_in_graph" in {m["reason_code"] for m in missing["missingness"]}


_IDENTIFIER = re.compile(r"^[^\W\d]\w*$")


@pytest.mark.parametrize(
    "new_name",
    [
        "renamed_beta",
        "_x",
        "hélper",
        "π",
        "名前",
        "x٣",  # an Arabic-Indic digit after the first character
        "Ⅰx",  # a letter number (Nl) is \w but not \d
        "x²",  # a superscript digit (No) is \w
        "abc\n",  # `$` matches before a final newline
        "1 bad name",
        "has-dash",
        "٣x",  # \d first
        "é",  # a combining mark is not \w
        "x‿",  # an undertie (Pc) is not \w
        "x​",  # repr escapes what is not printable
        " x",
        "x\U000e0001",
        "it's",
    ],
)
def test_refactor_rename_checks_identifiers_as_python_re_does(repo: Path, new_name: str) -> None:
    result = refactor_func(mode="rename", old_name="helper", new_name=new_name, repo_root=str(repo))
    if _IDENTIFIER.match(new_name):
        assert result["status"] == "ok", result
        assert result["edits"][0]["new"] == new_name
    else:
        assert result["status"] == "error"
        assert result["error"] == f"new_name is not a valid identifier: {new_name!r}"
        assert (result["old_name"], result["new_name"]) == ("helper", new_name)


@pytest.mark.parametrize(
    "content",
    [
        None,
        b"",
        b"not json",
        b'\xef\xbb\xbf{"repos": []}',
        b'{"repos": []}',
        b'{"other": 1}',
        b'{"repos": [{"path": "/a", "alias": "x"}, 7, null]}',
        b'{"repos": ["\xff"]}',
        b'{"repos": "ab"}',
        b'{"repos": 5}',
        b'{"repos": 1.5}',
        b'{"repos": null}',
        b"[1]",
        b'"x"',
        b"null",
        b"true",
    ],
)
def test_list_repos_answers_through_rust_as_the_registry_does(
    content: bytes | None, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from dagayn.registry import Registry
    from dagayn.tools.registry_tools import list_repos_func

    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    registry = home / ".dagayn" / "registry.json"
    if content is not None:
        registry.parent.mkdir()
        registry.write_bytes(content)

    result = list_repos_func()
    assert registry.parent.is_dir()
    try:
        expected = Registry(registry).list_repos()
    except Exception as exc:
        assert result["status"] == "error", result
        assert result["error"] == str(exc)
        assert result["missingness"][0]["reason_code"] == "tool_runtime_error"
    else:
        assert result["status"] == "ok", result
        assert result["repos"] == expected
        assert result["summary"] == f"{len(expected)} registered repository(ies)."


def test_list_repos_reports_a_registry_directory_it_cannot_create(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from dagayn.registry import Registry
    from dagayn.tools.registry_tools import list_repos_func

    (tmp_path / ".dagayn").write_text("a file where the directory goes")
    monkeypatch.setenv("HOME", str(tmp_path))
    with pytest.raises(OSError) as raised:
        Registry(tmp_path / ".dagayn" / "registry.json")
    assert list_repos_func()["error"] == str(raised.value)


_HINTS = ("CRG_REPO_ROOT", "CURSOR_PROJECT_DIR", "CLAUDE_PROJECT_DIR", "WORKSPACE_FOLDER_PATHS")


def test_apply_refactor_resolves_its_root_as_python_did(
    repo: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    for name in _HINTS:
        monkeypatch.delenv(name, raising=False)
    preview = refactor_func(
        mode="rename", old_name="helper", new_name="assist", repo_root=str(repo)
    )
    refactor_id = preview["refactor_id"]

    # No root: the checkout the working directory is in.
    monkeypatch.chdir(repo)
    dry = apply_refactor_func(refactor_id, dry_run=True)
    assert dry["status"] == "ok", dry
    assert dry["would_modify"] == ["app.py"]
    assert "+def assist():\n" in dry["diffs"]["app.py"]

    # A named root is checked as `_validate_repo_root` does.
    missing = tmp_path / "missing"
    assert apply_refactor_func(refactor_id, repo_root=str(missing)) == {
        "status": "error",
        "error": f"repo_root is not an existing directory: {missing.resolve()}",
    }
    plain = tmp_path / "plain"
    plain.mkdir()
    assert apply_refactor_func(refactor_id, repo_root=str(plain))["error"] == (
        "repo_root does not look like a project root (no .git or .dagayn/graph.db "
        f"found): {plain.resolve()}"
    )
    # A git-backed jj workspace is a project root.
    jj = tmp_path / "jj"
    (jj / ".jj" / "repo" / "store").mkdir(parents=True)
    (jj / ".jj" / "repo" / "store" / "git_target").write_text(str(repo / ".git"))
    assert apply_refactor_func("00000000", repo_root=str(jj)) == {
        "status": "error",
        "error": "Refactor '00000000' not found or expired.",
    }

    # Workspace hints naming two repositories, from outside both.
    other = tmp_path / "other"
    (other / ".git").mkdir(parents=True)
    monkeypatch.setenv("WORKSPACE_FOLDER_PATHS", f"{repo},{other}")
    monkeypatch.chdir(plain)
    ambiguous = apply_refactor_func(refactor_id)
    assert ambiguous["status"] == "error"
    assert ambiguous["error"].startswith("workspace hints name more than one repository")

    # The write: CRLF line ends come back as `\n`, as `read_text` and
    # `write_text` left them.
    monkeypatch.delenv("WORKSPACE_FOLDER_PATHS")
    (repo / "app.py").write_bytes(
        b"def main():\r\n    return helper()\r\n\r\n\r\ndef helper():\r\n"
    )
    applied = apply_refactor_func(refactor_id, repo_root=str(repo))
    assert applied["status"] == "ok", applied
    assert applied["files_modified"] == [str((repo / "app.py").resolve())]
    assert (
        repo / "app.py"
    ).read_bytes() == b"def main():\n    return assist()\n\n\ndef assist():\n"
