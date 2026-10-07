"""Fixture checks for eval/run_refactor_eval.py (the refactor findings eval).

The fast tests load every case.yaml under tests/fixtures/refactor_eval
and materialise it into a git repository without building a graph, so a
broken fixture fails CI. The full harness run (graph build + suggest per
case) is opt-in: set DAGAYN_REFACTOR_EVAL=1.
"""

from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
from pathlib import Path

import pytest

_SCRIPT = Path(__file__).resolve().parents[1] / "eval" / "run_refactor_eval.py"


def _load_harness():
    spec = importlib.util.spec_from_file_location("run_refactor_eval", _SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


harness = _load_harness()
CASE_PATHS = harness.case_paths()


def test_fixture_set_covers_every_kind():
    cases = harness.load_cases()
    expected_kinds = {exp.kind for case in cases for exp in case.expected}
    assert expected_kinds == set(harness.FINDING_KINDS)
    assert any(case.negative for case in cases)
    assert any(case.commits for case in cases)
    assert len({case.name for case in cases}) == len(cases)


@pytest.mark.parametrize("case_path", CASE_PATHS, ids=lambda path: path.parent.name)
def test_case_loads_and_materialises(case_path: Path, tmp_path: Path):
    case = harness.load_case(case_path)
    repo = tmp_path / case.name
    harness.materialize(case, repo)
    tracked = set(
        subprocess.run(
            ["git", "ls-files"], cwd=repo, capture_output=True, text=True, check=True
        ).stdout.split()
    )
    assert tracked == set(case.files)


def test_case_loader_rejects_malformed_cases(tmp_path: Path):
    bad = tmp_path / "bad" / "case.yaml"
    bad.parent.mkdir()
    bad.write_text(
        "negative: true\nfiles: {a.py: 'x = 1\\n'}\n"
        "expected_findings: [{kind: unused_symbol, target: a.py}]\n"
    )
    with pytest.raises(harness.CaseError, match="negative case cannot expect"):
        harness.load_case(bad)
    bad.write_text("files: {a.py: 'x = 1\\n'}\nexpected_findings: [{kind: hubs, target: a.py}]\n")
    with pytest.raises(harness.CaseError, match="needs a kind"):
        harness.load_case(bad)


def _case(expected):
    return harness.Case(
        name="synthetic",
        path=Path("synthetic/case.yaml"),
        description="",
        negative=not expected,
        files={"a.py": ""},
        expected=tuple(harness.Expected(kind=k, targets=tuple(t)) for k, t in expected),
    )


def test_scoring_matches_grouped_targets_and_counts_errors():
    case = _case(
        [("unused_symbol", ["pkg/a.py::old", "pkg/a.py"]), ("complex_hotspot", ["pkg/b.py::run"])]
    )
    scored = harness.score_case(
        case,
        [
            {"kind": "unused_symbol", "qualified_name": "pkg/a.py::old"},
            {"kind": "undocumented_surface", "qualified_name": "pkg/util.py::helper"},
        ],
    )
    assert scored["per_kind"] == {
        "unused_symbol": {"tp": 1, "fp": 0, "fn": 0},
        "undocumented_surface": {"tp": 0, "fp": 1, "fn": 0},
        "complex_hotspot": {"tp": 0, "fp": 0, "fn": 1},
    }
    assert scored["missed"] == [{"kind": "complex_hotspot", "target": "pkg/b.py::run"}]


def test_commits_become_history(tmp_path: Path):
    case = harness.load_case(
        harness.CASES_DIR / "pos_hotspot_long_function_keeps_changing" / "case.yaml"
    )
    harness.materialize(case, tmp_path / "repo")
    count = subprocess.run(
        ["git", "rev-list", "--count", "HEAD"],
        cwd=tmp_path / "repo",
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    assert int(count) == 1 + len(case.commits)


@pytest.mark.skipif(
    not os.environ.get("DAGAYN_REFACTOR_EVAL"),
    reason="full refactor eval builds a graph per case; set DAGAYN_REFACTOR_EVAL=1",
)
def test_full_refactor_eval_runs(tmp_path: Path):
    cases = harness.load_cases()
    result = harness.run_eval(cases, dagayn_cmd=["uv", "run", "dagayn"], work_dir=tmp_path)
    assert result["summary"]["case_count"] == len(cases)
    assert result["gate_failures"] == [], harness.format_report(result)
