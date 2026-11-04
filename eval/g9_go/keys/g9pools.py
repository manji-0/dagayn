"""Candidate pools for G9, tagged LONG or SHORT, from scip-go, gofuncs, and Go coverage.

    python g9pools.py INPUTS testify=TARGET compress=TARGET kong=TARGET > pools.json

INPUTS holds build_inputs.sh's files (`../../g8_go/keys/build_inputs.sh`) for
every repository; each TARGET is the tag's archive they were built on. The
four kinds are G8's (`../../g8_go/keys/pools.py`, unchanged), less G7's and
G8's targets. Each candidate gets `name_calls`, the lines a grep for its name
followed by `(` finds, and a length:

- LONG: kind 1 (which tests execute a function), or a kind 2 to 4 candidate
  whose `name_calls` is at least NAME_CALLS_LONG;
- SHORT: a kind 2 to 4 candidate below it.

PREREG.md states the rule and the G7 and G8 numbers it rests on
(`length_basis.py`). Nothing here reads dagayn.
"""

from __future__ import annotations

import json
import re
import sys
from functools import cache
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent.parent / "g8_go" / "keys"))
from pools import bare, pools, skipped  # noqa: E402

NAME_CALLS_LONG = 200
G8_TARGETS = {
    "mock/mock.go::diffArguments",
    "assert/assertion_forward.go::Assertions.JSONEq",
    "suite/suite.go::Suite.SetT",
    "assert/assertions.go::YAMLEq",
    "zip/reader.go::File.OpenRaw",
    "gzip/gzip.go::Writer.init",
    "gzhttp/writer/gzkp/gzkp.go::NewWriter",
    "zstd/blockenc.go::blockHeader.setSize",
}
REPOS = ("testify", "compress", "kong")


def go_files(target: Path) -> list[Path]:
    """The .go files `go build ./...` and `go test ./...` see."""
    return sorted(
        p
        for p in target.rglob("*.go")
        if not skipped(p.relative_to(target).as_posix()) and ".git" not in p.parts
    )


def name_counter(target: Path):
    texts = [p.read_text(errors="replace") for p in go_files(target)]

    @cache
    def count(name: str) -> int:
        """Lines with the word *name* followed by `(`: what `grep -E '\\bname\\('` finds."""
        word = re.compile(rf"\b{re.escape(name)}\(")
        return sum(bool(word.search(line)) for t in texts for line in t.splitlines())

    return count


def tagged(inputs: Path, target: Path, name: str) -> dict:
    found = pools(inputs, target, name)
    count = name_counter(target)
    out: dict = {"repository": name}
    for kind in ("kind1", "kind2", "kind3", "kind4"):
        entries = []
        for e in found[kind]:
            if e["target"] in G8_TARGETS:
                continue
            calls = count(bare(e["target"]))
            long = kind == "kind1" or calls >= NAME_CALLS_LONG
            entries.append({**e, "name_calls": calls, "length": "LONG" if long else "SHORT"})
        out[kind] = entries
    return out


if __name__ == "__main__":
    inputs = Path(sys.argv[1])
    targets = dict(arg.split("=", 1) for arg in sys.argv[2:])
    assert tuple(targets) == REPOS, f"pass {REPOS} in that order"
    out = {name: tagged(inputs, Path(t), name) for name, t in targets.items()}
    print(json.dumps(out, indent=1, sort_keys=True))
