"""Fixture checks for eval/run_architecture_eval.py (the architecture findings eval).

The fast tests load every case.yaml under tests/fixtures/architecture_eval
and materialise it into a git repository without building a graph, so a
broken fixture fails CI. The full harness run (graph build + overview per
case) is opt-in: set DAGAYN_ARCHITECTURE_EVAL=1.
"""

from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
from pathlib import Path

import pytest

_SCRIPT = Path(__file__).resolve().parents[1] / "eval" / "run_architecture_eval.py"


def _load_harness():
    spec = importlib.util.spec_from_file_location("run_architecture_eval", _SCRIPT)
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
    assert any(case.expected_units for case in cases)
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
        "expected_findings: [{kind: import_cycle, target: a.py}]\n"
    )
    with pytest.raises(harness.CaseError, match="negative case cannot expect"):
        harness.load_case(bad)
    bad.write_text("files: {a.py: 'x = 1\\n'}\nexpected_findings: [{kind: hubs, target: a.py}]\n")
    with pytest.raises(harness.CaseError, match="needs a kind"):
        harness.load_case(bad)


def _case(expected, units=None):
    return harness.Case(
        name="synthetic",
        path=Path("synthetic/case.yaml"),
        description="",
        negative=not expected,
        files={"a.py": ""},
        expected=tuple(harness.Expected(kind=k, targets=tuple(t)) for k, t in expected),
        expected_units=units or {},
    )


def test_scoring_matches_grouped_targets_and_counts_errors():
    case = _case([("import_cycle", ["pkg/a.py", "pkg/b.py"]), ("broken_doc_link", ["docs/x.md"])])
    scored = harness.score_case(
        case,
        [
            {"kind": "import_cycle", "file": "pkg/a.py", "targets": ["pkg/a.py", "pkg/b.py"]},
            {"kind": "untested_core", "qualified_name": "pkg/util.py::helper"},
        ],
    )
    assert scored["per_kind"] == {
        "import_cycle": {"tp": 1, "fp": 0, "fn": 0},
        "untested_core": {"tp": 0, "fp": 1, "fn": 0},
        "broken_doc_link": {"tp": 0, "fp": 0, "fn": 1},
    }
    assert scored["missed"] == [{"kind": "broken_doc_link", "target": "docs/x.md"}]


def test_unit_mismatches_name_missing_and_wrong_kinds():
    case = _case([("import_cycle", ["a.py"])], units={"core": "cargo_crate", "web": "npm_package"})
    payload = {"units": [{"name": "core", "kind": "directory"}]}
    assert harness.unit_mismatches(case, payload) == [
        "core: expected cargo_crate, got directory",
        "web: expected npm_package, got nothing",
    ]


@pytest.mark.skipif(
    not os.environ.get("DAGAYN_ARCHITECTURE_EVAL"),
    reason="full architecture eval builds a graph per case; set DAGAYN_ARCHITECTURE_EVAL=1",
)
def test_full_architecture_eval_runs(tmp_path: Path):
    cases = harness.load_cases()
    result = harness.run_eval(cases, dagayn_cmd=["uv", "run", "dagayn"], work_dir=tmp_path)
    assert result["summary"]["case_count"] == len(cases)
    assert result["gate_failures"] == [], harness.format_report(result)
