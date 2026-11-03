"""Test-reach keys from per_test_cover.py runs: the `Test` functions that executed a target.

    python reach_key.py TARGET COVER_JSON [COVER_JSON ...]

Prints the tests that executed the target in every run, and any that did in
only some (a run-to-run difference to read by hand). Fuzz functions are left
out: the questions ask about `Test` functions.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path


def reached(cover: dict, target: str) -> set[str]:
    return {
        t
        for t, fns in cover["tests"].items()
        if target in fns and t.split("::")[1].startswith("Test")
    }


def main() -> int:
    target, *files = sys.argv[1:]
    runs = [reached(json.loads(Path(f).read_text()), target) for f in files]
    stable = set.intersection(*runs)
    for t in sorted(stable):
        print(t)
    for i, r in enumerate(runs):
        for t in sorted(r - stable):
            print(f"only in {files[i]}: {t}", file=sys.stderr)
    print(f"{len(stable)} tests", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
