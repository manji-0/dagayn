#!/usr/bin/env python3
"""Test-reach eval: ``untested_change``'s judgement against real coverage.

``review_tool``'s ``untested_change`` calls a changed function untested when
no test reaches it through the call graph within the hop limit
(docs/plans/TEST-REACH-TARGET.md). This harness asks the same question of
every Python production function of this repository, through
``query_graph_tool(pattern="tests_for")``'s ``test_reach`` (the code path the
finding uses), and scores the answers against a pytest-cov JSON report of a
test run: a function counts as covered when any line of its body ran.

Reported:

- ``untested`` precision (the share of functions called untested that did
  not run) and recall (the share of functions that did not run that are
  called untested);
- ``tested`` precision (the share of functions called tested that ran);
- why each function called untested is: ``no_callers`` (nothing in the graph
  calls or references it), ``no_test_path`` (callers, but no test within
  the limit), or ``beyond_limit`` (a test a few hops past it).

Coverage counts only what ran in the pytest process: code a test runs in a
subprocess (the ``dagayn`` command) counts as not run, so the precision of
``untested`` is if anything overstated.

Usage::

    uv run pytest --cov=dagayn --cov-report=json:coverage.json
    uv run dagayn build
    uv run python eval/run_test_reach_eval.py --coverage coverage.json --gate
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import sys
from collections import Counter
from multiprocessing import Pool
from pathlib import Path
from typing import Any

import yaml

ROOT = Path(__file__).resolve().parent.parent
THRESHOLDS = ROOT / "eval" / "test_reach_thresholds.yaml"
TEST_DIRS = {"tests", "test", "__tests__", "spec", "testdata", "fixtures"}


def is_test_path(path: str) -> bool:
    """The test-path rule of ``findings::is_test_path``, plus fixtures."""
    parts = path.split("/")
    name = parts[-1]
    return (
        any(part in TEST_DIRS for part in parts)
        or name.startswith("test_")
        or "_test." in name
        or ".test." in name
        or ".spec." in name
        or name == "conftest.py"
    )


def population(db_path: Path, coverage: dict[str, Any]) -> list[tuple[str, bool, int]]:
    """``(qualified_name, covered, incoming)`` of each Python production
    function the coverage report measures."""
    files = coverage["files"]
    rows = []
    with sqlite3.connect(f"file:{db_path}?mode=ro", uri=True) as conn:
        incoming = Counter(
            target
            for (target,) in conn.execute(
                "SELECT target_qualified FROM edges WHERE kind = 'CALLS' "
                "OR (kind = 'REFERENCES' AND json_extract(extra, '$.relationship_role') IS NULL)"
            )
        )
        nodes = conn.execute(
            "SELECT qualified_name, file_path, line_start, line_end FROM nodes "
            "WHERE kind = 'Function' AND is_test = 0 AND language = 'python'"
        ).fetchall()
    for qualified_name, file_path, line_start, line_end in nodes:
        report = files.get(file_path)
        if report is None or is_test_path(file_path):
            continue
        executed = set(report["executed_lines"])
        measured = executed | set(report["missing_lines"])
        # The `def` line runs when the module loads; the body runs on a call.
        body = [line for line in range(line_start + 1, line_end + 1) if line in measured]
        if not body:
            continue
        covered = any(line in executed for line in body)
        rows.append((qualified_name, covered, incoming[qualified_name]))
    return rows


def _reach(qualified_name: str) -> tuple[str, dict[str, Any] | None]:
    from dagayn.tools._native import native_tool

    reply = native_tool(
        "query_graph_tool", pattern="tests_for", target=qualified_name, repo_root=str(ROOT)
    )
    return qualified_name, reply.get("test_reach")


def evaluate(coverage_path: Path, db_path: Path, jobs: int) -> dict[str, Any]:
    coverage = json.loads(coverage_path.read_text(encoding="utf-8"))
    rows = population(db_path, coverage)
    with Pool(jobs) as pool:
        reach = dict(pool.map(_reach, [qn for qn, _, _ in rows], chunksize=20))
    counts = Counter()
    causes = Counter()
    examples: dict[str, list[str]] = {}
    for qualified_name, covered, incoming in rows:
        answer = reach.get(qualified_name) or {}
        tested = bool(answer.get("counts_as_tested"))
        counts[("tested" if tested else "untested", "ran" if covered else "did_not_run")] += 1
        if not tested:
            if answer.get("hops") is not None:
                cause = "beyond_limit"
            elif incoming == 0:
                cause = "no_callers"
            else:
                cause = "no_test_path"
            causes[(cause, "ran" if covered else "did_not_run")] += 1
            if covered:
                examples.setdefault(cause, []).append(qualified_name)

    def share(numerator: int, denominator: int) -> float | None:
        return round(numerator / denominator, 3) if denominator else None

    untested = counts[("untested", "ran")] + counts[("untested", "did_not_run")]
    did_not_run = counts[("untested", "did_not_run")] + counts[("tested", "did_not_run")]
    tested = counts[("tested", "ran")] + counts[("tested", "did_not_run")]
    return {
        "functions": len(rows),
        "untested": untested,
        "untested_precision": share(counts[("untested", "did_not_run")], untested),
        "untested_recall": share(counts[("untested", "did_not_run")], did_not_run),
        "tested_precision": share(counts[("tested", "ran")], tested),
        "causes": {
            cause: {
                "ran": causes[(cause, "ran")],
                "did_not_run": causes[(cause, "did_not_run")],
            }
            for cause in ("no_callers", "no_test_path", "beyond_limit")
        },
        "examples": {cause: sorted(names)[:10] for cause, names in examples.items()},
    }


def gate(result: dict[str, Any], thresholds: dict[str, Any]) -> list[str]:
    failures = []
    for metric, floor in thresholds.get("floors", {}).items():
        value = result.get(metric)
        if value is None or value < floor:
            failures.append(f"{metric} {value} is below its floor {floor}")
    return failures


def format_report(result: dict[str, Any]) -> str:
    lines = [
        f"functions: {result['functions']}  called untested: {result['untested']}",
        f"untested precision: {result['untested_precision']}  "
        f"untested recall: {result['untested_recall']}  "
        f"tested precision: {result['tested_precision']}",
        "",
        "| cause | ran (wrongly untested) | did not run |",
        "|---|---|---|",
    ]
    for cause, row in result["causes"].items():
        lines.append(f"| {cause} | {row['ran']} | {row['did_not_run']} |")
    for cause, names in result["examples"].items():
        lines.append(f"\n{cause}, ran: " + ", ".join(names))
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--coverage", type=Path, required=True, help="pytest-cov JSON report")
    parser.add_argument("--db", type=Path, default=ROOT / ".dagayn" / "graph.db")
    parser.add_argument("--jobs", type=int, default=8)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--gate", action="store_true")
    parser.add_argument("--thresholds", type=Path, default=THRESHOLDS)
    args = parser.parse_args()
    result = evaluate(args.coverage, args.db, args.jobs)
    print(json.dumps(result, indent=2) if args.json else format_report(result))
    if args.gate:
        failures = gate(result, yaml.safe_load(args.thresholds.read_text(encoding="utf-8")))
        for failure in failures:
            print(f"GATE FAILED {failure}")
        return 1 if failures else 0
    return 0


if __name__ == "__main__":
    sys.exit(main())
