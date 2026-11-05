"""The seeded draw of G10's twelve targets from the committed pools.

    python g10draw.py POOLS_JSON

The draw is G9's (`../../g9_go/keys/g9draw.py`, imported unchanged): the seed
is the first 8 hex digits of the SHA-256 of POOLS_JSON's bytes; for each
repository in ALLOCATION's order and each stratum in the order reach, name,
short, one draw picks a kind uniformly among the stratum's kinds that still
have a candidate, then a target uniformly from that kind's candidates of the
stratum, sorted by name; no target twice per repository, at most one kind 4
target per repository, and 2 edited callers for a kind 4 target. Only the
allocation is G10's.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent.parent / "g9_go" / "keys"))
import g9draw  # noqa: E402

# chi is the repository no fix since G7 was tuned on, so it carries half the
# questions (PREREG.md, "Draw"). Its name stratum has one candidate.
ALLOCATION = {
    "testify": {"reach": 1, "name": 1, "short": 0},
    "compress": {"reach": 1, "name": 1, "short": 0},
    "kong": {"reach": 1, "name": 0, "short": 1},
    "chi": {"reach": 2, "name": 1, "short": 3},
}


def draw(path: Path) -> dict:
    g9draw.ALLOCATION = ALLOCATION
    return g9draw.draw(path)


if __name__ == "__main__":
    print(json.dumps(draw(Path(sys.argv[1])), indent=1, sort_keys=True))
