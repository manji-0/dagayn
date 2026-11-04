"""The G7 and G8 numbers G9's LONG / SHORT rule rests on.

    python length_basis.py TESTIFY_ARCHIVE COMPRESS_ARCHIVE

Each ARCHIVE is the tag's `git archive` (the harness's target without a
change commit). For each of G7's and G8's sixteen questions the table gives
the median number of turns of the baseline (no dagayn) in every registered
run of it: G7's runs (`../../g7_go/results/`), and G8's primary runs and its
rerun of G7's questions (`../../g8_go/results/`). It adds the question's
`name_calls` and the length `g9pools.py` gives it. Only baseline runs are read.
"""

from __future__ import annotations

import csv
import statistics as st
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
EVAL = HERE.parent.parent
sys.path.insert(0, str(EVAL / "g8_go" / "keys"))
sys.path.insert(0, str(HERE))
from dagayn_answers import PLAN  # noqa: E402
from g9pools import NAME_CALLS_LONG, bare, name_counter  # noqa: E402

RUNS = {
    "g7": [("G7", EVAL / "g7_go/results/out-testify"), ("G7", EVAL / "g7_go/results/out-compress")],
    "g8": [
        ("G8", EVAL / "g8_go/results/out-g8-testify"),
        ("G8", EVAL / "g8_go/results/out-g8-compress"),
    ],
}
RERUN = [
    ("G8 rerun", EVAL / "g8_go/results/out-g7-testify"),
    ("G8 rerun", EVAL / "g8_go/results/out-g7-compress"),
]
# A G7 or G8 task id's digit gives its kind: T1 and T5 are kind 1, T2 and T6 kind 2, and so on.
KIND_OF_ID = {"1": 1, "5": 1, "2": 2, "6": 2, "3": 3, "7": 3, "4": 4, "8": 4}


def baseline_turns(runs: list[tuple[str, Path]], task: str) -> dict[str, float]:
    out = {}
    for label, d in runs:
        rows = [
            r
            for r in csv.DictReader(open(d / "results.csv"))
            if r["task"] == task and r["setup"] == "baseline"
        ]
        if rows:
            out[label] = st.median(float(r["turns"]) for r in rows)
    return out


def main() -> int:
    counters = {
        "testify": name_counter(Path(sys.argv[1])),
        "compress": name_counter(Path(sys.argv[2])),
    }
    print("| Q | Kind | Target | Baseline turns (median of 10) | name_calls | Length |")
    print("|---|---|---|---|---|---|")
    by_length: dict[str, list[float]] = {"LONG": [], "SHORT": []}
    for exp, plan in PLAN.items():
        for task, (repo, target, _kind) in plan.items():
            kind = KIND_OF_ID[task[1:]]
            runs = RUNS[exp] + (RERUN if exp == "g7" else [])
            turns = baseline_turns(runs, task)
            calls = counters[repo](bare(target))
            length = "LONG" if kind == 1 or calls >= NAME_CALLS_LONG else "SHORT"
            by_length[length] += turns.values()
            shown = ", ".join(f"{v:g} ({k})" for k, v in turns.items())
            print(f"| {task} | {kind} | `{target}` | {shown} | {calls} | {length} |")
    print()
    for length, values in by_length.items():
        print(
            f"{length}: {len(values)} medians, median {st.median(values):g}, "
            f"min {min(values):g}, max {max(values):g}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
