#!/usr/bin/env python3
"""Terraform bridge eval: CROSS_ARTIFACT edges on real repositories.

Acceptance criterion 4 of docs/CROSS-ARTIFACT-EDGES-WIP.md asks whether
cross-artifact edges stay trustworthy in review. This harness measures it for
Terraform: it clones each repository of ``eval/terraform_repos.yaml`` at its
pinned commit, builds the graph with the development ``dagayn``, reads the
Terraform ``CROSS_ARTIFACT`` and module ``IMPORTS_FROM`` edges from the
store, and scores them against the hand-written ground truth.

Each ground-truth entry is one literal reference from Terraform to code: a
``source`` node, the attribute, its ``value``, the code it names
(``target``: a path, a ``file::symbol``, or a list when the value alone names
several candidates) and whether that target is in the tree (``exists``).
Each emitted edge lands in one outcome:

- ``tp``: it names an expected target that exists;
- ``fp``: it names a target that is not in the tree, or one no entry expects;
- ``unresolved``: an entrypoint left ``<unresolved:...>`` (not reportable,
  so neither precision nor recall counts it);

An expected entry whose target exists and that no edge names is a ``fn``.
Families ``path_attr``, ``local_exec`` and ``entrypoint`` are what the
extractor covers (resource and data blocks); ``module_input``,
``object_source``, ``external_program`` and ``file_call`` are references it
does not cover and count only toward recall over all families.
``module_source`` is scored apart against ``IMPORTS_FROM``: recorded (the raw
source string is an edge target) and resolved (the target is the module
directory's node).

``--review N`` also runs ``review_tool(mode="changes")`` on the last N
commits touching ``.tf`` files of each repository and lists every
``bridge_touched`` finding, joined with the hand verdicts in the fixture's
``review_judgments``.

The network is needed for the first clone, so this is not wired into CI::

    uv run python eval/run_terraform_eval.py --work-dir /tmp/tf-repos
    uv run python eval/run_terraform_eval.py --work-dir /tmp/tf-repos --review 10
    uv run python eval/run_terraform_eval.py --work-dir /tmp/tf-repos --gate

Controls (rerun before trusting a change to the scoring)::

    # one expected entry removed: its edge must turn into an fp
    ... --reuse-graph --control drop-truth
    # one fake entry added: it must show as an fn
    ... --reuse-graph --control fake-truth
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import sqlite3
import subprocess
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import yaml

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / "eval" / "terraform_repos.yaml"
THRESHOLDS = ROOT / "eval" / "terraform_thresholds.yaml"

EXTRACTED_FAMILIES = ("path_attr", "local_exec", "entrypoint")
UNCOVERED_FAMILIES = ("module_input", "object_source", "external_program", "file_call")
FAMILY_OF_EVIDENCE = {
    "filename": "path_attr",
    "source_dir": "path_attr",
    "source_file": "path_attr",
    "source_directory": "path_attr",
    "provisioner.local-exec.command": "local_exec",
    "handler": "entrypoint",
    "entry_point": "entrypoint",
}


class InconclusiveError(RuntimeError):
    """The run cannot score: a missing graph, a wrong checkout, a failed build."""


@dataclass(frozen=True)
class Edge:
    source: str
    target: str
    line: int
    evidence_source: str
    symbol: str | None

    @property
    def family(self) -> str:
        return FAMILY_OF_EVIDENCE.get(self.evidence_source, f"other:{self.evidence_source}")


def _run(cmd: list[str], cwd: Path | None = None, timeout: float = 900) -> str:
    env = {k: v for k, v in os.environ.items() if k != "CRG_DATA_DIR"}
    done = subprocess.run(
        cmd, cwd=cwd, capture_output=True, text=True, timeout=timeout, env=env, check=False
    )
    if done.returncode != 0:
        raise InconclusiveError(f"{shlex.join(cmd)} exited {done.returncode}: {done.stderr[-600:]}")
    return done.stdout


def _git(repo: Path, *args: str) -> str:
    return _run(["git", "-C", str(repo), "-c", "advice.detachedHead=false", *args])


def ensure_checkout(repo: dict[str, Any], work_dir: Path) -> Path:
    """Clone the repository (history, no blobs up front) or reuse a clone, at
    its pinned commit. Never runs anything from the repository."""
    path = work_dir / repo["name"]
    if not (path / ".git").exists():
        work_dir.mkdir(parents=True, exist_ok=True)
        _run(
            [
                "git",
                "clone",
                "--quiet",
                "--filter=blob:none",
                "--no-checkout",
                repo["url"],
                str(path),
            ]
        )
    head = _git(path, "rev-parse", "HEAD").strip() if _has_head(path) else ""
    if head != repo["sha"]:
        dirty = [
            line
            for line in _git(path, "status", "--porcelain").splitlines()
            if ".dagayn" not in line
        ]
        if dirty:
            raise InconclusiveError(f"{path} has local changes: {dirty[:3]}")
        _git(path, "checkout", "--quiet", repo["sha"])
    if _git(path, "rev-parse", "HEAD").strip() != repo["sha"]:
        raise InconclusiveError(f"{path} is not at {repo['sha']}")
    return path


def _has_head(path: Path) -> bool:
    try:
        _git(path, "rev-parse", "--verify", "--quiet", "HEAD")
        return True
    except InconclusiveError:
        return False


def build(path: Path, dagayn_cmd: list[str]) -> None:
    _run([*dagayn_cmd, "build", "--repo", str(path)], cwd=ROOT)


def graph_db(path: Path) -> Path:
    db = path / ".dagayn" / "graph.db"
    if not db.is_file():
        raise InconclusiveError(f"no graph at {db}")
    return db


def read_edges(db: Path) -> tuple[list[Edge], list[tuple[str, str]], set[str]]:
    """Terraform CROSS_ARTIFACT edges, module IMPORTS_FROM edges, node names."""
    with sqlite3.connect(f"file:{db}?mode=ro", uri=True) as conn:
        bridges = [
            Edge(source, target, line or 0, evidence or "", symbol)
            for source, target, line, evidence, symbol in conn.execute(
                "SELECT source_qualified, target_qualified, line, "
                "json_extract(extra, '$.evidence_source'), "
                "json_extract(extra, '$.original_symbol_name') FROM edges "
                "WHERE kind = 'CROSS_ARTIFACT' "
                "AND json_extract(extra, '$.source_language') = 'terraform'"
            )
        ]
        modules = list(
            conn.execute(
                "SELECT source_qualified, target_qualified FROM edges "
                "WHERE kind = 'IMPORTS_FROM' AND source_qualified LIKE '%.tf::module.%'"
            )
        )
        nodes = {name for (name,) in conn.execute("SELECT qualified_name FROM nodes")}
    return bridges, modules, nodes


def _targets(entry: dict[str, Any]) -> list[str]:
    target = entry["target"]
    return list(target) if isinstance(target, list) else [target]


def _in_tree(path: Path, target: str) -> bool:
    file = target.split("::", 1)[0]
    return bool(file) and not file.startswith("<") and (path / file).exists()


def score_repo(
    path: Path, truth: list[dict[str, Any]], bridges: list[Edge], modules, nodes
) -> dict[str, Any]:
    counts: Counter[tuple[str, str]] = Counter()
    rows: list[dict[str, Any]] = []
    matched: set[int] = set()
    by_source: dict[str, list[tuple[int, dict[str, Any]]]] = {}
    for index, entry in enumerate(truth):
        if entry["family"] != "module_source":
            by_source.setdefault(entry["source"], []).append((index, entry))

    for edge in bridges:
        candidates = by_source.get(edge.source, [])
        if edge.target.startswith("<unresolved:"):
            hit = next((i for i, e in candidates if e["value"] == edge.symbol), None)
            if hit is not None:
                matched.add(hit)
            outcome, cause = (
                "unresolved",
                "ambiguous"
                if hit is not None and len(_targets(truth[hit])) > 1
                else ("no_match" if hit is not None else "unexpected"),
            )
        else:
            hit = next((i for i, e in candidates if edge.target in _targets(e)), None)
            if hit is not None and truth[hit]["exists"] and _in_tree(path, edge.target):
                matched.add(hit)
                outcome, cause = "tp", ""
            elif hit is not None:
                matched.add(hit)
                outcome, cause = "fp", "target_not_in_tree"
            else:
                outcome = "fp"
                cause = (
                    "wrong_target" if _in_tree(path, edge.target) else "unexpected_missing_target"
                )
        counts[(edge.family, outcome)] += 1
        if cause == "target_not_in_tree":
            counts[(edge.family, "fp_real_reference")] += 1
        if outcome != "tp":
            rows.append(
                {
                    "outcome": outcome,
                    "cause": cause,
                    "family": edge.family,
                    "source": edge.source,
                    "target": edge.target,
                    "line": edge.line,
                }
            )

    for index, entry in enumerate(truth):
        family = entry["family"]
        if family == "module_source":
            continue
        if entry["exists"] and len(_targets(entry)) == 1:
            counts[(family, "expected")] += 1
            if index not in matched:
                counts[(family, "fn")] += 1
                rows.append(
                    {
                        "outcome": "fn",
                        "family": family,
                        "at": entry["at"],
                        "source": entry["source"],
                        "target": entry["target"],
                    }
                )
        elif len(_targets(entry)) > 1:
            counts[(family, "ambiguous")] += 1

    module_edges = set(modules)
    module_truth = [e for e in truth if e["family"] == "module_source"]
    recorded = sum((e["source"], e["value"]) in module_edges for e in module_truth)
    resolved = sum(
        any((e["source"], t) in module_edges and t in nodes for t in _targets(e))
        for e in module_truth
    )
    return {
        "counts": {f"{family}/{outcome}": n for (family, outcome), n in sorted(counts.items())},
        "module_source": {
            "expected": len(module_truth),
            "recorded": recorded,
            "resolved": resolved,
        },
        "rows": rows,
    }


def summarize(per_repo: dict[str, dict[str, Any]]) -> dict[str, Any]:
    total: Counter[str] = Counter()
    for result in per_repo.values():
        total.update(result["counts"])

    def ratio(numerator: int, denominator: int) -> float | None:
        return round(numerator / denominator, 3) if denominator else None

    def family_metrics(counts: Counter[str] | dict[str, int], family: str) -> dict[str, Any]:
        tp, fp = counts.get(f"{family}/tp", 0), counts.get(f"{family}/fp", 0)
        expected, fn = counts.get(f"{family}/expected", 0), counts.get(f"{family}/fn", 0)
        return {
            "tp": tp,
            "fp": fp,
            "fn": fn,
            "expected": expected,
            "unresolved": counts.get(f"{family}/unresolved", 0),
            "ambiguous": counts.get(f"{family}/ambiguous", 0),
            "precision": ratio(tp, tp + fp),
            "recall": ratio(expected - fn, expected),
        }

    families = {f: family_metrics(total, f) for f in (*EXTRACTED_FAMILIES, *UNCOVERED_FAMILIES)}
    extracted = {
        k: sum(families[f][k] for f in EXTRACTED_FAMILIES) for k in ("tp", "fp", "fn", "expected")
    }
    every = {k: sum(families[f][k] for f in families) for k in ("tp", "fp", "fn", "expected")}
    real_reference = sum(total.get(f"{f}/fp_real_reference", 0) for f in EXTRACTED_FAMILIES)
    modules: Counter[str] = Counter()
    for result in per_repo.values():
        modules.update(result["module_source"])
    return {
        "families": families,
        "extracted_precision": ratio(extracted["tp"], extracted["tp"] + extracted["fp"]),
        # An fp whose value is a real reference to something outside the
        # tree (a build output, a submodule path): right reading, no code.
        "extracted_reference_precision": ratio(
            extracted["tp"] + real_reference, extracted["tp"] + extracted["fp"]
        ),
        "extracted_recall": ratio(extracted["expected"] - extracted["fn"], extracted["expected"]),
        "extracted_true_positives": extracted["tp"],
        "extracted_false_positives": extracted["fp"],
        "all_families_recall": ratio(every["expected"] - every["fn"], every["expected"]),
        "module_source": dict(modules),
        "per_repo": {
            name: {
                f: family_metrics(result["counts"], f)
                for f in families
                if any(k.startswith(f"{f}/") for k in result["counts"])
            }
            for name, result in per_repo.items()
        },
    }


def tf_commits(path: Path, count: int) -> list[str]:
    return _git(
        path, "log", f"-{count}", "--format=%H", "--first-parent", "HEAD", "--", "*.tf"
    ).split()


def review_commit(path: Path, commit: str, dagayn_cmd: list[str]) -> list[dict[str, Any]]:
    _git(path, "checkout", "--quiet", commit)
    build(path, dagayn_cmd)
    out = _run(
        [
            *dagayn_cmd,
            "tool",
            "review_tool",
            "--repo",
            str(path),
            "--arg",
            'mode="changes"',
            "--arg",
            'detail_level="minimal"',
            "--arg",
            f"base={json.dumps(commit + '~1')}",
        ],
        cwd=ROOT,
    )
    start = out.find("{")
    payload, _ = json.JSONDecoder().raw_decode(out, start)
    if not isinstance(payload.get("findings"), list):
        raise InconclusiveError(f"review_tool at {commit} returned no findings list")
    return [f for f in payload["findings"] if f.get("kind") == "bridge_touched"]


def run_reviews(
    repo: dict[str, Any], path: Path, count: int, dagayn_cmd: list[str]
) -> list[dict[str, Any]]:
    verdicts = {(j["commit"], j["target"]): j for j in repo.get("review_judgments", [])}
    rows: list[dict[str, Any]] = []
    try:
        for commit in tf_commits(path, count):
            changed = _git(path, "diff", "--name-only", f"{commit}~1", commit).split()
            firings = review_commit(path, commit, dagayn_cmd)
            if not firings:
                rows.append(
                    {
                        "commit": commit,
                        "tf_files": [f for f in changed if f.endswith(".tf")],
                        "bridge_touched": None,
                    }
                )
            for finding in firings:
                verdict = verdicts.get((commit, finding.get("qualified_name")), {})
                rows.append(
                    {
                        "commit": commit,
                        "bridge_touched": finding.get("qualified_name"),
                        "evidence": [
                            (e.get("source"), e.get("line")) for e in finding.get("evidence", [])
                        ],
                        "correct": verdict.get("correct"),
                        "why": verdict.get("why", "unjudged"),
                    }
                )
    finally:
        _git(path, "checkout", "--quiet", repo["sha"])
    return rows


def apply_control(repos: list[dict[str, Any]], control: str | None) -> None:
    if control is None:
        return
    for repo in repos:
        for index, entry in enumerate(repo["bridges"]):
            if (
                entry["family"] in EXTRACTED_FAMILIES
                and entry["exists"]
                and len(_targets(entry)) == 1
            ):
                if control == "drop-truth":
                    del repo["bridges"][index]
                else:
                    repo["bridges"].append(
                        {**entry, "target": entry["target"] + ".control", "at": "control"}
                    )
                return
    raise SystemExit("no entry to apply the control to")


def gate(summary: dict[str, Any], thresholds: dict[str, Any]) -> list[str]:
    failures = []
    for metric, floor in thresholds.get("floors", {}).items():
        value = summary.get(metric)
        if value is None or value < floor:
            failures.append(f"{metric} {value} is below its floor {floor}")
    for metric, ceiling in thresholds.get("ceilings", {}).items():
        value = summary.get(metric)
        if value is None or value > ceiling:
            failures.append(f"{metric} {value} is above its ceiling {ceiling}")
    return failures


def format_report(result: dict[str, Any]) -> str:
    summary = result["summary"]
    lines = [
        f"extracted families: precision {summary['extracted_precision']} "
        f"(reference {summary['extracted_reference_precision']})  "
        f"recall {summary['extracted_recall']}  "
        f"(tp {summary['extracted_true_positives']}, fp {summary['extracted_false_positives']})",
        f"all families recall: {summary['all_families_recall']}  "
        f"module_source: {summary['module_source']}",
        "",
        "| repo | family | expected | tp | fp | fn | unresolved | precision | recall |",
        "|---|---|---|---|---|---|---|---|---|",
    ]
    for repo, families in summary["per_repo"].items():
        for family, m in families.items():
            lines.append(
                f"| {repo} | {family} | {m['expected']} | {m['tp']} | {m['fp']} | {m['fn']} | "
                f"{m['unresolved']} | {m['precision']} | {m['recall']} |"
            )
    lines.append("")
    for repo, data in result["repos"].items():
        for row in data["rows"]:
            if row["outcome"] != "fn" or row["family"] in EXTRACTED_FAMILIES:
                lines.append(f"{repo}: {json.dumps(row, sort_keys=True)}")
    for repo, rows in result.get("reviews", {}).items():
        fired = [r for r in rows if r["bridge_touched"]]
        lines.append(
            f"\nreview {repo}: {len(rows) and len({r['commit'] for r in rows})} commits, "
            f"{len(fired)} bridge_touched"
        )
        for row in fired:
            lines.append(
                f"  {row['commit'][:10]} {row['bridge_touched']} "
                f"correct={row['correct']} {row['why']}"
            )
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--fixture", type=Path, default=FIXTURE)
    parser.add_argument("--work-dir", type=Path, required=True, help="clones live here")
    parser.add_argument("--repo", action="append", default=[], help="only this repository")
    parser.add_argument("--dagayn-cmd", default=str(ROOT / ".venv" / "bin" / "dagayn"))
    parser.add_argument("--reuse-graph", action="store_true", help="score the existing graph")
    parser.add_argument("--review", type=int, default=0, help="review the last N .tf commits")
    parser.add_argument("--control", choices=("drop-truth", "fake-truth"))
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--gate", action="store_true")
    parser.add_argument("--thresholds", type=Path, default=THRESHOLDS)
    args = parser.parse_args()

    dagayn_cmd = shlex.split(args.dagayn_cmd)
    repos = yaml.safe_load(args.fixture.read_text(encoding="utf-8"))["repos"]
    if args.repo:
        repos = [r for r in repos if r["name"] in args.repo]
    apply_control(repos, args.control)
    result: dict[str, Any] = {"repos": {}, "reviews": {}}
    try:
        for repo in repos:
            path = ensure_checkout(repo, args.work_dir.resolve())
            if not args.reuse_graph:
                build(path, dagayn_cmd)
            bridges, modules, nodes = read_edges(graph_db(path))
            result["repos"][repo["name"]] = score_repo(
                path, repo["bridges"], bridges, modules, nodes
            )
            if args.review:
                result["reviews"][repo["name"]] = run_reviews(repo, path, args.review, dagayn_cmd)
                build(path, dagayn_cmd)
    except InconclusiveError as exc:
        print(f"INCONCLUSIVE {exc}", file=sys.stderr)
        return 2
    result["summary"] = summarize(result["repos"])
    print(json.dumps(result, indent=2) if args.json else format_report(result))
    if args.gate:
        failures = gate(
            result["summary"], yaml.safe_load(args.thresholds.read_text(encoding="utf-8"))
        )
        for failure in failures:
            print(f"GATE FAILED {failure}")
        return 1 if failures else 0
    return 0


if __name__ == "__main__":
    sys.exit(main())
