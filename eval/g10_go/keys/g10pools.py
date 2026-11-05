"""Candidate pools for G10, tagged LONG or SHORT, from scip-go, gofuncs, and Go coverage.

    python g10pools.py INPUTS testify=TARGET compress=TARGET kong=TARGET chi=TARGET > pools.json

INPUTS holds build_inputs.sh's files (`../../g8_go/keys/build_inputs.sh`) for
every repository; each TARGET is the tag's archive they were built on. The
kinds, `name_calls`, and the LONG / SHORT rule are G9's
(`../../g9_go/keys/g9pools.py`, imported unchanged), which leaves out G7's
and G8's targets. G10 also leaves out G9's twelve. Nothing here reads dagayn.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent.parent / "g9_go" / "keys"))
from g9pools import tagged  # noqa: E402

G9_TARGETS = {
    "assert/assertion_compare.go::compareTwoValues",
    "assert/assertion_forward.go::Assertions.True",
    "assert/assertion_forward.go::Assertions.False",
    "assert/assertion_forward.go::Assertions.Contains",
    "zstd/encoder.go::Encoder.ReadFrom",
    "gzhttp/writer/zstdkp/zstdkp.go::NewWriter",
    "gzip/gzip.go::Writer.Reset",
    "zstd/enc_base.go::fastBase.CRC",
    "exit.go::exitCodeFromError",
    "context.go::flipBoolValue",
    "model.go::Value.IsBool",
    "help.go::LineIndenter",
}
REPOS = ("testify", "compress", "kong", "chi")


def g10_tagged(inputs: Path, target: Path, name: str) -> dict:
    out = tagged(inputs, target, name)
    for kind in ("kind1", "kind2", "kind3", "kind4"):
        out[kind] = [e for e in out[kind] if e["target"] not in G9_TARGETS]
    return out


if __name__ == "__main__":
    inputs = Path(sys.argv[1])
    targets = dict(arg.split("=", 1) for arg in sys.argv[2:])
    assert tuple(targets) == REPOS, f"pass {REPOS} in that order"
    out = {name: g10_tagged(inputs, Path(t), name) for name, t in targets.items()}
    print(json.dumps(out, indent=1, sort_keys=True))
