#!/usr/bin/env python3
"""Findings eval for ``review_tool(mode="changes")``.

Each case under ``tests/fixtures/review_eval/<case>/case.yaml`` describes a
base tree, a change on top of it, and the findings a reviewer should get.
The harness materialises every case into a scratch git repository, builds
the graph with the development ``dagayn``, runs ``review_tool`` and scores
the top-level ``findings`` list per kind (precision, recall, TP/FP/FN).

Case format (all paths repository-relative)::

    description: what the change does and why the findings follow
    negative: true            # expects no findings outside allowed_kinds
    commit_change: true       # true: commit the change, review base=HEAD~1
                              # false: leave it uncommitted, review base=HEAD
    allowed_kinds: [tests_to_run]   # tolerated kinds, scored neither TP nor FP
    base:
      from: py_pkg            # optional, _bases/<name>.yaml
      files: {path: content}  # added or overriding; null removes a base file
    change:
      files: {path: content}  # written on top; null deletes the file
      patch: |                # optional unified diff applied with git apply
    expected_findings:
      - kind: dangling_reference
        target: pkg/util.py::legacy   # qualified name or file
        also: [pkg/app.py::run]       # alternative targets, any one matches

A finding matches an expected entry of the same kind when any of its target
fields (``qualified_name``, ``file``, ``target``, ``source``) equals the
expected target or one of its ``also`` alternatives; a grouped finding also
offers each entry of its ``targets`` list. A finding matches every expected
entry of its kind it meets (a grouped one can meet several), each expected
entry is met at most once; unmatched findings are false positives, unmatched
expectations are false negatives.

Until review_tool emits ``findings`` the harness still runs: every case is
scored with an empty list and ``findings_field_present`` is false.
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
CASES_DIR = REPO_ROOT / "tests" / "fixtures" / "review_eval"
BASES_DIR = CASES_DIR / "_bases"
THRESHOLDS_PATH = Path(__file__).resolve().parent / "review_thresholds.yaml"

FINDING_KINDS = (
    "dangling_reference",
    "unchanged_caller",
    "tests_to_run",
    "untested_change",
    "contract_doc_not_updated",
    "bridge_touched",
)
TARGET_KEYS = ("qualified_name", "file", "target", "source")
CASE_KEYS = {
    "description",
    "negative",
    "commit_change",
    "allowed_kinds",
    "base",
    "change",
    "expected_findings",
}

_GIT_ENV = {
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_AUTHOR_NAME": "review-eval",
    "GIT_AUTHOR_EMAIL": "review-eval@example.invalid",
    "GIT_COMMITTER_NAME": "review-eval",
    "GIT_COMMITTER_EMAIL": "review-eval@example.invalid",
    "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z",
    "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z",
}


class CaseError(ValueError):
    """A case.yaml (or the base it names) is malformed."""


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
    commit_change: bool
    allowed_kinds: frozenset[str]
    base_files: dict[str, str]
    change_files: dict[str, str | None]
    change_patch: str | None
    expected: tuple[Expected, ...] = field(default_factory=tuple)

    @property
    def review_base(self) -> str:
        return "HEAD~1" if self.commit_change else "HEAD"


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


def _file_map(owner: Path, raw: object, *, allow_null: bool) -> dict[str, str | None]:
    if raw is None:
        return {}
    if not isinstance(raw, dict):
        raise CaseError(f"{owner}: files must be a mapping of path to content")
    out: dict[str, str | None] = {}
    for rel, content in raw.items():
        rel = _check_rel_path(owner, rel)
        if content is None and allow_null:
            out[rel] = None
        elif isinstance(content, str):
            out[rel] = content
        else:
            raise CaseError(f"{owner}: content of {rel!r} must be a string")
    return out


def load_base(name: str) -> dict[str, str]:
    path = BASES_DIR / f"{name}.yaml"
    if not path.is_file():
        raise CaseError(f"unknown base {name!r} (expected {path})")
    files = _file_map(path, _load_yaml(path).get("files"), allow_null=False)
    return {rel: content for rel, content in files.items() if content is not None}


def load_case(path: Path) -> Case:
    data = _load_yaml(path)
    unknown = set(data) - CASE_KEYS
    if unknown:
        raise CaseError(f"{path}: unknown keys {sorted(unknown)}")

    base = data.get("base") or {}
    if not isinstance(base, dict):
        raise CaseError(f"{path}: base must be a mapping")
    base_files: dict[str, str] = load_base(str(base["from"])) if base.get("from") else {}
    for rel, content in _file_map(path, base.get("files"), allow_null=True).items():
        if content is None:
            base_files.pop(rel, None)
        else:
            base_files[rel] = content
    if not base_files:
        raise CaseError(f"{path}: base tree is empty")

    change = data.get("change") or {}
    if not isinstance(change, dict):
        raise CaseError(f"{path}: change must be a mapping")
    change_files = _file_map(path, change.get("files"), allow_null=True)
    patch = change.get("patch")
    if patch is not None and not isinstance(patch, str):
        raise CaseError(f"{path}: change.patch must be a string")
    if not change_files and not patch:
        raise CaseError(f"{path}: change is empty")
    for rel, content in change_files.items():
        if content is None and rel not in base_files:
            raise CaseError(f"{path}: change deletes {rel!r}, which the base does not have")

    allowed = data.get("allowed_kinds") or []
    if not isinstance(allowed, list) or any(kind not in FINDING_KINDS for kind in allowed):
        raise CaseError(f"{path}: allowed_kinds must list kinds from {FINDING_KINDS}")

    expected: list[Expected] = []
    for item in data.get("expected_findings") or []:
        if not isinstance(item, dict) or item.get("kind") not in FINDING_KINDS:
            raise CaseError(f"{path}: expected finding {item!r} needs a kind from {FINDING_KINDS}")
        target = item.get("target")
        also = item.get("also") or []
        if not isinstance(target, str) or not target or not isinstance(also, list):
            raise CaseError(f"{path}: expected finding {item!r} needs a target string")
        if item["kind"] in allowed:
            raise CaseError(f"{path}: {item['kind']} is both expected and allowed")
        expected.append(Expected(kind=item["kind"], targets=(target, *map(str, also))))

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
        commit_change=bool(data.get("commit_change", True)),
        allowed_kinds=frozenset(allowed),
        base_files=base_files,
        change_files=change_files,
        change_patch=patch,
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
# Materialising
# --------------------------------------------------------------------------


def _git(repo: Path, *args: str, stdin: str | None = None) -> str:
    env = {**os.environ, **_GIT_ENV}
    result = subprocess.run(
        ["git", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", *args],
        cwd=repo,
        env=env,
        input=stdin,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def _write_files(repo: Path, files: dict[str, str | None]) -> None:
    for rel, content in files.items():
        target = repo / rel
        if content is None:
            target.unlink()
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")


def materialize(case: Case, dest: Path) -> str:
    """Build the case's git repository in ``dest``; return the review base."""
    dest.mkdir(parents=True, exist_ok=True)
    _git(dest, "init", "-q", "-b", "main")
    _write_files(dest, dict(case.base_files))
    _git(dest, "add", "-A")
    _git(dest, "commit", "-q", "-m", "base")
    _write_files(dest, case.change_files)
    if case.change_patch:
        _git(dest, "apply", "--whitespace=nowarn", "-", stdin=case.change_patch)
    if not _git(dest, "status", "--porcelain").strip():
        raise CaseError(f"{case.path}: change leaves the tree identical to the base")
    if case.commit_change:
        _git(dest, "add", "-A")
        _git(dest, "commit", "-q", "-m", "change")
    return case.review_base


# --------------------------------------------------------------------------
# Running review_tool
# --------------------------------------------------------------------------


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


def legacy_signals(payload: dict[str, Any] | None) -> dict[str, Any]:
    """Score-first fields of the current contract, kept for before/after comparison."""
    if not payload:
        return {}

    def names(key: str) -> list[str]:
        items = payload.get(key)
        if not isinstance(items, list):
            return []
        return sorted(
            str(item.get("qualified_name") or item.get("file"))
            for item in items
            if isinstance(item, dict)
        )

    reason_codes = payload.get("reason_codes")
    return {
        "risk_level": payload.get("risk_level"),
        "reason_code_count": len(reason_codes) if isinstance(reason_codes, list) else None,
        "changed_node_count": payload.get("changed_node_count"),
        "impacted_node_count": payload.get("impacted_node_count"),
        "test_gap_count": payload.get("test_gap_count"),
        "recommended_tests": names("recommended_tests"),
        "documentation_update_candidates": names("documentation_update_candidates"),
    }


def _run(cmd: Sequence[str], timeout: float) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(cmd),
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        timeout=timeout,
        check=False,
    )


def run_case(
    case: Case,
    work_dir: Path,
    dagayn_cmd: Sequence[str],
    timeout: float,
) -> dict[str, Any]:
    repo = work_dir / case.name
    if repo.exists():
        shutil.rmtree(repo)
    row: dict[str, Any] = {"case": case.name, "negative": case.negative}
    started = time.monotonic()
    payload: dict[str, Any] | None = None
    try:
        base = materialize(case, repo)
        row["base"] = base
        build = _run([*dagayn_cmd, "build", "--repo", str(repo)], timeout)
        if build.returncode != 0:
            raise RuntimeError(f"dagayn build failed: {build.stderr.strip()[-400:]}")
        row["build_seconds"] = round(time.monotonic() - started, 2)
        review = _run(
            [
                *dagayn_cmd,
                "tool",
                "review_tool",
                "--repo",
                str(repo),
                "--arg",
                'mode="changes"',
                "--arg",
                'detail_level="minimal"',
                "--arg",
                f"base={json.dumps(base)}",
            ],
            timeout,
        )
        payload, size, parse_error = parse_tool_output(review.stdout)
        row["output_chars"] = size
        if review.returncode != 0 or parse_error:
            raise RuntimeError(
                parse_error or f"review_tool exited {review.returncode}: {review.stderr[-400:]}"
            )
        if payload and payload.get("status") not in (None, "ok"):
            row["tool_status"] = payload.get("status")
    except (RuntimeError, CaseError, subprocess.TimeoutExpired, OSError) as exc:
        row["error"] = str(exc)
    row.setdefault("output_chars", 0)
    row["seconds"] = round(time.monotonic() - started, 2)
    findings, present = extract_findings(payload)
    row["findings_field_present"] = present
    row["finding_count"] = len(findings)
    row["legacy"] = legacy_signals(payload)
    row.update(score_case(case, findings))
    return row


# --------------------------------------------------------------------------
# Scoring
# --------------------------------------------------------------------------


def finding_targets(finding: dict[str, Any]) -> set[str]:
    targets = {str(finding[key]) for key in TARGET_KEYS if finding.get(key)}
    # A grouped finding (one file, several symbols) lists them in ``targets``.
    listed = finding.get("targets")
    if isinstance(listed, list):
        targets.update(str(item) for item in listed if item)
    return targets


def score_case(case: Case, findings: Iterable[dict[str, Any]]) -> dict[str, Any]:
    """Match findings to expectations; return per-kind counts and details."""
    unmatched = list(case.expected)
    per_kind: dict[str, dict[str, int]] = {}
    false_positives: list[dict[str, Any]] = []
    ignored = 0

    def bump(kind: str, key: str) -> None:
        per_kind.setdefault(kind, {"tp": 0, "fp": 0, "fn": 0})[key] += 1

    for finding in findings:
        kind = str(finding.get("kind") or "<missing kind>")
        if kind in case.allowed_kinds:
            ignored += 1
            continue
        targets = finding_targets(finding)
        # A grouped finding may meet several expectations of its kind.
        matches = [exp for exp in unmatched if exp.kind == kind and targets & set(exp.targets)]
        if not matches:
            bump(kind, "fp")
            false_positives.append(
                {"kind": kind, "targets": sorted(targets)},
            )
        for match in matches:
            unmatched.remove(match)
            bump(kind, "tp")
    for exp in unmatched:
        bump(exp.kind, "fn")
    return {
        "per_kind": per_kind,
        "false_positives": false_positives,
        "missed": [{"kind": exp.kind, "target": exp.target} for exp in unmatched],
        "allowed_findings": ignored,
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
    """Kinds marked ``gated: true`` whose precision (or recall) is below its floor.

    A precision of ``None`` (the kind emitted nothing) is not "below" a floor;
    set ``recall_floor`` to also fail a gated kind that never fires.
    """
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
        f"findings field present: {summary['findings_field_present']}",
        f"output chars p50: {summary['output_chars_p50']}  max: {summary['output_chars_max']}",
        f"negative cases with zero findings: {_fmt(summary['negative_zero_findings_share'])} "
        f"({summary['negative_case_count']} negative cases)",
        "",
        "| case | type | base | findings | TP | FP | FN | output chars | legacy risk | seconds |",
        "|---|---|---|---|---|---|---|---|---|---|",
    ]
    for row in result["rows"]:
        counts = {key: sum(c[key] for c in row["per_kind"].values()) for key in ("tp", "fp", "fn")}
        lines.append(
            f"| {row['case']} | {'negative' if row['negative'] else 'positive'} | "
            f"{row.get('base', '-')} | {row['finding_count']} | {counts['tp']} | "
            f"{counts['fp']} | {counts['fn']} | {row['output_chars']} | "
            f"{row.get('legacy', {}).get('risk_level') or '-'} | {row['seconds']} |"
        )
    errors = [row for row in result["rows"] if row.get("error")]
    if errors:
        lines.append("")
        lines += [f"error in {row['case']}: {row['error']}" for row in errors]
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
    parser.add_argument("--gate", action="store_true", help="exit 1 when a gated kind fails")
    parser.add_argument("--thresholds", default=str(THRESHOLDS_PATH))
    parser.add_argument("--dagayn-cmd", default="uv run dagayn", help="command to run dagayn")
    parser.add_argument("--jobs", type=int, default=4)
    parser.add_argument("--timeout", type=float, default=300.0, help="seconds per command")
    parser.add_argument("--work-dir", help="keep the materialised repositories here")
    args = parser.parse_args(argv)

    cases = load_cases(args.case or None)
    dagayn_cmd = shlex.split(args.dagayn_cmd)
    if args.work_dir:
        work_dir = Path(args.work_dir).resolve()
        work_dir.mkdir(parents=True, exist_ok=True)
        result = run_eval(
            cases,
            dagayn_cmd=dagayn_cmd,
            work_dir=work_dir,
            jobs=args.jobs,
            timeout=args.timeout,
            thresholds_path=Path(args.thresholds),
        )
    else:
        with tempfile.TemporaryDirectory(prefix="review-eval-") as tmp:
            result = run_eval(
                cases,
                dagayn_cmd=dagayn_cmd,
                work_dir=Path(tmp),
                jobs=args.jobs,
                timeout=args.timeout,
                thresholds_path=Path(args.thresholds),
            )

    if args.json:
        print(json.dumps(result, indent=2, sort_keys=True))
    else:
        print(format_report(result))
    return 1 if args.gate and result["gate_failures"] else 0


if __name__ == "__main__":
    sys.exit(main())
