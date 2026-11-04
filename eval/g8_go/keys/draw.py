"""The seeded draw of G8's eight targets from the committed pools.

    python draw.py POOLS_JSON

The seed is the first 8 hex digits of the SHA-256 of POOLS_JSON's bytes, so
nobody picks it after seeing the pools. For each repository (testify, then
compress) and each kind (1 to 4), the draw takes one target uniformly from
the pool, sorted by name, and draws again if the repository already has it.
For kind 4 it then draws 2 of the target's callers outside test files: the
change edits those two and the target.
"""

from __future__ import annotations

import hashlib
import json
import random
import sys
from pathlib import Path


def draw(path: Path) -> dict:
    data = path.read_bytes()
    seed = int(hashlib.sha256(data).hexdigest()[:8], 16)
    rng = random.Random(seed)
    pools = json.loads(data)
    out: dict = {"pools_sha256": hashlib.sha256(data).hexdigest(), "seed": seed, "picks": {}}
    for name in ("testify", "compress"):
        taken: set[str] = set()
        for kind in ("kind1", "kind2", "kind3", "kind4"):
            pool = sorted(pools[name][kind], key=lambda e: e["target"])
            while True:
                entry = rng.choice(pool)
                if entry["target"] not in taken:
                    break
            taken.add(entry["target"])
            pick = dict(entry)
            if kind == "kind4":
                pick["edited_callers"] = sorted(rng.sample(sorted(entry["non_test_callers"]), 2))
            out["picks"][f"{name}/{kind}"] = pick
    return out


if __name__ == "__main__":
    print(json.dumps(draw(Path(sys.argv[1])), indent=1, sort_keys=True))
