"""Fixture checks for eval/run_flow_eval.py (the flow entry-point eval).

The fast tests load every case.yaml under tests/fixtures/flow_eval and
materialise it into a git repository without building a graph, so a broken
fixture fails CI. The full harness run (graph build + entry_points per case)
is opt-in: set DAGAYN_FLOW_EVAL=1.
"""

from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
from pathlib import Path

import pytest

_SCRIPT = Path(__file__).resolve().parents[1] / "eval" / "run_flow_eval.py"


def _load_harness():
    spec = importlib.util.spec_from_file_location("run_flow_eval", _SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


harness = _load_harness()
CASE_PATHS = harness.case_paths()


def test_fixture_set_covers_every_gated_kind():
    cases = harness.load_cases()
    expected_kinds = {exp.kind for case in cases for exp in case.expected}
    gated = {kind for kind, conf in harness.load_thresholds().items() if conf.get("gated")}
    assert gated <= expected_kinds
    assert any(case.negative for case in cases)
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
        "negative: true\ntarget: a.py::f\nfiles: {a.py: 'x = 1\\n'}\n"
        "expected_entry_points: [{entry_point: a.py::main, kind: main}]\n"
    )
    with pytest.raises(harness.CaseError, match="negative case cannot expect"):
        harness.load_case(bad)
    bad.write_text(
        "target: a.py::f\nfiles: {a.py: 'x = 1\\n'}\n"
        "expected_entry_points: [{entry_point: a.py::main, kind: cli}]\n"
    )
    with pytest.raises(harness.CaseError, match="needs a kind"):
        harness.load_case(bad)
    bad.write_text(
        "target: a.py::f\nfiles: {a.py: 'x = 1\\n'}\n"
        "expected_entry_points: [{entry_point: a.py::main, kind: main, chain: [a.py::g]}]\n"
    )
    with pytest.raises(harness.CaseError, match="must run from it to the target"):
        harness.load_case(bad)


def _case(expected):
    return harness.Case(
        name="synthetic",
        path=Path("synthetic/case.yaml"),
        description="",
        negative=not expected,
        target="a.py::f",
        files={"a.py": ""},
        expected=tuple(harness.Expected(entry_point=e, kind=k) for e, k in expected),
    )


def test_scoring_counts_kinds_and_checks_chains():
    case = _case([("a.py::main", "main"), ("a.py::api", "uncalled")])
    edges = {("a.py::main", "a.py::f")}
    scored = harness.score_case(
        case,
        [
            {
                "entry_point": "a.py::main",
                "kind": "named_entry",
                "hops": 1,
                "chain": ["a.py::main", "a.py::f"],
            },
            {
                "entry_point": "a.py::other",
                "kind": "uncalled",
                "hops": 1,
                "chain": ["a.py::other", "a.py::f"],
            },
        ],
        edges,
    )
    assert scored["per_kind"] == {
        "main": {"tp": 1, "fp": 0, "fn": 0},
        "uncalled": {"tp": 0, "fp": 1, "fn": 1},
    }
    assert scored["mismatches"] == ["a.py::main: kind named_entry, expected main"]
    assert scored["chain_errors"] == ["a.py::other: no edge a.py::other -> a.py::f"]


@pytest.mark.skipif(
    not os.environ.get("DAGAYN_FLOW_EVAL"),
    reason="full flow eval builds a graph per case; set DAGAYN_FLOW_EVAL=1",
)
def test_full_flow_eval_runs(tmp_path: Path):
    cases = harness.load_cases()
    result = harness.run_eval(cases, dagayn_cmd=["uv", "run", "dagayn"], work_dir=tmp_path)
    assert result["summary"]["case_count"] == len(cases)
    assert result["gate_failures"] == [], harness.format_report(result)
