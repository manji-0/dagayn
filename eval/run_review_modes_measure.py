#!/usr/bin/env python3
"""Measure review_tool's impact and context modes against changes.

For each picked commit C, a private clone is checked out at C, the graph is
brought to C (one ``dagayn build`` per clone, then ``dagayn update``), and
review_tool runs with ``base=C~1`` in modes changes, impact, and context at
their defaults. Each reply's references become a set of (file, qualified_name);
impact-minus-changes and context-minus-changes are classified against the
graph at C:

- sibling: an unchanged node in a changed file (every node of a changed file
  seeds the radius)
- caller / test / doc: has a non-CONTAINS edge into a symbol the diff touched
  (changes' ``changed_functions``); these are the candidates judged by hand
- callee: a changed symbol points at it
- file / other_doc / indirect: a container file node, an unlinked doc node, or
  anything reached only through an unchanged sibling or a second hop

The full radius is recomputed from SQLite (both directions, depth 2,
low-confidence bridges skipped) and its count is checked against the tool's
``total_impacted``, so "context keeps k of n" compares against every candidate
the radius holds, not only the ones the reply kept.

    run_review_modes_measure.py run [REPO ...]   # clone, build, measure
    run_review_modes_measure.py summarize        # fold in judgments

``DAGAYN_MEASURE_DIR`` (default ``./measure``) holds one clone per repository
with a built graph; private clones and raw replies go under it.

``summarize`` refuses a judgment whose id is not a candidate recorded with a
graph node id at its commit.
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import statistics
import subprocess
import sys
import time
from collections import Counter
from pathlib import Path
from typing import Any

import yaml

W = Path(__file__).resolve().parent.parent
DAGAYN = str(W / ".venv/bin/dagayn")
SRC = Path(os.environ.get("DAGAYN_MEASURE_DIR", "measure")).resolve()
CLONES = SRC / "review_modes"
RAW = SRC / "raw" / "review_modes"
JUDGMENTS = Path(__file__).resolve().parent / "review_modes_judgments.yaml"
MODES = ("changes", "impact", "context")
TOOL_TIMEOUT = 600
CANDIDATE = ("caller", "test", "doc")
DOC_KINDS = {"DocSection", "DocBody"}
DEPENDENT_KINDS = {
    "CALLS",
    "REFERENCES",
    "IMPORTS_FROM",
    "INHERITS",
    "IMPLEMENTS",
    "DEPENDS_ON",
    "CROSS_ARTIFACT",
}

COMMITS: dict[str, list[tuple[str, str]]] = {
    "dagayn": [
        ("e09c5ec8", "Rust review logic: trait-method reach, 3 code files"),
        ("11f815bf", "one-file Rust fix in SCIP settlement, callers matter"),
        ("c8b5fedb", "removes a reply field across 11 code files; tests and callers must follow"),
        ("aebbe3be", "deletes a CLI command; dangling references possible"),
        ("9417ad7a", "deletes a duplicate argument parser used by the daemon"),
        ("1802feee", "changes the Rust test-walk algorithm, 3 code files"),
        ("84e7ba75", "small Python review-reach feature"),
        ("5df51e6b", "Python review-reach feature, 2 code files"),
        ("02e46263", "small Rust/Python fix to a count in query guidance"),
        ("67377eb3", "next-step retries, 6 code files across tools"),
        ("192b8094", "one-file flow search change"),
    ],
    "cfwdon": [
        ("6837e01", "Rust timeline cursor fix, 3 code files"),
        ("8b2023a", "Rust routing: HEAD answered like GET, new helper"),
        ("5e96d48", "Rust API validation across 5 files"),
        ("0b496c8", "TS web-ui ordering fix"),
        ("90ba47c", "Rust worker streaming change, 4 files, 299 churn"),
        ("0228a45", "Rust API: new exclude_direct parameter"),
        ("3b5ad59", "Rust worker boost streaming fix"),
        ("5e96c75", "one-file Rust streaming fix"),
        ("1d5c58d", "Rust config built once per isolate; callers change"),
        ("baed375", "one-file Rust perf refactor"),
        ("d45c7b8", "Rust list member batching, 2 files"),
    ],
    "purecrate-ts": [
        ("a198ea82", "one-file Rust typer refactor, 437 churn"),
        ("02aa4718", "flow narrowing fix with tests"),
        ("d503dbfc", "narrowing fold fix with tests"),
        ("8afc38d4", "printer fix with tests"),
        ("021bb708", "printer block fix with tests"),
        ("c17f2320", "const alias decision fix, 4 code files"),
        ("33b51967", "typed let fix, 4 code files"),
        ("0328efac", "feature: struct consts, 3 code files"),
        ("cfec53bb", "format! narrowing fix, 1 code file"),
        ("0d0d4fa9", "test runner method-call fix"),
        ("08db53d5", "variant list typing fix, 4 code files"),
    ],
    "tstodon": [
        ("90f9a0b", "TS streaming scope and DM reach fix, 8 code files"),
        ("65fd6bb", "one-file type-cycle refactor"),
        ("7c735f9", "TS feature: oEmbed route, 3 code files"),
        ("9bc81e9", "TS alias routes, 4 code files"),
        ("eebdb69", "TS federation sanitize fix, 3 code files"),
        ("2264280", "TS conversations thread fix, 2 files"),
        ("d40bfb1", "TS conversations domain-block fix"),
        ("03976f8", "TS push follower-policy fix"),
        ("8b116fc", "one-file TS suggestions fix"),
        ("2605df3", "TS federation host-port fix, 4 files"),
        ("4c66e18", "TS thread-context suppression fix, 3 code files"),
    ],
}


def git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(repo), *args], capture_output=True, text=True, check=True
    ).stdout.strip()


def dagayn(*args: str, timeout: int = 3600) -> tuple[int, str, str, float]:
    start = time.monotonic()
    try:
        proc = subprocess.run([DAGAYN, *args], capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return -1, "", "timeout", time.monotonic() - start
    return proc.returncode, proc.stdout, proc.stderr, time.monotonic() - start


def call_tool(repo: Path, mode: str, base: str) -> tuple[dict[str, Any] | None, dict[str, Any]]:
    rc, out, err, secs = dagayn(
        "tool",
        "review_tool",
        "--repo",
        str(repo),
        "--arg",
        f"mode={json.dumps(mode)}",
        "--arg",
        f"base={json.dumps(base)}",
        timeout=TOOL_TIMEOUT,
    )
    row: dict[str, Any] = {"mode": mode, "chars": len(out), "secs": round(secs, 2), "rc": rc}
    try:
        reply = json.loads(out)
    except json.JSONDecodeError:
        row["error"] = err.strip()[-300:] or "unparseable reply"
        return None, row
    if reply.get("status") != "ok":
        row["error"] = f"status={reply.get('status')}: {str(reply.get('summary', ''))[:200]}"
    return reply, row


def file_of(qn: str) -> str:
    return qn.split("::", 1)[0]


def refs_changes(reply: dict[str, Any]) -> set[tuple[str, str]]:
    refs = {(n["file_path"], n["qualified_name"]) for n in reply.get("changed_functions", [])}
    for f in reply.get("findings", []):
        if f.get("qualified_name"):
            refs.add((f["file"], f["qualified_name"]))
        for qn in [*f.get("targets", []), *f.get("covers", [])]:
            refs.add((file_of(qn), qn))
    return refs


def node_refs(nodes: list[dict[str, Any]]) -> set[tuple[str, str]]:
    return {(n["file_path"], n["qualified_name"]) for n in nodes}


def refs_impact(reply: dict[str, Any]) -> set[tuple[str, str]]:
    refs = node_refs(reply.get("changed_nodes", [])) | node_refs(reply.get("impacted_nodes", []))
    for bridge in reply.get("bridge_transitions", []):
        for end in ("source", "target"):
            refs.add((file_of(bridge[end]), bridge[end]))
    return refs


def refs_context(reply: dict[str, Any]) -> set[tuple[str, str]]:
    graph = reply.get("context", {}).get("graph", {})
    return node_refs(graph.get("changed_nodes", [])) | node_refs(graph.get("impacted_nodes", []))


def files_named(reply: dict[str, Any], mode: str) -> set[str]:
    if mode == "impact":
        return set(reply.get("impacted_files", []))
    if mode == "context":
        ctx = reply.get("context", {})
        return set(ctx.get("impacted_files", [])) | set(ctx.get("source_snippets", {}))
    return {f["file"] for f in reply.get("findings", [])}


def full_radius(
    conn: sqlite3.Connection, changed_files: list[str], depth: int = 2
) -> dict[str, int]:
    marks = ",".join("?" * len(changed_files))
    seeds = [
        r[0]
        for r in conn.execute(
            f"SELECT qualified_name FROM nodes WHERE file_path IN ({marks})", changed_files
        )
    ]
    visited = dict.fromkeys(seeds, 0)
    frontier = set(seeds)
    reportable = (
        "(kind != 'CROSS_ARTIFACT' OR (UPPER(COALESCE(confidence_tier,'EXTRACTED'))"
        " IN ('EXACT','HIGH','EXTRACTED')"
        " AND target_qualified NOT LIKE '<unresolved:%'"
        " AND source_qualified NOT LIKE '<unresolved:%'))"
    )
    for level in range(1, depth + 1):
        found: set[str] = set()
        chunk = list(frontier)
        for i in range(0, len(chunk), 400):
            part = chunk[i : i + 400]
            m = ",".join("?" * len(part))
            for (qn,) in conn.execute(
                f"SELECT target_qualified FROM edges"
                f" WHERE source_qualified IN ({m}) AND {reportable}",
                part,
            ):
                found.add(qn)
            for (qn,) in conn.execute(
                f"SELECT source_qualified FROM edges"
                f" WHERE target_qualified IN ({m}) AND {reportable}",
                part,
            ):
                found.add(qn)
        frontier = {qn for qn in found if qn not in visited}
        for qn in frontier:
            visited[qn] = level
    return {qn: d for qn, d in visited.items() if d > 0}


def classify(
    conn: sqlite3.Connection, qn: str, changed: set[str], changed_files: set[str]
) -> tuple[str, int | None, str | None, int | None]:
    node = conn.execute(
        "SELECT id, kind, file_path, is_test, line_start FROM nodes WHERE qualified_name = ?",
        (qn,),
    ).fetchone()
    if node is None:
        return "not_in_graph", None, None, None
    node_id, kind, path, is_test, line = node

    def tag(cls: str) -> tuple[str, int | None, str | None, int | None]:
        return cls, node_id, kind, line

    if path in changed_files:
        return tag("sibling")
    out_kinds = {
        k
        for (k, t) in conn.execute(
            "SELECT kind, target_qualified FROM edges WHERE source_qualified = ?", (qn,)
        )
        if t in changed
    }
    in_kinds = {
        k
        for (k, s) in conn.execute(
            "SELECT kind, source_qualified FROM edges WHERE target_qualified = ?", (qn,)
        )
        if s in changed
    }
    if kind in DOC_KINDS or (kind == "File" and path.endswith(".md")):
        return tag("doc" if (out_kinds | in_kinds) - {"CONTAINS"} else "other_doc")
    if kind == "File":
        # A TS route handler is an anonymous arrow, so its calls hang off the
        # file; an import alone is not a call.
        return tag("caller" if "CALLS" in out_kinds else "file")
    if is_test and (out_kinds & DEPENDENT_KINDS or "TESTED_BY" in in_kinds):
        return tag("test")
    if out_kinds & DEPENDENT_KINDS:
        return tag("caller")
    if in_kinds & DEPENDENT_KINDS:
        return tag("callee")
    return tag("indirect")


def bring_graph_to(clone: Path, sha: str, first: bool) -> dict[str, Any]:
    git(clone, "checkout", "-q", "--detach", sha)
    rc, out, err, secs = dagayn("build" if first else "update", "--repo", str(clone))
    _, status, _, _ = dagayn("status", "--repo", str(clone), timeout=120)
    synced = f"Built at commit: {sha[:12]}" in status and "commit_synced" in status
    return {
        "graph_cmd": "build" if first else "update",
        "graph_rc": rc,
        "graph_secs": round(secs, 1),
        "synced": synced,
        "graph_err": err.strip()[-300:] if rc else "",
    }


def measure_commit(
    clone: Path, repo: str, sha: str, reason: str, sync: dict[str, Any]
) -> dict[str, Any]:
    parent = git(clone, "rev-parse", f"{sha}^")
    out_dir = RAW / repo / sha[:8]
    out_dir.mkdir(parents=True, exist_ok=True)
    row: dict[str, Any] = {"repo": repo, "sha": sha, "reason": reason, **sync, "modes": {}}
    replies: dict[str, dict[str, Any]] = {}
    for mode in MODES:
        reply, mrow = call_tool(clone, mode, parent)
        row["modes"][mode] = mrow
        if reply is not None:
            (out_dir / f"{mode}.json").write_text(json.dumps(reply, indent=1))
            replies[mode] = reply
    if "changes" not in replies:
        (out_dir / "row.json").write_text(json.dumps(row, indent=1))
        return row
    changes = replies["changes"]
    changed_files = list(changes.get("changed_files", []))
    changed = {n["qualified_name"] for n in changes.get("changed_functions", [])}
    base_refs = refs_changes(changes)
    conn = sqlite3.connect(clone / ".dagayn" / "graph.db")
    radius = full_radius(conn, changed_files) if changed_files else {}
    radius_class = {qn: classify(conn, qn, changed, set(changed_files)) for qn in radius}
    row["changed_symbols"] = len(changed)
    row["changes_refs"] = len(base_refs)
    row["changes_findings"] = dict(Counter(f["kind"] for f in changes.get("findings", [])))
    row["radius_total"] = len(radius)
    row["radius_classes"] = dict(Counter(v[0] for v in radius_class.values()))
    radius_candidates = {qn for qn, v in radius_class.items() if v[0] in CANDIDATE}
    row["radius_candidates"] = sorted(radius_candidates)
    row["changes_named_radius_candidates"] = sorted(radius_candidates & {qn for _, qn in base_refs})
    for mode, extract in (("impact", refs_impact), ("context", refs_context)):
        reply = replies.get(mode)
        if reply is None:
            continue
        refs = extract(reply)
        added = refs - base_refs
        classes = {
            qn: radius_class.get(qn) or classify(conn, qn, changed, set(changed_files))
            for _, qn in added
        }
        impacted = (
            reply.get("impacted_nodes")
            if mode == "impact"
            else reply.get("context", {}).get("graph", {}).get("impacted_nodes")
        )
        tool_total = (
            reply.get("total_impacted")
            if mode == "impact"
            else (
                (reply.get("_truncation") or {}).get("impacted_nodes", {}).get("total")
                or (reply.get("_truncation") or {})
                .get("context.graph.impacted_nodes", {})
                .get("total")
                or len(impacted or [])
            )
        )
        row["modes"][mode].update(
            {
                "refs": len(refs),
                "added": len(added),
                "added_classes": dict(Counter(v[0] for v in classes.values())),
                "snippet_chars": sum(
                    len(v) for v in reply.get("context", {}).get("source_snippets", {}).values()
                ),
                "kept_impacted": len(impacted or []),
                "tool_total_impacted": tool_total,
                "kept_candidates": sum(
                    1
                    for n in impacted or []
                    if radius_class.get(n["qualified_name"], ("",))[0] in CANDIDATE
                ),
                "added_files": sorted(files_named(reply, mode) - set(changed_files)),
                "candidates": sorted(
                    (
                        {
                            "qn": qn,
                            "class": c,
                            "kind": kind,
                            "line": line,
                            "node_id": nid,
                            "depth": radius.get(qn),
                        }
                        for qn, (c, nid, kind, line) in classes.items()
                        if c in CANDIDATE
                    ),
                    key=lambda c: c["qn"],
                ),
            }
        )
    conn.close()
    (out_dir / "row.json").write_text(json.dumps(row, indent=1))
    return row


def run_repo(repo: str) -> None:
    clone = CLONES / repo
    if not clone.exists():
        subprocess.run(
            ["git", "clone", "-q", "--no-local", str(SRC / repo), str(clone)], check=True
        )
    shas = [git(clone, "rev-parse", short) for short, _ in COMMITS[repo]]
    reasons = dict(zip(shas, (r for _, r in COMMITS[repo]), strict=True))
    order = git(clone, "rev-list", "--topo-order", "--reverse", "--all").split()
    picked = [s for s in order if s in reasons]
    first = not (clone / ".dagayn" / "graph.db").exists()
    for sha in picked:
        if (RAW / repo / sha[:8] / "row.json").exists():
            print(f"{repo} {sha[:8]} cached", flush=True)
            first = False
            continue
        sync = bring_graph_to(clone, sha, first)
        first = False
        row = measure_commit(clone, repo, sha, reasons[sha], sync)
        chars = {m: r.get("chars") for m, r in row["modes"].items()}
        print(
            f"{repo} {sha[:8]} synced={row['synced']} chars={chars}"
            f" radius={row.get('radius_total')}",
            flush=True,
        )


def load_rows() -> list[dict[str, Any]]:
    return [json.loads(p.read_text()) for p in sorted(RAW.glob("*/*/row.json"))]


def summarize() -> None:
    rows = load_rows()
    judged = yaml.safe_load(JUDGMENTS.read_text()) or {} if JUDGMENTS.exists() else {}
    known = {}
    for row in rows:
        for mode in ("impact", "context"):
            for c in row["modes"].get(mode, {}).get("candidates", []):
                if c["node_id"] is not None:
                    known[f"{row['repo']}@{row['sha'][:8]}:{c['qn']}"] = c
    verdicts = {j["id"]: j for j in judged.get("judgments", [])}
    missing = sorted(set(verdicts) - set(known))
    for spot in judged.get("spot_checks", []):
        repo_sha, qn = spot["id"].split(":", 1)
        repo, sha8 = repo_sha.split("@")
        reply = json.loads((RAW / repo / sha8 / "impact.json").read_text())
        named = {n["qualified_name"]: n.get("id") for n in reply["changed_nodes"]}
        named |= {n["qualified_name"]: n.get("id") for n in reply["impacted_nodes"]}
        if named.get(qn) is None:
            missing.append(spot["id"])
    if missing:
        sys.exit(
            "judged ids that are not graph candidates at their commit:\n  " + "\n  ".join(missing)
        )
    errors = [
        f"{r['repo']}@{r['sha'][:8]} {m}: {mr.get('error') or 'rc=' + str(mr['rc'])}"
        for r in rows
        for m, mr in r["modes"].items()
        if mr.get("error") or mr["rc"] != 0
    ]
    errors += [
        f"{r['repo']}@{r['sha'][:8]}: graph not synced to C" for r in rows if not r["synced"]
    ]
    report: dict[str, Any] = {"errors": errors, "repos": {}}
    for repo in [*COMMITS, "overall"]:
        rs = [r for r in rows if repo in ("overall", r["repo"])]
        if not rs:
            continue
        out: dict[str, Any] = {"commits": len(rs), "median_chars": {}}
        for mode in MODES:
            chars = [r["modes"][mode]["chars"] for r in rs if mode in r["modes"]]
            out["median_chars"][mode] = statistics.median(chars) if chars else None
        for mode in ("impact", "context"):
            added: Counter[str] = Counter()
            act = noise = unjudged = 0
            with_candidates = set()
            for r in rs:
                mr = r["modes"].get(mode, {})
                added.update(mr.get("added_classes", {}))
                for c in mr.get("candidates", []):
                    with_candidates.add(r["sha"])
                    v = verdicts.get(f"{r['repo']}@{r['sha'][:8]}:{c['qn']}")
                    if v is None:
                        unjudged += 1
                    elif v["verdict"] == "actionable":
                        act += 1
                    else:
                        noise += 1
            out[mode] = {
                "added_by_class": dict(added),
                "commits_with_candidates": len(with_candidates),
                "unjudged_candidates": unjudged,
                "actionable": act,
                "judged_noise": noise,
                "mechanical_noise": sum(v for k, v in added.items() if k not in CANDIDATE),
                "actionable_per_commit": round(act / len(rs), 2),
            }
        measured = [r for r in rs if "radius_total" in r]
        for mode in ("impact", "context"):
            mrs = [r for r in measured if mode in r["modes"]]
            out[f"{mode}_kept"] = {
                "kept_impacted": sum(r["modes"][mode].get("kept_impacted", 0) for r in mrs),
                "radius_total": sum(r["radius_total"] for r in mrs),
                "kept_candidates": sum(r["modes"][mode].get("kept_candidates", 0) for r in mrs),
                "radius_candidates": sum(len(r["radius_candidates"]) for r in mrs),
            }
        out["changes_named_radius_candidates"] = sum(
            len(r["changes_named_radius_candidates"]) for r in measured
        )
        ctx = [r["modes"]["context"] for r in measured if "context" in r["modes"]]
        out["context_snippet_share"] = (
            round(sum(m["snippet_chars"] for m in ctx) / sum(m["chars"] for m in ctx), 3)
            if ctx
            else None
        )
        report["repos"][repo] = out
    (RAW / "summary.json").write_text(json.dumps(report, indent=1))
    print(json.dumps(report, indent=1))


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    sub = parser.add_subparsers(dest="cmd", required=True)
    run = sub.add_parser("run")
    run.add_argument("repos", nargs="*", default=list(COMMITS))
    sub.add_parser("summarize")
    args = parser.parse_args()
    RAW.mkdir(parents=True, exist_ok=True)
    if args.cmd == "run":
        for repo in args.repos:
            run_repo(repo)
    else:
        summarize()


if __name__ == "__main__":
    main()
