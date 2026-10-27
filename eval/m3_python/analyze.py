"""Registered analysis for M3 (PREREG.md).

    python analyze.py OUT_HTTPX OUT_DAGAYN [--seed 20261027] [--include-failed]

Each OUT is a `run_agent_tokens_eval.py --out` directory (results.csv, raw/).
The candidate setup is dagayn, the baseline no MCP server.

Primary: over the eight questions, the mean over questions of the difference
of mean log tokens (and log time) and of mean F1 between the setups, with a
permutation test that shuffles setup labels within each question (10,000
draws), and Holm's correction over the three p-values. Secondary: the same per
repository and per question (Holm over the eight questions, per measure).
"""

from __future__ import annotations

import argparse
import csv
import json
import math
import random
import statistics as st
from pathlib import Path

MEASURES = (("total_tokens", True), ("duration_s", True), ("f1", False))


def load(dirs: list[Path], include_failed: bool) -> tuple[list[dict], list[dict]]:
    """Runs that finished, and the rest. A dagayn run whose MCP server did not
    connect is a failed run. With *include_failed* (the sensitivity analysis),
    a failed run counts with F1 0 and the largest tokens and time of its
    question and setup."""
    rows, failed = [], []
    for d in dirs:
        for r in csv.DictReader(open(d / "results.csv")):
            r["_dir"] = str(d)
            ok = r["ok"] == "True"
            if r["setup"] == "candidate" and "dagayn:connected" not in r.get("mcp_status", ""):
                ok = False
            (rows if ok else failed).append(r)
    if include_failed:
        for r in failed:
            same = [x for x in rows if x["task"] == r["task"] and x["setup"] == r["setup"]]
            for key in ("total_tokens", "duration_s"):
                r[key] = str(max(float(x[key]) for x in same))
            r["f1"], r["exact"] = "0", "0"
        rows = rows + failed
    return rows, failed


def effect(rows: list[dict], tasks: list[str], key: str, log: bool) -> float:
    diffs = []
    for t in tasks:
        a = [float(r[key]) for r in rows if r["task"] == t and r["setup"] == "candidate"]
        b = [float(r[key]) for r in rows if r["task"] == t and r["setup"] == "baseline"]
        if log:
            a, b = [math.log(max(x, 1.0)) for x in a], [math.log(max(x, 1.0)) for x in b]
        diffs.append(st.mean(a) - st.mean(b))
    return st.mean(diffs)


def permutation(
    rows: list[dict], tasks: list[str], key: str, log: bool, rng: random.Random, n: int = 10000
):
    observed = effect(rows, tasks, key, log)
    by_task = {t: [r for r in rows if r["task"] == t] for t in tasks}
    hits = 0
    for _ in range(n):
        shuffled = []
        for t in tasks:
            labels = [r["setup"] for r in by_task[t]]
            rng.shuffle(labels)
            shuffled += [{"task": t, "setup": s, key: r[key]} for r, s in zip(by_task[t], labels)]
        if abs(effect(shuffled, tasks, key, log)) >= abs(observed) - 1e-12:
            hits += 1
    return observed, (hits + 1) / (n + 1)


def holm(pvalues: dict[str, float]) -> dict[str, float]:
    out, running = {}, 0.0
    for i, k in enumerate(sorted(pvalues, key=pvalues.get)):
        running = max(running, min(1.0, pvalues[k] * (len(pvalues) - i)))
        out[k] = running
    return out


def family(rows: list[dict], tasks: list[str], rng: random.Random) -> dict:
    res = {}
    for key, log in MEASURES:
        obs, p = permutation(rows, tasks, key, log, rng)
        res[key] = {"effect": round(math.exp(obs), 4) if log else round(obs, 4), "p": round(p, 4)}
    for k, p in holm({k: v["p"] for k, v in res.items()}).items():
        res[k]["p_holm"] = round(p, 4)
    return res


def checks_after_dagayn(raw: Path) -> int:
    seen, n = False, 0
    for line in open(raw):
        e = json.loads(line)
        if e.get("type") != "assistant":
            continue
        for b in e["message"]["content"]:
            if b.get("type") != "tool_use":
                continue
            if b["name"].startswith("mcp__"):
                seen = True
            elif seen and b["name"] in ("Bash", "Grep", "Read", "Glob"):
                n += 1
    return n


def describe(rows: list[dict], tasks: list[str]) -> dict:
    out = {}
    for setup in ("candidate", "baseline"):
        mine = [r for r in rows if r["setup"] == setup]
        out[setup] = {
            "runs": len(mine),
            "exact": sum(int(r["exact"]) for r in mine),
            "mean_f1": round(st.mean(float(r["f1"]) for r in mine), 4) if mine else None,
            "cost_usd": round(sum(float(r["cost_usd"]) for r in mine), 2),
            "per_task": {},
        }
        for t in tasks:
            tr = [r for r in mine if r["task"] == t]
            if not tr:
                continue
            out[setup]["per_task"][t] = {
                "runs": len(tr),
                "exact": sum(int(r["exact"]) for r in tr),
                "mean_f1": round(st.mean(float(r["f1"]) for r in tr), 4),
                "median_tokens": st.median(int(r["total_tokens"]) for r in tr),
                "median_duration_s": st.median(float(r["duration_s"]) for r in tr),
                "median_turns": st.median(int(r["turns"]) for r in tr),
                "median_dagayn_calls": st.median(int(r["dagayn_calls"]) for r in tr),
                "median_checks_after_dagayn": st.median(
                    checks_after_dagayn(Path(r["_dir"]) / "raw" / f"{r['id']}.jsonl") for r in tr
                ),
            }
    return out


def bar(report: dict) -> dict:
    """M3's bar: pooled tokens at most 0.85x with Holm p < 0.05, and the dagayn
    arm no more than 2 exact matches behind (3 or more fewer fails, as in E12
    and E13)."""
    tokens = report["pooled"]["total_tokens"]
    exact = {s: report["descriptive"][s]["exact"] for s in ("candidate", "baseline")}
    checks = {
        "tokens_at_most_0.85x": tokens["effect"] <= 0.85,
        "tokens_holm_p_below_0.05": tokens["p_holm"] < 0.05,
        "exact_not_3_or_more_fewer": exact["candidate"] > exact["baseline"] - 3,
    }
    return {**checks, "exact": exact, "met": all(checks.values())}


def graphs_rewritten(dirs: list[Path]) -> list[str]:
    """dagayn runs whose graph metadata changed between the build and the end
    of the run (another dagayn rewrote the measured graph)."""
    return [
        meta["id"]
        for d in dirs
        for path in sorted((d / "raw").glob("*.meta.json"))
        if (meta := json.loads(path.read_text()))["setup"] == "candidate"
        and meta.get("graph_before") != meta.get("graph_after")
    ]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("dirs", nargs="+", type=Path)
    ap.add_argument("--seed", type=int, default=20261027)
    ap.add_argument("--include-failed", action="store_true")
    a = ap.parse_args()
    rng = random.Random(a.seed)
    rows, failed = load(a.dirs, a.include_failed)
    tasks = sorted({r["task"] for r in rows})
    report = {
        "failed_or_excluded": [
            {k: r[k] for k in ("id", "setup", "task", "ok", "mcp_status")} for r in failed
        ],
        "pooled": family(rows, tasks, rng),
        "per_repository": {},
        "per_task": {},
        "descriptive": describe(rows, tasks),
    }
    for d in a.dirs:
        mine = [r for r in rows if r["_dir"] == str(d)]
        report["per_repository"][d.name] = family(mine, sorted({r["task"] for r in mine}), rng)
    per_task = {key: {} for key, _ in MEASURES}
    for t in tasks:
        mine = [r for r in rows if r["task"] == t]
        for key, log in MEASURES:
            obs, p = permutation(mine, [t], key, log, rng)
            per_task[key][t] = {
                "effect": round(math.exp(obs), 4) if log else round(obs, 4),
                "p": round(p, 4),
            }
    for key in per_task:
        for t, p in holm({t: v["p"] for t, v in per_task[key].items()}).items():
            per_task[key][t]["p_holm"] = round(p, 4)
    report["per_task"] = per_task
    report["bar"] = bar(report)
    report["graphs_rewritten_during_a_run"] = graphs_rewritten(a.dirs)
    print(json.dumps(report, indent=1, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
