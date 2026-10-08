"""Checks for eval/run_test_reach_eval.py (untested_change against coverage).

The fast tests check the harness's own rules. The full run reads a pytest-cov
JSON report of this repository's test run and the repository's graph: set
DAGAYN_TEST_REACH_EVAL=1 and DAGAYN_COVERAGE_JSON to the report.
"""

from __future__ import annotations

import importlib.util
import os
import sqlite3
import subprocess
import sys
from pathlib import Path

import pytest

_SCRIPT = Path(__file__).resolve().parents[1] / "eval" / "run_test_reach_eval.py"


def _load_harness():
    spec = importlib.util.spec_from_file_location("run_test_reach_eval", _SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


harness = _load_harness()


def test_test_paths_follow_the_finding():
    assert harness.is_test_path("tests/test_app.py")
    assert harness.is_test_path("pkg/conftest.py")
    assert harness.is_test_path("tests/fixtures/repo/app.py")
    assert not harness.is_test_path("dagayn/testing_helpers.py")


def test_population_counts_a_body_line_not_the_def_line(tmp_path: Path):
    db = tmp_path / "graph.db"
    with sqlite3.connect(db) as conn:
        conn.executescript(
            "CREATE TABLE nodes (qualified_name TEXT, kind TEXT, is_test INT, language TEXT,"
            " file_path TEXT, line_start INT, line_end INT);"
            "CREATE TABLE edges (kind TEXT, target_qualified TEXT, extra TEXT);"
        )
        conn.executemany(
            "INSERT INTO nodes VALUES (?, 'Function', 0, 'python', 'pkg/a.py', ?, ?)",
            [("pkg/a.py::ran", 1, 2), ("pkg/a.py::loaded", 4, 5)],
        )
        conn.execute("INSERT INTO edges VALUES ('CALLS', 'pkg/a.py::ran', '{}')")
    coverage = {"files": {"pkg/a.py": {"executed_lines": [1, 2, 4], "missing_lines": [5]}}}
    rows = harness.population(db, coverage)
    assert sorted(rows) == [("pkg/a.py::loaded", False, 0), ("pkg/a.py::ran", True, 1)]


def test_gate_reports_each_metric_below_its_floor():
    result = {"untested_precision": 0.3, "tested_precision": 0.99, "untested_recall": None}
    floors = {
        "floors": {"untested_precision": 0.4, "tested_precision": 0.95, "untested_recall": 0.8}
    }
    failures = harness.gate(result, floors)
    assert len(failures) == 2
    assert any("untested_precision" in failure for failure in failures)


@pytest.mark.skipif(
    os.environ.get("DAGAYN_TEST_REACH_EVAL") != "1",
    reason="set DAGAYN_TEST_REACH_EVAL=1 and DAGAYN_COVERAGE_JSON to run the eval",
)
def test_untested_change_against_coverage_meets_its_floors():
    # A process of its own: the harness's worker pool imports it by path.
    run = subprocess.run(
        [
            sys.executable,
            str(_SCRIPT),
            "--coverage",
            os.environ["DAGAYN_COVERAGE_JSON"],
            "--gate",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    print(run.stdout)
    assert run.returncode == 0, run.stdout + run.stderr
