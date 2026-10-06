"""Fixture checks for eval/run_review_eval.py (the review_tool findings eval).

The fast tests load every case.yaml under tests/fixtures/review_eval and
materialise it into a git repository without building a graph, so a broken
fixture fails CI. The full harness run (graph build + review_tool per case)
is opt-in: set DAGAYN_REVIEW_EVAL=1.
"""

from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest

_SCRIPT = Path(__file__).resolve().parents[1] / "eval" / "run_review_eval.py"


def _load_harness():
    spec = importlib.util.spec_from_file_location("run_review_eval", _SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


harness = _load_harness()
CASE_PATHS = harness.case_paths()


def _git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=repo, capture_output=True, text=True, check=True
    ).stdout


def test_fixture_set_covers_every_kind_and_both_modes():
    cases = harness.load_cases()
    expected_kinds = {exp.kind for case in cases for exp in case.expected}
    assert expected_kinds == set(harness.FINDING_KINDS)
    assert {case.commit_change for case in cases} == {True, False}
    assert any(case.negative for case in cases)
    assert len({case.name for case in cases}) == len(cases)


@pytest.mark.parametrize("case_path", CASE_PATHS, ids=lambda path: path.parent.name)
def test_case_loads_and_materialises(case_path: Path, tmp_path: Path):
    case = harness.load_case(case_path)
    repo = tmp_path / case.name
    base = harness.materialize(case, repo)

    assert base == ("HEAD~1" if case.commit_change else "HEAD")
    commits = _git(repo, "rev-list", "--count", "HEAD").strip()
    status = _git(repo, "status", "--porcelain").strip()
    if case.commit_change:
        assert commits == "2"
        assert status == ""
        changed = _git(repo, "diff", "--name-only", "HEAD~1", "HEAD").split()
    else:
        assert commits == "1"
        assert status != ""
        changed = _git(repo, "diff", "--name-only", "HEAD").split()
        changed += _git(repo, "ls-files", "--others", "--exclude-standard").split()
    assert changed, "the change must touch at least one file"

    # Every expected target names a file that exists on one side of the change.
    base_files = set(case.base_files)
    final_files = set(_git(repo, "ls-files").split()) | set(
        _git(repo, "ls-files", "--others", "--exclude-standard").split()
    )
    known = base_files | final_files
    for exp in case.expected:
        for target in exp.targets:
            file_part = target.split("::", 1)[0]
            assert file_part in known or any(path.startswith(file_part + "/") for path in known), (
                f"{case.name}: target {target!r} names no file in the fixture"
            )


def test_case_loader_rejects_malformed_cases(tmp_path: Path):
    bad = tmp_path / "bad" / "case.yaml"
    bad.parent.mkdir()
    bad.write_text(
        "negative: true\nbase: {files: {a.py: 'x = 1\\n'}}\nchange: {files: {a.py: 'x = 2\\n'}}\n"
        "expected_findings: [{kind: dangling_reference, target: a.py}]\n"
    )
    with pytest.raises(harness.CaseError, match="negative case cannot expect"):
        harness.load_case(bad)

    bad.write_text(
        "base: {files: {a.py: 'x = 1\\n'}}\nchange: {files: {a.py: 'x = 2\\n'}}\n"
        "expected_findings: [{kind: risk_level, target: a.py}]\n"
    )
    with pytest.raises(harness.CaseError, match="needs a kind"):
        harness.load_case(bad)


def _case(expected, allowed=()):
    return harness.Case(
        name="synthetic",
        path=Path("synthetic/case.yaml"),
        description="",
        negative=not expected,
        commit_change=True,
        allowed_kinds=frozenset(allowed),
        base_files={"a.py": ""},
        change_files={"a.py": "x = 1\n"},
        change_patch=None,
        expected=tuple(harness.Expected(kind=k, targets=tuple(t)) for k, t in expected),
    )


def test_scoring_matches_targets_and_counts_errors():
    case = _case(
        [
            ("dangling_reference", ["pkg/util.py::legacy", "pkg/app.py::run"]),
            ("tests_to_run", ["tests/test_util.py::test_helper"]),
        ]
    )
    findings = [
        {"kind": "dangling_reference", "qualified_name": "pkg/app.py::run", "file": "pkg/app.py"},
        {"kind": "dangling_reference", "qualified_name": "pkg/other.py::x"},
        {"kind": "untested_change", "file": "pkg/util.py"},
    ]
    scored = harness.score_case(case, findings)
    assert scored["per_kind"]["dangling_reference"] == {"tp": 1, "fp": 1, "fn": 0}
    assert scored["per_kind"]["tests_to_run"] == {"tp": 0, "fp": 0, "fn": 1}
    assert scored["per_kind"]["untested_change"] == {"tp": 0, "fp": 1, "fn": 0}

    negative = _case([], allowed=["tests_to_run"])
    quiet = harness.score_case(negative, [{"kind": "tests_to_run", "qualified_name": "t"}])
    assert quiet["quiet"] and quiet["allowed_findings"] == 1


def test_missing_findings_field_scores_as_zero_recall():
    payload, size, error = harness.parse_tool_output('log line\n{"status": "ok", "risk": 1}\n')
    assert error is None and size == len('{"status": "ok", "risk": 1}')
    findings, present = harness.extract_findings(payload)
    assert findings == [] and not present

    row = {
        "case": "c",
        "negative": False,
        "output_chars": size,
        "findings_field_present": present,
        **harness.score_case(_case([("bridge_touched", ["rust/Cargo.toml"])]), findings),
    }
    summary = harness.summarize([row])
    assert summary["kinds"]["bridge_touched"]["recall"] == 0.0
    assert summary["kinds"]["bridge_touched"]["precision"] is None


def test_gate_only_fails_gated_kinds_below_floor():
    summary: dict[str, Any] = {
        "kinds": {kind: {"precision": 0.5, "recall": 1.0} for kind in harness.FINDING_KINDS}
    }
    ungated = {kind: {"gated": False, "precision_floor": 0.8} for kind in harness.FINDING_KINDS}
    assert harness.gate_failures(summary, ungated) == []
    gated = dict(ungated, tests_to_run={"gated": True, "precision_floor": 0.8})
    assert harness.gate_failures(summary, gated) == ["tests_to_run: precision 0.5 < 0.8"]
    summary["kinds"]["tests_to_run"]["precision"] = None
    assert harness.gate_failures(summary, gated) == []


def test_shipped_thresholds_gate_every_kind():
    thresholds = harness.load_thresholds()
    assert set(thresholds) == set(harness.FINDING_KINDS)
    assert all(
        conf["gated"] and conf["precision_floor"] == 0.8 and conf["recall_floor"] == 0.8
        for conf in thresholds.values()
    )


@pytest.mark.skipif(
    not os.environ.get("DAGAYN_REVIEW_EVAL"),
    reason="full review eval builds a graph per case; set DAGAYN_REVIEW_EVAL=1",
)
def test_full_review_eval_runs(tmp_path: Path):
    cases = harness.load_cases()
    result = harness.run_eval(cases, dagayn_cmd=["uv", "run", "dagayn"], work_dir=tmp_path)
    assert result["summary"]["case_count"] == len(cases)
    assert result["summary"]["error_count"] == 0, [
        row["error"] for row in result["rows"] if row.get("error")
    ]
    assert result["gate_failures"] == [], result["gate_failures"]
