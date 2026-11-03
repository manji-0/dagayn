"""Recompute the machine-derived part of every G7 key and compare it with the tasks files.

    python check_keys.py WORK

WORK holds, for each repository NAME (testify, compress): the target
checkout `NAME/` (the harness's target: archive plus the change commit),
`NAME.occ.tsv` (scipdump of `scip-go` on it), `NAME.funcs.tsv` (gofuncs),
and `NAME.cover*.json` (per_test_cover.py runs). None of it is committed.

Caller keys are the functions holding a scip-go reference to the target
symbol, with the hand classification below applied (PREREG.md gives the
reasons); change keys then drop the functions the last commit edited.
Test-reach keys are the `Test` functions that executed the target in every
coverage run.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from edited_functions import edited  # noqa: E402
from reach_key import reached  # noqa: E402
from scip_refs import refs  # noqa: E402

TESTIFY = "github.com/stretchr/testify"
COMPRESS = "github.com/klauspost/compress"

# kind, repository, symbols (scip-go suffixes) or the reached function,
# sites that reference the symbol without calling it (by hand), change.
PLAN = {
    "T1": ("reach", "testify", "assert/assertions.go::CollectT.Errorf"),
    "T2": ("callers", "testify", [f"`{TESTIFY}/assert`/Assertions#Equal()."], set(), False),
    # `suite.T()` in Run is on a TestingSuite; every suite in the repository
    # embeds Suite and none declares its own T, so it can run (*Suite).T.
    "T3": (
        "callers",
        "testify",
        [f"`{TESTIFY}/suite`/Suite#T().", f"`{TESTIFY}/suite`/TestingSuite#T."],
        set(),
        False,
    ),
    "T4": ("callers", "testify", [f"`{TESTIFY}/assert`/ObjectsAreEqual()."], set(), True),
    "C1": ("reach", "compress", "flate/stateless.go::statelessEnc"),
    "C2": (
        "callers",
        "compress",
        [f"`{COMPRESS}/zstd/internal/xxhash`/Digest#Reset()."],
        set(),
        False,
    ),
    # `d.step(d)` calls the field init sets to (*compressor).storeFast for
    # levels 1 to 6, the default level, and custom window sizes; init and
    # TestWriterReset only assign the field.
    "C3": (
        "callers",
        "compress",
        [f"`{COMPRESS}/flate`/compressor#step."],
        {"flate/deflate.go::compressor.init", "flate/deflate_test.go::TestWriterReset"},
        False,
    ),
    "C4": ("callers", "compress", [f"`{COMPRESS}/flate`/Writer#Reset()."], set(), True),
}


def key(work: Path, task: str) -> set[str]:
    kind, name, *rest = PLAN[task]
    if kind == "reach":
        runs = [json.loads(p.read_text()) for p in sorted(work.glob(f"{name}.cover*.json"))]
        return set.intersection(*(reached(c, rest[0]) for c in runs))
    symbols, not_calls, change = rest
    found = {
        fn
        for fn, _site, _sym in refs(work / f"{name}.occ.tsv", work / f"{name}.funcs.tsv", symbols)
    }
    found -= not_calls
    if change:
        found -= edited(work / name, work / f"{name}.funcs.tsv")
    return found


def main() -> int:
    work = Path(sys.argv[1])
    truth = {}
    for name in ("testify", "compress"):
        spec = json.loads((HERE.parent / f"tasks_{name}.json").read_text())
        truth.update({t: set(s["truth"]) for t, s in spec["tasks"].items()})
    bad = 0
    for task in PLAN:
        got = key(work, task)
        same = got == truth[task]
        bad += not same
        print(f"{task}: {'agrees' if same else 'DIFFERS'} ({len(truth[task])} entries)")
        for q in sorted(got - truth[task]):
            print(f"   derived, not in key: {q}")
        for q in sorted(truth[task] - got):
            print(f"   in key, not derived: {q}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
