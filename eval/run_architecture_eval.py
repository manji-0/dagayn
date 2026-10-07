#!/usr/bin/env python3
"""Findings eval for ``architecture_analysis_tool(mode="overview")``.

Each case under ``tests/fixtures/architecture_eval/<case>/case.yaml``
describes a repository tree and the findings the overview should report
(docs/plans/ARCHITECTURE-TOOL-TARGET.md#evaluation). The harness writes
every case into a scratch git repository, builds the graph with the
development ``dagayn``, runs the overview, and scores the top-level
``findings`` list per kind (precision, recall, TP/FP/FN).

Case format (all paths repository-relative)::

    description: what the tree contains and why the findings follow
    negative: true            # expects no findings
    files: {path: content}
    expected_findings:
      - kind: import_cycle
        target: pkg/a.py      # qualified name or file
        also: [pkg/b.py]      # alternative targets, any one matches
    expected_units:           # optional: unit name -> kind
      core-lib: cargo_crate

Matching follows ``eval/run_review_eval.py``: a finding matches an expected
entry of its kind when one of its target fields (``qualified_name``,
``file``, ``target``, ``source``, or an entry of ``targets``) is the expected
target or one of its alternatives.
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from collections.abc import Iterable, Sequence
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import yaml

REPO_ROOT = Path(__file__).resolve().parents[1]
CASES_DIR = REPO_ROOT / "tests" / "fixtures" / "architecture_eval"
THRESHOLDS_PATH = Path(__file__).resolve().parent / "architecture_thresholds.yaml"

FINDING_KINDS = ("import_cycle", "unstable_dependency", "untested_core", "broken_doc_link")
TARGET_KEYS = ("qualified_name", "file", "target", "source")
CASE_KEYS = {"description", "negative", "files", "expected_findings", "expected_units"}

_GIT_ENV = {
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_AUTHOR_NAME": "architecture-eval",
    "GIT_AUTHOR_EMAIL": "architecture-eval@example.invalid",
    "GIT_COMMITTER_NAME": "architecture-eval",
    "GIT_COMMITTER_EMAIL": "architecture-eval@example.invalid",
    "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z",
    "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z",
}


class CaseError(ValueError):
    """A case.yaml is malformed."""


@dataclass(frozen=True)
class Expected:
    kind: str
    targets: tuple[str, ...]

    @property
    def target(self) -> str:
        return self.targets[0]


@dataclass(frozen=True)
class Case:
    name: str
    path: Path
    description: str
    negative: bool
    files: dict[str, str]
    expected: tuple[Expected, ...] = field(default_factory=tuple)
    expected_units: dict[str, str] = field(default_factory=dict)


# --------------------------------------------------------------------------
# Loading
# --------------------------------------------------------------------------


def _load_yaml(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as fh:
        data = yaml.safe_load(fh) or {}
    if not isinstance(data, dict):
        raise CaseError(f"{path}: top level must be a mapping")
    return data


def _check_rel_path(owner: Path, rel: object) -> str:
    if not isinstance(rel, str) or not rel or rel.startswith("/") or ".." in Path(rel).parts:
        raise CaseError(f"{owner}: invalid repository path {rel!r}")
    return rel


def load_case(path: Path) -> Case:
    data = _load_yaml(path)
    unknown = set(data) - CASE_KEYS
    if unknown:
        raise CaseError(f"{path}: unknown keys {sorted(unknown)}")
    raw_files = data.get("files")
    if not isinstance(raw_files, dict) or not raw_files:
        raise CaseError(f"{path}: files must be a non-empty mapping of path to content")
    files: dict[str, str] = {}
    for rel, content in raw_files.items():
        if not isinstance(content, str):
            raise CaseError(f"{path}: content of {rel!r} must be a string")
        files[_check_rel_path(path, rel)] = content

    expected: list[Expected] = []
    for item in data.get("expected_findings") or []:
        if not isinstance(item, dict) or item.get("kind") not in FINDING_KINDS:
            raise CaseError(f"{path}: expected finding {item!r} needs a kind from {FINDING_KINDS}")
        target = item.get("target")
        also = item.get("also") or []
        if not isinstance(target, str) or not target or not isinstance(also, list):
            raise CaseError(f"{path}: expected finding {item!r} needs a target string")
        expected.append(Expected(kind=item["kind"], targets=(target, *map(str, also))))

    units = data.get("expected_units") or {}
    if not isinstance(units, dict) or not all(
        isinstance(key, str) and isinstance(value, str) for key, value in units.items()
    ):
        raise CaseError(f"{path}: expected_units must map unit names to kinds")

    negative = bool(data.get("negative", False))
    if negative and expected:
        raise CaseError(f"{path}: a negative case cannot expect findings")
    if not negative and not expected:
        raise CaseError(f"{path}: a positive case needs expected_findings")
    return Case(
        name=path.parent.name,
        path=path,
        description=str(data.get("description") or "").strip(),
        negative=negative,
        files=files,
        expected=tuple(expected),
        expected_units=dict(units),
    )


def case_paths(root: Path = CASES_DIR) -> list[Path]:
    return sorted(
        path / "case.yaml"
        for path in root.iterdir()
        if path.is_dir() and not path.name.startswith("_") and (path / "case.yaml").is_file()
    )


def load_cases(names: Sequence[str] | None = None, root: Path = CASES_DIR) -> list[Case]:
    cases = [load_case(path) for path in case_paths(root)]
    if names:
        wanted = set(names)
        missing = wanted - {case.name for case in cases}
        if missing:
            raise CaseError(f"unknown case(s): {sorted(missing)}")
        cases = [case for case in cases if case.name in wanted]
    return cases


# --------------------------------------------------------------------------
# Materialising and running
# --------------------------------------------------------------------------


def _git(repo: Path, *args: str) -> None:
    result = subprocess.run(
        ["git", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", *args],
        cwd=repo,
        env={**os.environ, **_GIT_ENV},
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} failed: {result.stderr.strip()}")


def materialize(case: Case, dest: Path) -> None:
    dest.mkdir(parents=True, exist_ok=True)
    _git(dest, "init", "-q", "-b", "main")
    for rel, content in case.files.items():
        target = dest / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")
    _git(dest, "add", "-A")
    _git(dest, "commit", "-q", "-m", "tree")


def parse_tool_output(stdout: str) -> tuple[dict[str, Any] | None, int, str | None]:
    """Parse the first JSON object in ``stdout``: (payload, size in chars, error)."""
    start = stdout.find("{")
    if start < 0:
        return None, 0, "no JSON object in output"
    try:
        payload, end = json.JSONDecoder().raw_decode(stdout, start)
    except json.JSONDecodeError as exc:
        return None, len(stdout) - start, f"invalid JSON: {exc}"
    if not isinstance(payload, dict):
        return None, end - start, "JSON output is not an object"
    return payload, end - start, None


def extract_findings(payload: dict[str, Any] | None) -> tuple[list[dict[str, Any]], bool]:
    if not payload or not isinstance(payload.get("findings"), list):
        return [], False
    return [item for item in payload["findings"] if isinstance(item, dict)], True


def _run(cmd: Sequence[str], timeout: float) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(cmd), cwd=REPO_ROOT, capture_output=True, text=True, timeout=timeout, check=False
    )


def run_case(
    case: Case, work_dir: Path, dagayn_cmd: Sequence[str], timeout: float
) -> dict[str, Any]:
    repo = work_dir / case.name
    if repo.exists():
        shutil.rmtree(repo)
    row: dict[str, Any] = {"case": case.name, "negative": case.negative}
    started = time.monotonic()
    payload: dict[str, Any] | None = None
    try:
        materialize(case, repo)
        build = _run([*dagayn_cmd, "build", "--repo", str(repo)], timeout)
        if build.returncode != 0:
            raise RuntimeError(f"dagayn build failed: {build.stderr.strip()[-400:]}")
        overview = _run(
            [
                *dagayn_cmd,
                "tool",
                "architecture_analysis_tool",
                "--repo",
                str(repo),
                "--arg",
                'mode="overview"',
                "--arg",
                'detail_level="minimal"',
            ],
            timeout,
        )
        payload, size, parse_error = parse_tool_output(overview.stdout)
        row["output_chars"] = size
        if overview.returncode != 0 or parse_error:
            raise RuntimeError(
                parse_error
                or f"architecture_analysis_tool exited {overview.returncode}: "
                f"{overview.stderr[-400:]}"
            )
    except (RuntimeError, CaseError, subprocess.TimeoutExpired, OSError) as exc:
        row["error"] = str(exc)
    row.setdefault("output_chars", 0)
    row["seconds"] = round(time.monotonic() - started, 2)
    findings, present = extract_findings(payload)
    row["findings_field_present"] = present
    row["finding_count"] = len(findings)
    row["unit_mismatches"] = unit_mismatches(case, payload)
    row.update(score_case(case, findings))
    return row


# --------------------------------------------------------------------------
# Scoring
# --------------------------------------------------------------------------


def unit_mismatches(case: Case, payload: dict[str, Any] | None) -> list[str]:
    """Expected units missing from the map or reported with another kind."""
    units = payload.get("units") if payload else None
    found = {
        str(unit.get("name")): str(unit.get("kind"))
        for unit in units or []
        if isinstance(unit, dict)
    }
    return [
        f"{name}: expected {kind}, got {found.get(name, 'nothing')}"
        for name, kind in case.expected_units.items()
        if found.get(name) != kind
    ]


def finding_targets(finding: dict[str, Any]) -> set[str]:
    targets = {str(finding[key]) for key in TARGET_KEYS if finding.get(key)}
    listed = finding.get("targets")
    if isinstance(listed, list):
        targets.update(str(item) for item in listed if item)
    return targets


def score_case(case: Case, findings: Iterable[dict[str, Any]]) -> dict[str, Any]:
    """Match findings to expectations; return per-kind counts and details."""
    unmatched = list(case.expected)
    per_kind: dict[str, dict[str, int]] = {}
    false_positives: list[dict[str, Any]] = []

    def bump(kind: str, key: str) -> None:
        per_kind.setdefault(kind, {"tp": 0, "fp": 0, "fn": 0})[key] += 1

    for finding in findings:
        kind = str(finding.get("kind") or "<missing kind>")
        targets = finding_targets(finding)
        matches = [exp for exp in unmatched if exp.kind == kind and targets & set(exp.targets)]
        if not matches:
            bump(kind, "fp")
            false_positives.append({"kind": kind, "targets": sorted(targets)})
        for match in matches:
            unmatched.remove(match)
            bump(kind, "tp")
    for exp in unmatched:
        bump(exp.kind, "fn")
    return {
        "per_kind": per_kind,
        "false_positives": false_positives,
        "missed": [{"kind": exp.kind, "target": exp.target} for exp in unmatched],
        "quiet": not false_positives and not any(c["tp"] for c in per_kind.values()),
    }


def _ratio(num: int, den: int) -> float | None:
    return round(num / den, 4) if den else None


def summarize(rows: list[dict[str, Any]]) -> dict[str, Any]:
    totals: dict[str, dict[str, int]] = {
        kind: {"tp": 0, "fp": 0, "fn": 0} for kind in FINDING_KINDS
    }
    for row in rows:
        for kind, counts in row["per_kind"].items():
            bucket = totals.setdefault(kind, {"tp": 0, "fp": 0, "fn": 0})
            for key, value in counts.items():
                bucket[key] += value
    kinds = {
        kind: {
            **counts,
            "precision": _ratio(counts["tp"], counts["tp"] + counts["fp"]),
            "recall": _ratio(counts["tp"], counts["tp"] + counts["fn"]),
        }
        for kind, counts in totals.items()
    }
    sizes = [row["output_chars"] for row in rows if not row.get("error")]
    negatives = [row for row in rows if row["negative"]]
    return {
        "case_count": len(rows),
        "error_count": sum(1 for row in rows if row.get("error")),
        "unit_mismatch_count": sum(len(row["unit_mismatches"]) for row in rows),
        "findings_field_present": any(row["findings_field_present"] for row in rows),
        "kinds": kinds,
        "output_chars_p50": int(statistics.median(sizes)) if sizes else None,
        "output_chars_max": max(sizes) if sizes else None,
        "negative_case_count": len(negatives),
        "negative_zero_findings_share": _ratio(
            sum(1 for row in negatives if row["quiet"] and not row.get("error")),
            len(negatives),
        ),
    }


def load_thresholds(path: Path = THRESHOLDS_PATH) -> dict[str, dict[str, Any]]:
    data = _load_yaml(path) if path.is_file() else {}
    defaults = data.get("defaults") or {}
    kinds = data.get("kinds") or {}
    return {kind: {**defaults, **(kinds.get(kind) or {})} for kind in FINDING_KINDS}


def gate_failures(summary: dict[str, Any], thresholds: dict[str, dict[str, Any]]) -> list[str]:
    """Gated kinds below a floor, plus errors and unit-map mismatches."""
    failures: list[str] = []
    for kind, conf in thresholds.items():
        if not conf.get("gated"):
            continue
        stats = summary["kinds"][kind]
        for metric in ("precision", "recall"):
            floor = conf.get(f"{metric}_floor")
            value = stats[metric]
            if floor is not None and value is not None and value < float(floor):
                failures.append(f"{kind}: {metric} {value} < {floor}")
    if summary["error_count"]:
        failures.append(f"{summary['error_count']} case(s) errored")
    if summary["unit_mismatch_count"]:
        failures.append(f"{summary['unit_mismatch_count']} expected unit(s) not mapped")
    return failures


# --------------------------------------------------------------------------
# Reporting
# --------------------------------------------------------------------------


def _fmt(value: float | None) -> str:
    return "-" if value is None else f"{value:.2f}"


def format_report(result: dict[str, Any]) -> str:
    summary = result["summary"]
    thresholds = result["thresholds"]
    lines = [
        "| kind | gated | floor | TP | FP | FN | precision | recall |",
        "|---|---|---|---|---|---|---|---|",
    ]
    for kind, stats in summary["kinds"].items():
        conf = thresholds.get(kind, {})
        lines.append(
            f"| {kind} | {'yes' if conf.get('gated') else 'no'} | "
            f"{conf.get('precision_floor', '-')} | {stats['tp']} | {stats['fp']} | "
            f"{stats['fn']} | {_fmt(stats['precision'])} | {_fmt(stats['recall'])} |"
        )
    lines += [
        "",
        f"cases: {summary['case_count']}  errors: {summary['error_count']}  "
        f"unit mismatches: {summary['unit_mismatch_count']}  "
        f"findings field present: {summary['findings_field_present']}",
        f"output chars p50: {summary['output_chars_p50']}  max: {summary['output_chars_max']}",
        f"negative cases with zero findings: {_fmt(summary['negative_zero_findings_share'])} "
        f"({summary['negative_case_count']} negative cases)",
        "",
        "| case | type | findings | TP | FP | FN | output chars | seconds |",
        "|---|---|---|---|---|---|---|---|",
    ]
    for row in result["rows"]:
        counts = {key: sum(c[key] for c in row["per_kind"].values()) for key in ("tp", "fp", "fn")}
        lines.append(
            f"| {row['case']} | {'negative' if row['negative'] else 'positive'} | "
            f"{row['finding_count']} | {counts['tp']} | {counts['fp']} | {counts['fn']} | "
            f"{row['output_chars']} | {row['seconds']} |"
        )
    for row in result["rows"]:
        if row.get("error"):
            lines.append(f"error in {row['case']}: {row['error']}")
        for mismatch in row["unit_mismatches"]:
            lines.append(f"unit mismatch in {row['case']}: {mismatch}")
        for fp in row["false_positives"]:
            lines.append(f"false positive in {row['case']}: {fp['kind']} {fp['targets']}")
        for missed in row["missed"]:
            lines.append(f"missed in {row['case']}: {missed['kind']} {missed['target']}")
    if result["gate_failures"]:
        lines.append("")
        lines += [f"GATE FAILED {failure}" for failure in result["gate_failures"]]
    return "\n".join(lines)


def run_eval(
    cases: Sequence[Case],
    *,
    dagayn_cmd: Sequence[str],
    work_dir: Path,
    jobs: int = 4,
    timeout: float = 300.0,
    thresholds_path: Path = THRESHOLDS_PATH,
) -> dict[str, Any]:
    with ThreadPoolExecutor(max_workers=max(1, jobs)) as pool:
        rows = list(pool.map(lambda case: run_case(case, work_dir, dagayn_cmd, timeout), cases))
    summary = summarize(rows)
    thresholds = load_thresholds(thresholds_path)
    return {
        "summary": summary,
        "thresholds": thresholds,
        "gate_failures": gate_failures(summary, thresholds),
        "rows": rows,
    }


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    parser.add_argument("--case", action="append", default=[], help="run only this case")
    parser.add_argument("--json", action="store_true", help="print the full result as JSON")
    parser.add_argument("--gate", action="store_true", help="exit 1 when the gate fails")
    parser.add_argument("--thresholds", default=str(THRESHOLDS_PATH))
    parser.add_argument("--dagayn-cmd", default="uv run dagayn", help="command to run dagayn")
    parser.add_argument("--jobs", type=int, default=4)
    parser.add_argument("--timeout", type=float, default=300.0, help="seconds per command")
    parser.add_argument("--work-dir", help="keep the materialised repositories here")
    args = parser.parse_args(argv)

    cases = load_cases(args.case or None)
    dagayn_cmd = shlex.split(args.dagayn_cmd)
    run = lambda work_dir: run_eval(  # noqa: E731
        cases,
        dagayn_cmd=dagayn_cmd,
        work_dir=work_dir,
        jobs=args.jobs,
        timeout=args.timeout,
        thresholds_path=Path(args.thresholds),
    )
    if args.work_dir:
        work_dir = Path(args.work_dir).resolve()
        work_dir.mkdir(parents=True, exist_ok=True)
        result = run(work_dir)
    else:
        with tempfile.TemporaryDirectory(prefix="architecture-eval-") as tmp:
            result = run(Path(tmp))

    if args.json:
        print(json.dumps(result, indent=2, sort_keys=True))
    else:
        print(format_report(result))
    return 1 if args.gate and result["gate_failures"] else 0


if __name__ == "__main__":
    sys.exit(main())
