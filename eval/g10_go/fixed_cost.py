"""The SHORT fixed cost of the dagayn arm, a registered report-only measure (#292).

    python fixed_cost.py OUT_TESTIFY OUT_COMPRESS OUT_KONG OUT_CHI

`analyze.py` calls `fixed_cost_report`; run alone it prints the same report.
It reads each run's raw transcript (`OUT/raw/ID.jsonl`, not committed).

A request is one model call: one distinct assistant message id, in transcript
order, with the tokens of its usage (input, output, cache read, and cache
write). A dagayn call is a tool call named `mcp__dagayn__*`, or a Bash call
whose command runs `dagayn`. For each candidate run of a SHORT question:

- `fixed`: the tokens of every request up to and including the first one that
  makes a dagayn call. A run with no dagayn call has none and is counted.
- `tool_search`: the part of `fixed` in requests that call `ToolSearch`.
- `first_call`: the tokens of the request that makes the first dagayn call.
- `pattern`: request 1 calls `ToolSearch` and request 2 makes the dagayn call.

The report gives the medians per SHORT question and pooled, the counts, and
the SHORT token ratio with each candidate run's `fixed` taken off its tokens
(G7's effect: the mean over questions of the difference of mean log tokens).
It decides nothing.
"""

from __future__ import annotations

import json
import math
import re
import statistics as st
import sys
from pathlib import Path

USAGE = ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")
CLI_CALL = re.compile(r"(?:^|[\s;&|(/])dagayn\s")


def is_dagayn_call(block: dict) -> bool:
    if block.get("type") != "tool_use":
        return False
    name = block.get("name", "")
    if name.startswith("mcp__dagayn__"):
        return True
    command = (block.get("input") or {}).get("command") or ""
    return name == "Bash" and bool(CLI_CALL.search(command))


def requests(path: Path) -> list[tuple[int, list[dict]]]:
    """(tokens, tool calls) per request, in order."""
    usage: dict[str, dict] = {}
    calls: dict[str, list[dict]] = {}
    order: list[str] = []
    for line in path.read_text().splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("type") != "assistant":
            continue
        message = event.get("message", {})
        mid = message.get("id")
        if mid is None:
            continue
        if mid not in usage:
            order.append(mid)
            usage[mid], calls[mid] = {}, []
        usage[mid] = message.get("usage") or usage[mid]
        calls[mid] += [b for b in message.get("content", []) if b.get("type") == "tool_use"]
    return [(sum(usage[m].get(k, 0) or 0 for k in USAGE), calls[m]) for m in order]


def run_fixed_cost(path: Path) -> dict | None:
    reqs = requests(path)
    for i, (_tokens, calls) in enumerate(reqs):
        if any(is_dagayn_call(c) for c in calls):
            upto = reqs[: i + 1]
            return {
                "fixed": sum(t for t, _ in upto),
                "tool_search": sum(
                    t for t, cs in upto if any(c.get("name") == "ToolSearch" for c in cs)
                ),
                "first_call": reqs[i][0],
                "requests_before_and_with_call": i + 1,
                "pattern": i == 1 and any(c.get("name") == "ToolSearch" for c in reqs[0][1]),
            }
    return None


def _median(xs: list[float]) -> float | None:
    return st.median(xs) if xs else None


def fixed_cost_report(rows: list[dict], short_tasks: list[str]) -> dict:
    per_task: dict = {}
    diffs = []
    every: list[dict] = []
    for t in short_tasks:
        cand = [r for r in rows if r["task"] == t and r["setup"] == "candidate"]
        base = [
            float(r["total_tokens"]) for r in rows if r["task"] == t and r["setup"] == "baseline"
        ]
        found = []
        for r in cand:
            fc = run_fixed_cost(Path(r["_dir"]) / "raw" / f"{r['id']}.jsonl")
            found.append((r, fc))
        with_call = [fc for _r, fc in found if fc]
        per_task[t] = {
            "runs": len(cand),
            "runs_with_a_dagayn_call": len(with_call),
            "runs_tool_search_then_call": sum(fc["pattern"] for fc in with_call),
            "median_fixed": _median([fc["fixed"] for fc in with_call]),
            "median_tool_search": _median([fc["tool_search"] for fc in with_call]),
            "median_first_call": _median([fc["first_call"] for fc in with_call]),
        }
        rest = [max(float(r["total_tokens"]) - (fc["fixed"] if fc else 0), 1.0) for r, fc in found]
        if rest and base:
            diffs.append(
                st.mean(map(math.log, rest)) - st.mean(math.log(max(x, 1.0)) for x in base)
            )
        every += with_call
    return {
        "per_task": per_task,
        "pooled": {
            "runs": sum(p["runs"] for p in per_task.values()),
            "runs_with_a_dagayn_call": len(every),
            "runs_tool_search_then_call": sum(fc["pattern"] for fc in every),
            "median_fixed": _median([fc["fixed"] for fc in every]),
            "median_tool_search": _median([fc["tool_search"] for fc in every]),
            "median_first_call": _median([fc["first_call"] for fc in every]),
        },
        "short_token_ratio_without_fixed": round(math.exp(st.mean(diffs)), 4) if diffs else None,
    }


if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from analyze import lengths, load

    dirs = [Path(d) for d in sys.argv[1:]]
    rows, _failed = load(dirs, False)
    info = lengths()
    short = sorted({r["task"] for r in rows if info[r["task"]][0] == "SHORT"})
    print(json.dumps(fixed_cost_report(rows, short), indent=1))
