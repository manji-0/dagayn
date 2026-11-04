"""G9's questions and keys from the draw, on the harness's targets.

    python g9keys.py INPUTS testify=TARGET compress=TARGET kong=TARGET [--write]

INPUTS holds build_inputs.sh's files for every target. The draw holds no kind
4 question, so each target is the tag's archive with no change commit and
INPUTS are the files the pools were built from. Without --write it recomputes
every key and compares it with the tasks files; with --write it writes them.
The prompts and keys are G8's (`../../g8_go/keys/make_keys.py`, imported
unchanged): one prompt template per kind with only the target's names
filled in, and the key rule of each kind. Nothing here reads dagayn.
"""

from __future__ import annotations

import importlib.util
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
G9 = HERE.parent
G8_KEYS = HERE.parent.parent / "g8_go" / "keys"
sys.path.insert(0, str(G8_KEYS))
_spec = importlib.util.spec_from_file_location("g8_make_keys", G8_KEYS / "make_keys.py")
g8 = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(g8)
from pools import Index  # noqa: E402

REPOS = {
    "testify": {
        "module": "github.com/stretchr/testify",
        "version": "v1.12.1",
        "commit": "959dbdacf1533e155162811ea90c90117a420463",
        "prefix": "T",
        "first": 9,
    },
    "compress": {
        "module": "github.com/klauspost/compress",
        "version": "v1.20.1",
        "commit": "5d880f230c38a0fc806b9ca1613103a44feff0ac",
        "prefix": "C",
        "first": 9,
    },
    "kong": {
        "module": "github.com/alecthomas/kong",
        "version": "v1.15.0",
        "commit": "14c97a2bfca94fff8da01fef6ceb1fc746303044",
        "prefix": "K",
        "first": 1,
    },
}
PACKAGE = re.compile(r"^package (\w+)", re.M)


class Target(g8.Target):
    """G8's target, with the package named by the file's `package` clause.
    G8 took the directory's name, which is empty for a file at the root."""

    def __init__(self, qname: str, tree: Path):
        super().__init__(qname, tree)
        self.package = PACKAGE.search((tree / self.path).read_text()).group(1)


def tasks_file(name: str, inputs: Path, tree: Path, picks: list[dict]) -> dict:
    repo = REPOS[name]
    ix = Index(inputs, tree, name)
    mine = [p for p in picks if p["repository"] == name]
    assert not any(p["kind"] == 4 for p in mine), "a kind 4 pick needs a change commit"
    tasks = {}
    for i, pick in enumerate(mine):
        t = Target(pick["target"], tree)
        tasks[f"{repo['prefix']}{repo['first'] + i}"] = {
            "prompt": g8.prompt(pick["kind"], t),
            "kind": "symbols",
            "length": pick["length"],
            "stratum": pick["stratum"],
            "question_kind": pick["kind"],
            "truth": sorted(g8.key(pick["kind"], pick, ix, inputs, name, tree)),
        }
    about = (
        f"G9 (eval/g9_go/PREREG.md): four Go questions on {name} {repo['version']} "
        f"({repo['commit'][:7]}), drawn by keys/g9draw.py. Keys were built without "
        "dagayn (keys/g9keys.py)."
    )
    spec = json.loads((G8_KEYS.parent / "tasks_testify.json").read_text())
    preamble = spec["preamble"].replace(
        "github.com/stretchr/testify v1.12.1", f"{repo['module']} {repo['version']}"
    )
    return {
        "_about": about,
        "source_commit": repo["commit"],
        "preamble": preamble,
        "condition_line": spec["condition_line"],
        "tasks": tasks,
    }


def main() -> int:
    inputs = Path(sys.argv[1])
    trees = dict(a.split("=", 1) for a in sys.argv[2:] if "=" in a)
    assert tuple(trees) == tuple(REPOS), f"pass {tuple(REPOS)} in that order"
    picks = json.loads((G9 / "draw.json").read_text())["picks"]
    bad = 0
    for name, tree in trees.items():
        spec = tasks_file(name, inputs, Path(tree), picks)
        out = G9 / f"tasks_{name}.json"
        if "--write" in sys.argv:
            out.write_text(json.dumps(spec, indent=1, ensure_ascii=False) + "\n")
            continue
        registered = json.loads(out.read_text())
        same_file = spec == registered
        bad += not same_file
        for tid, task in spec["tasks"].items():
            same = task == registered["tasks"].get(tid)
            print(f"{tid}: {'agrees' if same else 'DIFFERS'} ({len(task['truth'])} entries)")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
