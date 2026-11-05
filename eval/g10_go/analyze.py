"""Registered analysis for G10 (PREREG.md): G9's analysis with the 1.25 SHORT guard.

    python analyze.py OUT_TESTIFY OUT_COMPRESS OUT_KONG OUT_CHI [--seed 20261111] [--include-failed]

Each OUT is a `run_agent_tokens_eval.py --out` directory (results.csv, raw/).
Each question's length (LONG or SHORT) is read from this directory's tasks
files. The effect, the permutation test, Holm's correction, and the
descriptive numbers are G7's (`../g7_go/analyze.py`, imported unchanged).

- LONG (primary): the effect and permutation test over the LONG questions,
  Holm over tokens, time, and F1.
- SHORT (guard): the token effect over the SHORT questions, a point
  estimate; its permutation p is reported and decides nothing.
- Secondary: the same family per repository, for chi's LONG and SHORT
  questions on their own (the untuned repository), and per question (Holm
  over the twelve questions, per measure).
- Report only: the SHORT fixed cost (`fixed_cost.py`), from the raw
  transcripts.

Nothing here depends on how the candidate arm reaches dagayn (MCP or CLI):
a dagayn call is either an `mcp__dagayn__*` tool call or a Bash command that
runs `dagayn` (`fixed_cost.is_dagayn_call`), and a candidate run fails on its
MCP status only when its init event lists an MCP server.
"""

from __future__ import annotations

import argparse
import csv
import importlib.util
import json
import math
import random
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
_spec = importlib.util.spec_from_file_location("g7_analyze", HERE.parent / "g7_go" / "analyze.py")
g7 = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(g7)
MEASURES, describe, effect, family = g7.MEASURES, g7.describe, g7.effect, g7.family
graphs_rewritten, holm, permutation = g7.graphs_rewritten, g7.holm, g7.permutation
from fixed_cost import fixed_cost_report  # noqa: E402

SHORT_TOKEN_RATIO_MAX = 1.25
NEW_REPOSITORY = "chi"


def lengths() -> dict[str, tuple[str, str]]:
    """Task id -> (length, repository), from this directory's tasks files."""
    out = {}
    for path in sorted(HERE.glob("tasks_*.json")):
        repo = path.stem.removeprefix("tasks_")
        for tid, task in json.loads(path.read_text())["tasks"].items():
            out[tid] = (task["length"], repo)
    return out


def load(dirs: list[Path], include_failed: bool) -> tuple[list[dict], list[dict]]:
    """G7's `load`, except for the MCP check: a run is failed when it has no
    result, ended with an error, or timed out (`ok`), or when its init event
    lists an MCP server and `dagayn:connected` is not among them. With
    *include_failed*, a failed run counts with F1 0 and the largest tokens and
    time of its question and setup, as in G7."""
    rows, failed = [], []
    for d in dirs:
        for r in csv.DictReader(open(d / "results.csv")):
            r["_dir"] = str(d)
            ok = r["ok"] == "True"
            status = r.get("mcp_status", "")
            if r["setup"] == "candidate" and status and "dagayn:connected" not in status:
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


def exact(rows: list[dict], tasks: list[str]) -> dict[str, int]:
    return {
        s: sum(int(r["exact"]) for r in rows if r["setup"] == s and r["task"] in tasks)
        for s in ("candidate", "baseline")
    }


def bar(long_family: dict, long_exact: dict, short_tokens: float, short_exact: dict) -> dict:
    """LONG: tokens or time at most 0.85x with that measure's Holm p (over
    tokens, time, and F1 on LONG) below 0.05, and the dagayn arm no more than
    2 exact matches behind on LONG. SHORT: the token ratio at most 1.25, and
    the dagayn arm no exact match behind on SHORT. Both must hold."""
    gain = {
        label: long_family[key]["effect"] <= 0.85 and long_family[key]["p_holm"] < 0.05
        for key, label in (("total_tokens", "tokens"), ("duration_s", "time"))
    }
    long_exact_ok = long_exact["candidate"] > long_exact["baseline"] - 3
    long_ok = (gain["tokens"] or gain["time"]) and long_exact_ok
    short_ratio_ok = short_tokens <= SHORT_TOKEN_RATIO_MAX
    short_exact_ok = short_exact["candidate"] >= short_exact["baseline"]
    return {
        "long": {
            "gain": gain,
            "exact": long_exact,
            "exact_not_3_or_more_fewer": long_exact_ok,
            "met": long_ok,
        },
        "short": {
            "token_ratio": short_tokens,
            "token_ratio_at_most_1.25": short_ratio_ok,
            "exact": short_exact,
            "exact_not_fewer": short_exact_ok,
            "met": short_ratio_ok and short_exact_ok,
        },
        "met": long_ok and short_ratio_ok and short_exact_ok,
    }


def short_guard(rows: list[dict], tasks: list[str], rng: random.Random) -> dict:
    ratio = round(math.exp(effect(rows, tasks, "total_tokens", True)), 4)
    _obs, p = permutation(rows, tasks, "total_tokens", True, rng)
    return {"total_tokens": {"effect": ratio, "p_report_only": round(p, 4)}}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("dirs", nargs="+", type=Path)
    ap.add_argument("--seed", type=int, default=20261111)
    ap.add_argument("--include-failed", action="store_true")
    a = ap.parse_args()
    rng = random.Random(a.seed)
    rows, failed = load(a.dirs, a.include_failed)
    info = lengths()
    tasks = sorted({r["task"] for r in rows})
    long_tasks = [t for t in tasks if info[t][0] == "LONG"]
    short_tasks = [t for t in tasks if info[t][0] == "SHORT"]
    long_rows = [r for r in rows if r["task"] in long_tasks]
    short_rows = [r for r in rows if r["task"] in short_tasks]
    long_family = family(long_rows, long_tasks, rng)
    short = short_guard(short_rows, short_tasks, rng)
    report = {
        "failed_or_excluded": [
            {k: r[k] for k in ("id", "setup", "task", "ok", "mcp_status")} for r in failed
        ],
        "lengths": {t: info[t][0] for t in tasks},
        "long": long_family,
        "short": short,
        "per_repository": {},
        "new_repository": {},
        "per_task": {},
        "descriptive": describe(rows, tasks),
    }
    for d in a.dirs:
        mine = [r for r in rows if r["_dir"] == str(d)]
        report["per_repository"][d.name] = family(mine, sorted({r["task"] for r in mine}), rng)
    new_long = [t for t in long_tasks if info[t][1] == NEW_REPOSITORY]
    new_short = [t for t in short_tasks if info[t][1] == NEW_REPOSITORY]
    report["new_repository"] = {
        "repository": NEW_REPOSITORY,
        "long": family([r for r in rows if r["task"] in new_long], new_long, rng),
        "long_exact": exact(rows, new_long),
        "short": short_guard([r for r in rows if r["task"] in new_short], new_short, rng),
        "short_exact": exact(rows, new_short),
    }
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
    report["bar"] = bar(
        long_family,
        exact(rows, long_tasks),
        short["total_tokens"]["effect"],
        exact(rows, short_tasks),
    )
    report["short_fixed_cost_report_only"] = fixed_cost_report(rows, short_tasks)
    report["graphs_rewritten_during_a_run"] = graphs_rewritten(a.dirs)
    print(json.dumps(report, indent=1, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
