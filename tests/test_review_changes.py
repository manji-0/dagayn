"""Change analysis behind ``review_tool(mode="changes")``, end to end.

The analysis runs in Rust (``crates/dagayn-tools/src/changes.rs``); these
tests drive it through :func:`review_func` on real git repositories with a
built graph: diff-range attribution, renames, unmapped files, test gaps,
affected flows, and review-priority scoring.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest

from dagayn.tools.review_dispatcher import review_func

DAGAYN = Path(sys.executable).with_name("dagayn")
GIT = [
    "git",
    "-c",
    "user.name=t",
    "-c",
    "user.email=t@example.invalid",
    "-c",
    "commit.gpgsign=false",
]

THREE = "def alpha():\n    pass\n\n\ndef beta():\n    pass\n\n\ndef gamma():\n    pass\n"


def _git(root: Path, *args: str) -> None:
    subprocess.run([*GIT, *args], cwd=root, check=True, capture_output=True)


def _write(root: Path, files: dict[str, str]) -> None:
    for name, text in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")


def _commit(root: Path, message: str = "commit") -> None:
    _git(root, "add", "-A")
    _git(root, "commit", "-q", "-m", message)


def _build(root: Path) -> None:
    subprocess.run([DAGAYN, "build", "--repo", root], check=True, capture_output=True)


def _repo(tmp_path: Path, files: dict[str, str]) -> Path:
    """A one-commit repository holding *files*, with a built graph."""
    root = tmp_path / "repo"
    root.mkdir()
    _git(root, "init", "-q")
    _write(root, files)
    _commit(root, "init")
    _build(root)
    return root


def _review(root: Path, **kwargs: Any) -> dict[str, Any]:
    kwargs.setdefault("base", "HEAD")
    result = review_func(mode="changes", repo_root=str(root), detail_level="verbose", **kwargs)
    assert result["status"] == "ok", result["summary"]
    return result


def _names(result: dict[str, Any]) -> set[str]:
    return {f["name"] for f in result["changed_functions"]}


def _scores(result: dict[str, Any]) -> dict[str, float]:
    return {f["name"]: f["risk_score"] for f in result["changed_functions"]}


# ---------------------------------------------------------------------------
# Diff-range attribution
# ---------------------------------------------------------------------------


def test_only_functions_overlapping_the_diff_are_changed(tmp_path: Path) -> None:
    root = _repo(tmp_path, {"app.py": THREE, "other.py": THREE})
    _write(root, {"app.py": THREE.replace("def beta():\n    pass", "def beta():\n    return 1")})

    result = _review(root)

    assert result["diff_parse_status"] == "ok"
    assert [f["qualified_name"] for f in result["changed_functions"]] == ["app.py::beta"]
    assert result["changed_files"] == ["app.py"]
    assert result["unmapped_changed_files"] == []


def test_two_hunks_in_one_function_count_once_and_files_are_separate(tmp_path: Path) -> None:
    body = "def big():\n" + "".join(f"    x{i} = {i}\n" for i in range(12)) + "    return x0\n"
    root = _repo(tmp_path, {"a.py": body, "b.py": THREE})
    _write(
        root,
        {
            "a.py": body.replace("x1 = 1", "x1 = 10").replace("x10 = 10", "x10 = 100"),
            "b.py": THREE.replace("def gamma():\n    pass", "def gamma():\n    return 3"),
        },
    )

    result = _review(root)

    assert sorted(f["qualified_name"] for f in result["changed_functions"]) == [
        "a.py::big",
        "b.py::gamma",
    ]


def test_an_untracked_file_counts_as_a_whole_file_change(tmp_path: Path) -> None:
    root = _repo(tmp_path, {"app.py": THREE})
    _write(root, {"fresh.py": "def one():\n    pass\n\n\ndef two():\n    pass\n"})
    _build(root)

    result = _review(root)

    assert result["change_file_source_counts"]["untracked"] == 1
    assert {"one", "two"} <= _names(result)


def test_a_changed_file_outside_the_graph_is_reported_unmapped(tmp_path: Path) -> None:
    root = _repo(tmp_path, {"app.py": THREE})

    result = _review(root, changed_files=["missing.py"])

    assert result["changed_functions"] == []
    assert result["unmapped_changed_files"] == ["missing.py"]
    assert "unmapped_changed_files" in result["attribution"]["reason_codes"]


def test_a_renamed_file_maps_to_the_graph_s_old_path(tmp_path: Path) -> None:
    """A graph indexed before a rename still attributes the renamed file's
    change: ``git diff --name-status`` names the old path."""
    root = _repo(tmp_path, {"src/app.py": "def alpha():\n    return 1\n"})
    (root / "src" / "app.py").rename(root / "src" / "renamed.py")
    _write(root, {"src/renamed.py": "def alpha():\n    return 2\n"})
    _commit(root, "rename")

    result = _review(root, base="HEAD~1", changed_files=["src/renamed.py"])

    assert result["unmapped_changed_files"] == []
    assert [f["qualified_name"] for f in result["changed_functions"]] == ["src/app.py::alpha"]


@pytest.mark.xfail(
    strict=True,
    reason=(
        "stale_line_range_files never fires from the review path: the changed "
        "path is absolute while the graph's file_hash rows are keyed by the "
        "repo-relative path, so line ranges are trusted even when the indexed "
        "content is stale (the retired Python analyze_changes behaved the same)"
    ),
)
def test_stale_indexed_line_ranges_degrade_to_the_whole_file(tmp_path: Path) -> None:
    root = _repo(tmp_path, {"app.py": THREE})
    # Prepend a function without re-indexing: the graph's line ranges are stale.
    _write(root, {"app.py": "def delta():\n    pass\n\n\n" + THREE})

    result = _review(root)

    assert result["attribution"]["stale_line_range_files"] == ["app.py"]
    assert "stale_graph_line_ranges" in result["attribution"]["reason_codes"]
    assert {"alpha", "beta", "gamma"} <= _names(result)


# ---------------------------------------------------------------------------
# Test gaps
# ---------------------------------------------------------------------------


def test_functions_without_tests_are_test_gaps(tmp_path: Path) -> None:
    app = (
        "def untested_a():\n    return 1\n\n\ndef untested_b():\n    return 2\n\n\n"
        "def tested_c():\n    return 3\n"
    )
    root = _repo(
        tmp_path,
        {
            "app.py": app,
            "tests/test_app.py": "from app import tested_c\n\n\ndef test_c():\n    tested_c()\n",
        },
    )
    _write(root, {"app.py": app.replace("return", "return 10 +")})

    result = _review(root)

    assert _names(result) == {"untested_a", "untested_b", "tested_c"}
    assert {gap["name"] for gap in result["test_gaps"]} == {"untested_a", "untested_b"}
    assert {gap["coverage_confidence"] for gap in result["test_gaps"]} == {"none"}
    untested = next(f for f in result["findings"] if f["kind"] == "untested_change")
    assert sorted(untested["targets"]) == ["app.py::untested_a", "app.py::untested_b"]


def test_a_test_named_for_the_function_suppresses_the_gap(tmp_path: Path) -> None:
    """Naming heuristics (a test module and class named for the target) count
    as coverage even with no TESTED_BY edge."""
    root = _repo(
        tmp_path,
        {
            "pkg/context.py": "def get_minimal_context():\n    return {}\n",
            "tests/test_context.py": (
                "class TestGetMinimalContext:\n    def test_shape(self):\n        assert True\n"
            ),
        },
    )
    _write(root, {"pkg/context.py": "def get_minimal_context():\n    return {'a': 1}\n"})

    result = _review(root)

    assert _names(result) == {"get_minimal_context"}
    assert result["test_gaps"] == []
    assert result["test_gap_evidence"]["heuristic_suppression_enabled"] is True
    assert result["test_gap_evidence"]["heuristic_truncated"] is False


# ---------------------------------------------------------------------------
# Affected flows and review-priority scores
# ---------------------------------------------------------------------------

ROUTES = "from services import service\n\n\ndef handler():\n    return service()\n"
SERVICES = (
    "def service():\n    return 1\n\n\n"
    "def process_data():\n    return 2\n\n\n"
    "def verify_auth_token():\n    return 3\n\n\n"
    "def design_doc():\n    return 4\n\n\n"
    "def verify_signature():\n    return 5\n"
)


@pytest.fixture
def services_change(tmp_path: Path) -> dict[str, Any]:
    root = _repo(tmp_path, {"routes.py": ROUTES, "services.py": SERVICES})
    _write(root, {"services.py": SERVICES.replace("return", "return 10 +")})
    return _review(root)


def test_a_change_inside_a_flow_lists_the_flow(services_change: dict[str, Any]) -> None:
    flows = services_change["affected_flows"]
    assert [flow["name"] for flow in flows] == ["handler"]
    assert [step["qualified_name"] for step in flows[0]["changed_steps"]] == [
        "services.py::service"
    ]
    assert flows[0]["changed_steps"][0]["file"] == "services.py"
    assert services_change["affected_flow_count"] == 1


def test_review_priority_scores_weigh_flows_and_security_names(
    services_change: dict[str, Any],
) -> None:
    scores = _scores(services_change)
    assert all(0.0 <= score <= 1.0 for score in scores.values())
    # Security keywords match at identifier-token starts only: ``sign`` in
    # ``design`` adds nothing, ``signature`` adds the 0.20 security weight.
    assert scores["verify_auth_token"] > scores["process_data"]
    assert scores["verify_signature"] == pytest.approx(scores["design_doc"] + 0.20)
    # Flow membership raises the score.
    assert scores["service"] > scores["process_data"]
    priorities = [p["risk_score"] for p in services_change["review_priorities"]]
    assert priorities == sorted(priorities, reverse=True)
    assert services_change["risk_score"] == max(scores.values())


def test_review_priority_scores_weigh_callers(tmp_path: Path) -> None:
    files = {"lib.py": "def popular():\n    return 1\n\n\ndef lonely():\n    return 2\n"}
    for i in range(10):
        files[f"c{i}.py"] = (
            f"from lib import popular\n\n\ndef caller_{i}():\n    return popular()\n"
        )
    root = _repo(tmp_path, files)
    _write(root, {"lib.py": files["lib.py"].replace("return", "return 10 +")})

    scores = _scores(_review(root))

    assert scores["popular"] > scores["lonely"]
