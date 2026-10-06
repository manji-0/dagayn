#!/usr/bin/env python3
"""Entry-point eval for ``flow_tool(mode="entry_points")``.

Each case under ``tests/fixtures/flow_eval/<case>/case.yaml`` describes a
repository tree, a target symbol, and the entry points that reach it
(docs/plans/FLOW-TOOL-TARGET.md#evaluation). The harness writes every case
into a scratch git repository, builds the graph with the development
``dagayn`` (``--skip-flows``: the mode needs no stored flows), asks
for the target's entry points, and scores them per entry kind (precision,
recall, TP/FP/FN). It also checks every returned chain: it must start at
the entry point, end at the target, and follow a ``CALLS`` or
``CROSS_ARTIFACT`` edge of the built graph at every hop.

Case format (all paths repository-relative)::

    description: what the tree contains and why the entry points follow
    target: app/core.py::save
    negative: true             # expects no entry points
    files: {path: content}
    expected_entry_points:
      - entry_point: app/cli.py::main
        kind: main
        chain: [app/cli.py::main, app/core.py::save]   # optional, exact
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import shutil
import sqlite3
import statistics
import subprocess
import sys
import tempfile
import time
from collections.abc import Sequence
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import yaml

REPO_ROOT = Path(__file__).resolve().parents[1]
CASES_DIR = REPO_ROOT / "tests" / "fixtures" / "flow_eval"
THRESHOLDS_PATH = Path(__file__).resolve().parent / "flow_thresholds.yaml"

ENTRY_KINDS = (
    "main",
    "framework_handler",
    "ffi_export",
    "named_entry",
    "dispatched_method",
    "uncalled",
    "module_level",
)
CASE_KEYS = {"description", "negative", "target", "files", "expected_entry_points"}
CHAIN_EDGE_KINDS = ("CALLS", "CROSS_ARTIFACT")

_GIT_ENV = {
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_AUTHOR_NAME": "flow-eval",
    "GIT_AUTHOR_EMAIL": "flow-eval@example.invalid",
    "GIT_COMMITTER_NAME": "flow-eval",
    "GIT_COMMITTER_EMAIL": "flow-eval@example.invalid",
    "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z",
    "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z",
}


class CaseError(ValueError):
    """A case.yaml is malformed."""


@dataclass(frozen=True)
class Expected:
    entry_point: str
    kind: str
    chain: tuple[str, ...] | None = None


@dataclass(frozen=True)
class Case:
    name: str
    path: Path
    description: str
    negative: bool
    target: str
    files: dict[str, str]
    expected: tuple[Expected, ...] = field(default_factory=tuple)


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
    target = data.get("target")
    if not isinstance(target, str) or "::" not in target:
        raise CaseError(f"{path}: target must be a qualified name (path::name)")

    expected: list[Expected] = []
    for item in data.get("expected_entry_points") or []:
        if not isinstance(item, dict) or item.get("kind") not in ENTRY_KINDS:
            raise CaseError(f"{path}: expected entry {item!r} needs a kind from {ENTRY_KINDS}")
        entry = item.get("entry_point")
        chain = item.get("chain")
        if not isinstance(entry, str) or not entry:
            raise CaseError(f"{path}: expected entry {item!r} needs an entry_point")
        if chain is not None and (
            not isinstance(chain, list) or not chain or chain[0] != entry or chain[-1] != target
        ):
            raise CaseError(f"{path}: chain of {entry!r} must run from it to the target")
        expected.append(
            Expected(entry_point=entry, kind=item["kind"], chain=tuple(chain) if chain else None)
        )

    negative = bool(data.get("negative", False))
    if negative and expected:
        raise CaseError(f"{path}: a negative case cannot expect entry points")
    if not negative and not expected:
        raise CaseError(f"{path}: a positive case needs expected_entry_points")
    return Case(
        name=path.parent.name,
        path=path,
        description=str(data.get("description") or "").strip(),
        negative=negative,
        target=target,
        files=files,
        expected=tuple(expected),
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


def _run(cmd: Sequence[str], timeout: float) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(cmd), cwd=REPO_ROOT, capture_output=True, text=True, timeout=timeout, check=False
    )


def graph_edges(db_path: str) -> set[tuple[str, str]]:
    """(source, target) of every chain-eligible edge in the built graph."""
    marks = ",".join("?" for _ in CHAIN_EDGE_KINDS)
    with sqlite3.connect(f"file:{db_path}?mode=ro", uri=True) as conn:
        rows = conn.execute(
            f"SELECT source_qualified, target_qualified FROM edges WHERE kind IN ({marks})",  # nosec B608 - placeholders only
            CHAIN_EDGE_KINDS,
        ).fetchall()
    return {(str(source), str(target)) for source, target in rows}


def run_case(
    case: Case, work_dir: Path, dagayn_cmd: Sequence[str], timeout: float
) -> dict[str, Any]:
    repo = work_dir / case.name
    if repo.exists():
        shutil.rmtree(repo)
    row: dict[str, Any] = {"case": case.name, "negative": case.negative}
    started = time.monotonic()
    payload: dict[str, Any] | None = None
    edges: set[tuple[str, str]] = set()
    try:
        materialize(case, repo)
        build = _run([*dagayn_cmd, "build", "--repo", str(repo), "--skip-flows"], timeout)
        if build.returncode != 0:
            raise RuntimeError(f"dagayn build failed: {build.stderr.strip()[-400:]}")
        answer = _run(
            [
                *dagayn_cmd,
                "tool",
                "flow_tool",
                "--repo",
                str(repo),
                "--arg",
                'mode="entry_points"',
                "--arg",
                f"target={json.dumps(case.target)}",
                "--arg",
                'detail_level="minimal"',
            ],
            timeout,
        )
        payload, size, parse_error = parse_tool_output(answer.stdout)
        row["output_chars"] = size
        if answer.returncode != 0 or parse_error:
            raise RuntimeError(
                parse_error or f"flow_tool exited {answer.returncode}: {answer.stderr[-400:]}"
            )
        if payload is None or payload.get("status") != "ok":
            raise RuntimeError(f"flow_tool answered {payload and payload.get('summary')!r}")
        db_path = (payload.get("_repo") or {}).get("db_path")
        if not db_path:
            raise RuntimeError("flow_tool did not report _repo.db_path; chains cannot be checked")
        edges = graph_edges(str(db_path))
    except (RuntimeError, CaseError, subprocess.TimeoutExpired, OSError, sqlite3.Error) as exc:
        row["error"] = str(exc)
    row.setdefault("output_chars", 0)
    row["seconds"] = round(time.monotonic() - started, 2)
    entries = [item for item in (payload or {}).get("entry_points") or [] if isinstance(item, dict)]
    row["entry_point_count"] = len(entries)
    row.update(score_case(case, entries, edges))
    return row


# --------------------------------------------------------------------------
# Scoring
# --------------------------------------------------------------------------


def chain_errors(target: str, entry: dict[str, Any], edges: set[tuple[str, str]]) -> list[str]:
    """Why a returned chain is not a call path from its entry to the target."""
    name = str(entry.get("entry_point"))
    chain = entry.get("chain")
    if not isinstance(chain, list) or not chain:
        return [f"{name}: no chain"]
    errors: list[str] = []
    if chain[0] != name or chain[-1] != target:
        errors.append(f"{name}: chain runs {chain[0]} -> {chain[-1]}")
    if entry.get("hops") != len(chain) - 1:
        errors.append(f"{name}: hops {entry.get('hops')} for {len(chain)} steps")
    errors += [
        f"{name}: no edge {source} -> {callee}"
        for source, callee in zip(chain, chain[1:], strict=False)
        if (source, callee) not in edges
    ]
    return errors


def score_case(
    case: Case, entries: Sequence[dict[str, Any]], edges: set[tuple[str, str]]
) -> dict[str, Any]:
    """Match returned entry points to expectations, per expected kind."""
    expected = {exp.entry_point: exp for exp in case.expected}
    per_kind: dict[str, dict[str, int]] = {}
    false_positives: list[dict[str, Any]] = []
    mismatches: list[str] = []
    bad_chains: list[str] = []
    seen: set[str] = set()

    def bump(kind: str, key: str) -> None:
        per_kind.setdefault(kind, {"tp": 0, "fp": 0, "fn": 0})[key] += 1

    for entry in entries:
        name = str(entry.get("entry_point"))
        kind = str(entry.get("kind") or "<missing kind>")
        bad_chains += chain_errors(case.target, entry, edges)
        exp = expected.get(name)
        if exp is None or name in seen:
            bump(kind, "fp")
            false_positives.append({"kind": kind, "entry_point": name})
            continue
        seen.add(name)
        bump(exp.kind, "tp")
        if kind != exp.kind:
            mismatches.append(f"{name}: kind {kind}, expected {exp.kind}")
        if exp.chain is not None and tuple(entry.get("chain") or ()) != exp.chain:
            mismatches.append(f"{name}: chain {entry.get('chain')}, expected {list(exp.chain)}")
    missed = [exp for exp in case.expected if exp.entry_point not in seen]
    for exp in missed:
        bump(exp.kind, "fn")
    return {
        "per_kind": per_kind,
        "false_positives": false_positives,
        "missed": [{"kind": exp.kind, "entry_point": exp.entry_point} for exp in missed],
        "mismatches": mismatches,
        "chain_errors": bad_chains,
        "quiet": not entries,
    }


def _ratio(num: int, den: int) -> float | None:
    return round(num / den, 4) if den else None


def summarize(rows: list[dict[str, Any]]) -> dict[str, Any]:
    totals: dict[str, dict[str, int]] = {kind: {"tp": 0, "fp": 0, "fn": 0} for kind in ENTRY_KINDS}
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
        "mismatch_count": sum(len(row["mismatches"]) for row in rows),
        "chain_error_count": sum(len(row["chain_errors"]) for row in rows),
        "kinds": kinds,
        "output_chars_p50": int(statistics.median(sizes)) if sizes else None,
        "output_chars_max": max(sizes) if sizes else None,
        "negative_case_count": len(negatives),
        "negative_zero_entries_share": _ratio(
            sum(1 for row in negatives if row["quiet"] and not row.get("error")),
            len(negatives),
        ),
    }


def load_thresholds(path: Path = THRESHOLDS_PATH) -> dict[str, dict[str, Any]]:
    data = _load_yaml(path) if path.is_file() else {}
    defaults = data.get("defaults") or {}
    kinds = data.get("kinds") or {}
    return {kind: {**defaults, **(kinds.get(kind) or {})} for kind in ENTRY_KINDS}


def gate_failures(summary: dict[str, Any], thresholds: dict[str, dict[str, Any]]) -> list[str]:
    """Gated kinds below a floor, plus errors, kind or chain mismatches, bad chains."""
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
    if summary["mismatch_count"]:
        failures.append(f"{summary['mismatch_count']} kind or chain mismatch(es)")
    if summary["chain_error_count"]:
        failures.append(f"{summary['chain_error_count']} chain hop(s) without a graph edge")
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
        f"mismatches: {summary['mismatch_count']}  chain errors: {summary['chain_error_count']}",
        f"output chars p50: {summary['output_chars_p50']}  max: {summary['output_chars_max']}",
        f"negative cases with no entry points: {_fmt(summary['negative_zero_entries_share'])} "
        f"({summary['negative_case_count']} negative cases)",
        "",
        "| case | type | entry points | TP | FP | FN | output chars | seconds |",
        "|---|---|---|---|---|---|---|---|",
    ]
    for row in result["rows"]:
        counts = {key: sum(c[key] for c in row["per_kind"].values()) for key in ("tp", "fp", "fn")}
        lines.append(
            f"| {row['case']} | {'negative' if row['negative'] else 'positive'} | "
            f"{row['entry_point_count']} | {counts['tp']} | {counts['fp']} | {counts['fn']} | "
            f"{row['output_chars']} | {row['seconds']} |"
        )
    for row in result["rows"]:
        if row.get("error"):
            lines.append(f"error in {row['case']}: {row['error']}")
        for fp in row["false_positives"]:
            lines.append(f"false positive in {row['case']}: {fp['kind']} {fp['entry_point']}")
        for missed in row["missed"]:
            lines.append(f"missed in {row['case']}: {missed['kind']} {missed['entry_point']}")
        for mismatch in row["mismatches"]:
            lines.append(f"mismatch in {row['case']}: {mismatch}")
        for error in row["chain_errors"]:
            lines.append(f"bad chain in {row['case']}: {error}")
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
        with tempfile.TemporaryDirectory(prefix="flow-eval-") as tmp:
            result = run(Path(tmp))

    if args.json:
        print(json.dumps(result, indent=2, sort_keys=True))
    else:
        print(format_report(result))
    return 1 if args.gate and result["gate_failures"] else 0


if __name__ == "__main__":
    sys.exit(main())
