"""What dagayn answers to each G9 question, scored against the keys.

    DAGAYN=/path/to/dagayn python g9answers.py testify=TARGET compress=TARGET kong=TARGET

Each TARGET is a harness target with a graph from `dagayn build
--local-embedding none`, as the harness builds it. The calls are G8's
(`../../g8_go/keys/dagayn_answers.py`, imported unchanged): `tests_for` and
`callers_of(depth=6)` for test reach, `callers_of` for callers, at the
default detail level. The report also records each graph's extractor
versions.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent.parent / "g8_go" / "keys"))
from dagayn_answers import answer, extractor_versions  # noqa: E402

KIND = {1: "reach", 2: "callers", 3: "callers"}


def main() -> int:
    repos = {n: Path(p) for n, p in (a.split("=", 1) for a in sys.argv[1:])}
    report: dict = {"graphs": {n: extractor_versions(p) for n, p in repos.items()}}
    picks = json.loads((HERE.parent / "draw.json").read_text())["picks"]
    for name, repo in repos.items():
        tasks = json.loads((HERE.parent / f"tasks_{name}.json").read_text())["tasks"]
        # g9keys.py numbers a repository's tasks in the draw's order.
        mine = [p for p in picks if p["repository"] == name]
        for (tid, task), pick in zip(tasks.items(), mine, strict=True):
            kind = KIND[pick["kind"]]
            report[tid] = answer(repo, pick["target"], kind, set(task["truth"]))
    print(json.dumps(report, indent=1, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
