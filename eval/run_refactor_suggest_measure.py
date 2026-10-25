#!/usr/bin/env python3
"""Measure ``refactor_tool(mode="suggest")`` on real repositories.

Runs suggest on each measured clone at ``minimal``, ``standard`` (the
default), and ``verbose``, saves every raw reply, and counts findings per
kind and reply size.  When ``refactor_suggest_judgments.yaml`` exists it
scores the hand verdicts (actionable / false / noise) per kind; it refuses a
judged id that is not a finding in the standard reply or not a node in the
repository's graph.

    python eval/run_refactor_suggest_measure.py [--measure-dir DIR]
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

import yaml

HERE = Path(__file__).resolve().parent
DAGAYN = HERE.parent / ".venv" / "bin" / "dagayn"
REPOS = ("dagayn", "cfwdon", "purecrate-ts", "tstodon")
LEVELS = ("minimal", "standard", "verbose")
VERDICTS = ("actionable", "false", "noise")
JUDGMENTS = HERE / "refactor_suggest_judgments.yaml"


def call(repo: Path, level: str) -> tuple[str, dict | None, str | None]:
    cmd = [str(DAGAYN), "tool", "refactor_tool", "--repo", str(repo)]
    cmd += ["--arg", 'mode="suggest"', "--arg", f'detail_level="{level}"']
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
    except subprocess.TimeoutExpired:
        return "", None, "timeout"
    try:
        reply = json.loads(proc.stdout)
    except json.JSONDecodeError:
        return proc.stdout, None, f"unparsable (rc={proc.returncode}) {proc.stderr[-200:]}"
    if reply.get("status") != "ok":
        return proc.stdout, reply, f"status={reply.get('status')}"
    return proc.stdout, reply, None


def node_exists(repo: Path, qualified_name: str) -> bool:
    db = sqlite3.connect(f"file:{repo}/.dagayn/graph.db?mode=ro", uri=True)
    try:
        row = db.execute(
            "SELECT 1 FROM nodes WHERE qualified_name = ? OR file_path = ? LIMIT 1",
            (qualified_name, qualified_name),
        ).fetchone()
    finally:
        db.close()
    return row is not None


def finding_id(finding: dict) -> str:
    return finding.get("qualified_name") or finding.get("file") or finding.get("target") or ""


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--measure-dir", type=Path, required=True)
    args = parser.parse_args()
    out = args.measure_dir / "raw" / "refactor_suggest"
    out.mkdir(parents=True, exist_ok=True)

    errors: list[str] = []
    summary: dict = {"repos": {}}
    standard_findings: dict[str, dict[tuple[str, str], dict]] = {}
    for name in REPOS:
        repo = args.measure_dir / name
        row: dict = {}
        for level in LEVELS:
            text, reply, err = call(repo, level)
            (out / f"{name}.{level}.json").write_text(text)
            if err:
                errors.append(f"{name}/{level}: {err}")
                row[level] = {"chars": len(text), "error": err}
                continue
            findings = reply.get("findings", [])
            row[level] = {
                "chars": len(text),
                "findings": len(findings),
                "by_kind": dict(Counter(f["kind"] for f in findings)),
                "summary": reply.get("summary"),
            }
            if level == "standard":
                standard_findings[name] = {(f["kind"], finding_id(f)): f for f in findings}
        summary["repos"][name] = row

    if JUDGMENTS.exists():
        rows = yaml.safe_load(JUDGMENTS.read_text())["judgments"]
        tally: dict[str, Counter] = defaultdict(Counter)
        for j in rows:
            key = (j["kind"], j["id"])
            if j["verdict"] not in VERDICTS:
                sys.exit(f"bad verdict {j['verdict']!r} for {j['id']}")
            if key not in standard_findings.get(j["repo"], {}):
                sys.exit(f"judged {key} is not a finding in {j['repo']}'s standard reply")
            if not node_exists(args.measure_dir / j["repo"], j["id"]):
                sys.exit(f"judged id {j['id']} is not a node in {j['repo']}'s graph")
            tally[j["kind"]][j["verdict"]] += 1
            tally["ALL"][j["verdict"]] += 1
        summary["judged"] = {
            kind: {
                **c,
                "n": sum(c.values()),
                "precision": round(c["actionable"] / sum(c.values()), 3),
            }
            for kind, c in tally.items()
        }

    summary["errors"] = errors
    (out / "summary.json").write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary, indent=2))
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
