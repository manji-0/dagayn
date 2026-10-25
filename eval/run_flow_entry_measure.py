#!/usr/bin/env python3
"""flow_tool(mode="entry_points") on real repositories, against hand judgments.

For each target in ``flow_entry_judgments.yaml`` this runs
``flow_tool(mode="entry_points")`` (default limit, ``standard`` and
``minimal``; ``limit=50`` for the full set) and
``query_graph_tool(pattern="callers_of", depth=6)``, and scores both against
the hand-established ways in (``ways_in``: the outside boundaries, such as a
Worker event handler, ``main``, an HTTP route, a CLI subcommand, an exported
package symbol, or a trait method a framework calls, whose execution reaches
the target).

Each returned entry point and each callers_of-derived root is judged:

- ``boundary``: it is a way in;
- ``nearest_handler``: the first handler one dispatch hop inside a way in
  (a route handler, a queue job handler), which the tool's contract reports
  in place of the way in;
- ``helper``: a real step on a real path that is neither;
- ``wrong``: no real call path from it reaches the target.

A callers_of root is a row the reply shows that has no production caller in
the graph. The reply carries no edges, so this reads the graph to find them:
it is the most an agent could derive from that reply with one more hop.

Precision is (boundary + nearest_handler) over judged items (strict:
boundary only). Recall is the share of ways in that some such item maps to
(``way_in``). Micro-averaged over all targets.

Usage::

    python eval/run_flow_entry_measure.py --measure-root $S/measure --out $S/measure/raw/flow_entry
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import statistics
import subprocess
import sys
from pathlib import Path
from typing import Any

import yaml

HERE = Path(__file__).resolve().parent
JUDGMENTS = HERE / "flow_entry_judgments.yaml"
DAGAYN = HERE.parent / ".venv" / "bin" / "dagayn"
GOOD = {"boundary", "nearest_handler"}
VERDICTS = GOOD | {"helper", "wrong"}
TEST_DIRS = {"tests", "test", "__tests__", "spec", "testdata", "fixtures"}


def call(repo: Path, tool: str, args: dict[str, Any]) -> tuple[str, dict[str, Any] | None, str]:
    argv = [str(DAGAYN), "tool", tool, "--repo", str(repo)]
    for key, value in args.items():
        argv += ["--arg", f"{key}={json.dumps(value)}"]
    try:
        proc = subprocess.run(argv, capture_output=True, text=True, timeout=120, check=False)
    except subprocess.TimeoutExpired:
        return "", None, "timeout"
    try:
        reply = json.loads(proc.stdout)
    except json.JSONDecodeError:
        return proc.stdout, None, f"bad json (exit {proc.returncode}): {proc.stderr[-200:]}"
    status = reply.get("status")
    return proc.stdout, reply, "" if status == "ok" else f"status={status}"


def is_test(row: sqlite3.Row) -> bool:
    path = row["file_path"]
    parts = path.split("/")
    name = parts[-1]
    return bool(
        row["is_test"]
        or TEST_DIRS.intersection(parts[:-1])
        or ".test." in name
        or ".spec." in name
        or name.startswith("test_")
        or name.endswith("_test.rs")
        or name == "tests.rs"
    )


class Graph:
    def __init__(self, db: Path) -> None:
        self.db = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
        self.db.row_factory = sqlite3.Row

    def node(self, qn: str) -> sqlite3.Row | None:
        return self.db.execute("select * from nodes where qualified_name = ?", (qn,)).fetchone()

    def production_callers(self, qn: str) -> list[str]:
        rows = self.db.execute(
            "select distinct n.* from edges e join nodes n on n.qualified_name = e.source_qualified"
            " where e.kind = 'CALLS' and e.target_qualified = ?",
            (qn,),
        ).fetchall()
        return [r["qualified_name"] for r in rows if not is_test(r)]

    def is_production_root(self, qn: str) -> bool:
        """A production function, or a script file, that nothing in production calls."""
        node = self.node(qn)
        if node is None or is_test(node):
            return False
        if node["kind"] == "File" and node["language"] == "rust":
            return False
        return node["kind"] in ("Function", "File") and not self.production_callers(qn)

    def has_flow_edge(self, source: str, target: str) -> bool:
        return (
            self.db.execute(
                "select 1 from edges where kind in ('CALLS', 'CROSS_ARTIFACT')"
                " and source_qualified = ? and target_qualified = ? limit 1",
                (source, target),
            ).fetchone()
            is not None
        )


def score(
    items: list[dict[str, Any]], judged: dict[str, dict[str, Any]], ways: set[str]
) -> dict[str, Any]:
    verdicts = [judged[i["qn"]] for i in items if i["qn"] in judged]
    good = [v for v in verdicts if v["verdict"] in GOOD]
    return {
        "returned": len(items),
        "judged": len(verdicts),
        "good": len(good),
        "boundary": sum(v["verdict"] == "boundary" for v in verdicts),
        "covered": sorted(ways & {v.get("way_in") for v in good}),
        "covered_strict": sorted(
            ways & {v.get("way_in") for v in verdicts if v["verdict"] == "boundary"}
        ),
        "unjudged": [i["qn"] for i in items if i["qn"] not in judged],
    }


def measure(target: dict[str, Any], repo: Path, out: Path, errors: list[str]) -> dict[str, Any]:
    qn = target["target"]
    graph = Graph(repo / ".dagayn" / "graph.db")
    row: dict[str, Any] = {
        "id": target["id"],
        "repo": target["repo"],
        "lang": target["lang"],
        "target": qn,
    }
    replies = {
        "ep_standard": ("flow_tool", {"mode": "entry_points", "target": qn}),
        "ep_minimal": (
            "flow_tool",
            {"mode": "entry_points", "target": qn, "detail_level": "minimal"},
        ),
        "ep_full": (
            "flow_tool",
            {"mode": "entry_points", "target": qn, "limit": 50, "detail_level": "minimal"},
        ),
        "callers": ("query_graph_tool", {"pattern": "callers_of", "target": qn, "depth": 6}),
    }
    got: dict[str, dict[str, Any]] = {}
    for key, (tool, args) in replies.items():
        text, reply, error = call(repo, tool, args)
        (out / f"{target['id']}.{key}.json").write_text(text)
        row[f"chars_{key}"] = len(text)
        if error:
            errors.append(f"{target['id']} {key}: {error}")
        got[key] = reply or {}

    full = got["ep_full"]
    entries = [
        {"qn": e["entry_point"], "kind": e["kind"], "chain": e["chain"]}
        for e in full.get("entry_points", [])
    ]
    shown_default = [{"qn": e["entry_point"]} for e in got["ep_standard"].get("entry_points", [])]
    row["ep_omitted_default"] = got["ep_standard"].get("entry_points_omitted")
    row["ep_truncated"] = full.get("truncated")
    row["ep_bad_hops"] = [
        f"{a} -> {b}"
        for e in entries
        for a, b in zip(e["chain"], e["chain"][1:])
        if not graph.has_flow_edge(a, b)
    ]
    callers = got["callers"]
    shown = callers.get("results", [])
    row["callers_total"] = callers.get("result_count")
    row["callers_shown"] = len(shown)
    row["callers_truncated"] = callers.get("truncated")
    row["callers_max_depth_shown"] = max((r.get("depth", 0) for r in shown), default=0)
    roots = [
        {"qn": r["qualified_name"], "depth": r.get("depth")}
        for r in shown
        if graph.is_production_root(r["qualified_name"])
    ]

    for qn_ in [w["node"] for w in target.get("ways_in", []) if w.get("node")] + [
        j["qn"] for key in ("entry_points", "callers_roots") for j in target.get(key, [])
    ]:
        if graph.node(qn_) is None:
            sys.exit(f"{target['id']}: judged id not in the {target['repo']} graph: {qn_}")
    for key in ("entry_points", "callers_roots"):
        for j in target.get(key, []):
            if j["verdict"] not in VERDICTS:
                sys.exit(f"{target['id']}: unknown verdict {j['verdict']!r} for {j['qn']}")

    ways = {w["id"] for w in target.get("ways_in", [])}
    row["ways_in"] = sorted(ways)
    row["ep"] = score(entries, {j["qn"]: j for j in target.get("entry_points", [])}, ways)
    row["ep"]["kinds"] = {e["qn"]: e["kind"] for e in entries}
    row["ep_default"] = score(
        shown_default, {j["qn"]: j for j in target.get("entry_points", [])}, ways
    )
    row["roots"] = score(roots, {j["qn"]: j for j in target.get("callers_roots", [])}, ways)
    row["roots"]["depths"] = {r["qn"]: r["depth"] for r in roots}
    return row


def micro(rows: list[dict[str, Any]], key: str) -> dict[str, Any]:
    judged = sum(r[key]["judged"] for r in rows)
    ways = sum(len(r["ways_in"]) for r in rows)
    with_ways = [r for r in rows if r["ways_in"]]

    def ratio(a: int, b: int) -> float | None:
        return round(a / b, 3) if b else None

    return {
        "returned": sum(r[key]["returned"] for r in rows),
        "precision": ratio(sum(r[key]["good"] for r in rows), judged),
        "precision_strict": ratio(sum(r[key]["boundary"] for r in rows), judged),
        "recall": ratio(sum(len(r[key]["covered"]) for r in rows), ways),
        "recall_strict": ratio(sum(len(r[key]["covered_strict"]) for r in rows), ways),
        "ways_in": ways,
        "unjudged": sum(len(r[key]["unjudged"]) for r in rows),
        "macro_recall": round(
            statistics.mean(len(r[key]["covered"]) / len(r["ways_in"]) for r in with_ways), 3
        )
        if with_ways
        else None,
        "targets_with_zero_recall": [r["id"] for r in with_ways if not r[key]["covered"]],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--measure-root", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--judgments", type=Path, default=JUDGMENTS)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    targets = yaml.safe_load(args.judgments.read_text())["targets"]
    errors: list[str] = []
    rows = [measure(t, args.measure_root / t["repo"], args.out, errors) for t in targets]
    summary = {
        "targets": len(rows),
        "by_lang": {
            lang: sum(r["lang"] == lang for r in rows) for lang in sorted({r["lang"] for r in rows})
        },
        "errors": errors,
        "entry_points": micro(rows, "ep"),
        "entry_points_default_limit": micro(rows, "ep_default"),
        "callers_of_roots": micro(rows, "roots"),
        "median_chars": {
            key: statistics.median(r[f"chars_{key}"] for r in rows)
            for key in ("ep_standard", "ep_minimal", "callers")
        },
        "callers_truncated": sum(bool(r["callers_truncated"]) for r in rows),
        "ep_bad_hops": sum(len(r["ep_bad_hops"]) for r in rows),
        "rows": rows,
    }
    (args.out / "summary.json").write_text(json.dumps(summary, indent=2))
    for r in rows:
        print(
            f"{r['id']:4} {r['lang']:4}"
            f" ep={r['ep']['good']}/{r['ep']['judged']}/{r['ep']['returned']}"
            f" cov={len(r['ep']['covered'])}/{len(r['ways_in'])}"
            f" default_cov={len(r['ep_default']['covered'])}"
            f" roots={r['roots']['good']}/{r['roots']['judged']}/{r['roots']['returned']}"
            f" cov={len(r['roots']['covered'])}/{len(r['ways_in'])}"
            f" chars={r['chars_ep_standard']}/{r['chars_ep_minimal']}/{r['chars_callers']}"
            f" callers={r['callers_shown']}/{r['callers_total']} {r['target']}"
        )
    print(json.dumps({k: v for k, v in summary.items() if k != "rows"}, indent=2))
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
