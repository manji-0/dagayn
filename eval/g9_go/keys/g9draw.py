"""The seeded draw of G9's twelve targets from the committed pools.

    python g9draw.py POOLS_JSON

The seed is the first 8 hex digits of the SHA-256 of POOLS_JSON's bytes. For
each repository in ALLOCATION's order, and each stratum in the order
reach, name, short, it draws the stratum's number of targets. One draw picks
a kind uniformly among the stratum's kinds that still have a candidate, then
a target uniformly from that kind's candidates of the stratum, sorted by
name. A target the repository already has is not a candidate, and nor is a
kind 4 target once the repository has one (one change commit per
repository). For kind 4 it then draws 2 of the target's callers outside test
files: the change edits those two and the target.
"""

from __future__ import annotations

import hashlib
import json
import random
import sys
from pathlib import Path

# reach: kind 1 (LONG). name: kinds 2 to 4, LONG. short: kinds 2 to 4, SHORT.
STRATA = {
    "reach": ("LONG", ("kind1",)),
    "name": ("LONG", ("kind2", "kind3", "kind4")),
    "short": ("SHORT", ("kind2", "kind3", "kind4")),
}
ALLOCATION = {
    "testify": {"reach": 1, "name": 2, "short": 1},
    "compress": {"reach": 1, "name": 2, "short": 1},
    "kong": {"reach": 2, "name": 0, "short": 2},
}


def draw(path: Path) -> dict:
    data = path.read_bytes()
    digest = hashlib.sha256(data).hexdigest()
    rng = random.Random(int(digest[:8], 16))
    pools = json.loads(data)
    out: dict = {"pools_sha256": digest, "seed": int(digest[:8], 16), "picks": []}
    for repo, strata in ALLOCATION.items():
        taken: set[str] = set()
        for stratum, n in strata.items():
            length, kinds = STRATA[stratum]
            for _ in range(n):
                has_kind4 = any(p["repository"] == repo and p["kind"] == 4 for p in out["picks"])
                open_ = {
                    k: sorted(
                        (
                            e
                            for e in pools[repo][k]
                            if e["length"] == length and e["target"] not in taken
                        ),
                        key=lambda e: e["target"],
                    )
                    for k in kinds
                    if not (k == "kind4" and has_kind4)
                }
                kind = rng.choice([k for k, c in open_.items() if c])
                entry = rng.choice(open_[kind])
                taken.add(entry["target"])
                pick = {"repository": repo, "stratum": stratum, "kind": int(kind[-1]), **entry}
                if kind == "kind4":
                    pick["edited_callers"] = sorted(
                        rng.sample(sorted(entry["non_test_callers"]), 2)
                    )
                out["picks"].append(pick)
    return out


if __name__ == "__main__":
    print(json.dumps(draw(Path(sys.argv[1])), indent=1, sort_keys=True))
