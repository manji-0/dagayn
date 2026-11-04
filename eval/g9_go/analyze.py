"""Registered analysis for G9 (PREREG.md): G7's analysis, split by question length.

    python analyze.py OUT_TESTIFY OUT_COMPRESS OUT_KONG [--seed 20261107] [--include-failed]

Each OUT is a `run_agent_tokens_eval.py --out` directory. Each question's
length (LONG or SHORT) is read from this directory's tasks files. The
measures, the effect, the permutation test, Holm's correction, the handling
of failed runs, and the descriptive numbers are G7's (`../g7_go/analyze.py`,
imported unchanged).

- LONG (primary): the effect and permutation test over the LONG questions,
  Holm over tokens, time, and F1.
- SHORT (guard): the token effect over the SHORT questions, a point
  estimate; its permutation p is reported and decides nothing.
- Secondary: the same per repository and per question (Holm over the
  twelve questions, per measure).
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import math
import random
from pathlib import Path

HERE = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("g7_analyze", HERE.parent / "g7_go" / "analyze.py")
g7 = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(g7)
MEASURES, describe, effect, family = g7.MEASURES, g7.describe, g7.effect, g7.family
graphs_rewritten, holm, load, permutation = g7.graphs_rewritten, g7.holm, g7.load, g7.permutation

SHORT_TOKEN_RATIO_MAX = 1.10


def lengths() -> dict[str, str]:
    out = {}
    for path in sorted(HERE.glob("tasks_*.json")):
        for tid, task in json.loads(path.read_text())["tasks"].items():
            out[tid] = task["length"]
    return out


def exact(rows: list[dict], tasks: list[str]) -> dict[str, int]:
    return {
        s: sum(int(r["exact"]) for r in rows if r["setup"] == s and r["task"] in tasks)
        for s in ("candidate", "baseline")
    }


def bar(long_family: dict, long_exact: dict, short_tokens: float, short_exact: dict) -> dict:
    """LONG: tokens or time at most 0.85x with that measure's Holm p (over
    tokens, time, and F1 on LONG) below 0.05, and the dagayn arm no more than
    2 exact matches behind on LONG. SHORT: the token ratio at most 1.10, and
    the dagayn arm no exact match behind on SHORT. Both must hold."""
    gain = {
        label: long_family[key]["effect"] <= 0.85 and long_family[key]["p_holm"] < 0.05
        for key, label in (("total_tokens", "tokens"), ("duration_s", "time"))
    }
    long_ok = (gain["tokens"] or gain["time"]) and (
        long_exact["candidate"] > long_exact["baseline"] - 3
    )
    short_ok = (
        short_tokens <= SHORT_TOKEN_RATIO_MAX
        and short_exact["candidate"] >= short_exact["baseline"]
    )
    return {
        "long": {
            "gain": gain,
            "exact": long_exact,
            "exact_not_3_or_more_fewer": long_exact["candidate"] > long_exact["baseline"] - 3,
            "met": long_ok,
        },
        "short": {
            "token_ratio": short_tokens,
            "token_ratio_at_most_1.10": short_tokens <= SHORT_TOKEN_RATIO_MAX,
            "exact": short_exact,
            "exact_not_fewer": short_exact["candidate"] >= short_exact["baseline"],
            "met": short_ok,
        },
        "met": long_ok and short_ok,
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("dirs", nargs="+", type=Path)
    ap.add_argument("--seed", type=int, default=20261107)
    ap.add_argument("--include-failed", action="store_true")
    a = ap.parse_args()
    rng = random.Random(a.seed)
    rows, failed = load(a.dirs, a.include_failed)
    length = lengths()
    tasks = sorted({r["task"] for r in rows})
    long_tasks = [t for t in tasks if length[t] == "LONG"]
    short_tasks = [t for t in tasks if length[t] == "SHORT"]
    long_rows = [r for r in rows if r["task"] in long_tasks]
    short_rows = [r for r in rows if r["task"] in short_tasks]
    long_family = family(long_rows, long_tasks, rng)
    short_tokens = round(math.exp(effect(short_rows, short_tasks, "total_tokens", True)), 4)
    _obs, short_p = permutation(short_rows, short_tasks, "total_tokens", True, rng)
    report = {
        "failed_or_excluded": [
            {k: r[k] for k in ("id", "setup", "task", "ok", "mcp_status")} for r in failed
        ],
        "lengths": {t: length[t] for t in tasks},
        "long": long_family,
        "short": {"total_tokens": {"effect": short_tokens, "p_report_only": round(short_p, 4)}},
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
    report["bar"] = bar(
        long_family, exact(rows, long_tasks), short_tokens, exact(rows, short_tasks)
    )
    report["graphs_rewritten_during_a_run"] = graphs_rewritten(a.dirs)
    print(json.dumps(report, indent=1, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
