"""G10's questions and keys from the draw, on the harness's targets.

    python g10keys.py INPUTS testify=TARGET compress=TARGET kong=TARGET chi=TARGET [--write]

INPUTS holds build_inputs.sh's files for every target. testify, compress, and
kong have no kind 4 question, so their targets are the tags' archives and
their inputs are the pools'. chi has one (H4), so its target is the tag's
archive plus the change commit (`../chi-change.patch`), and its inputs are
built on that target. Without --write it recomputes every key and compares it
with the tasks files; with --write it writes them.

The prompts and keys are G8's (`../../g8_go/keys/make_keys.py`), with G9's
package name from the file's `package` clause (`../../g9_go/keys/g9keys.py`,
imported unchanged). Nothing here reads dagayn.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
G10 = HERE.parent
sys.path.insert(0, str(HERE.parent.parent / "g9_go" / "keys"))
import g9keys  # noqa: E402
from pools import Index  # noqa: E402

g8 = g9keys.g8
REPOS = {
    "testify": {**g9keys.REPOS["testify"], "first": 13},
    "compress": {**g9keys.REPOS["compress"], "first": 13},
    "kong": {**g9keys.REPOS["kong"], "first": 5},
    "chi": {
        "module": "github.com/go-chi/chi/v5",
        "version": "v5.5.0",
        "commit": "3a33ba578e0dd93444e0ff386cc26e665d92e31e",
        "prefix": "H",
        "first": 1,
        "change": {
            "patch": "chi-change.patch",
            "message": "middleware: write cW's string as it is when it has no arguments",
        },
    },
}


# The candidate arm's line (`--candidate-transport cli`; PREREG.md, "Arms"):
# the CLI pilot's, unchanged. `graph` (MCP) and `none` are G8's.
CLI_LINE = (
    "- dagayn (このリポジトリのコードのグラフを引く CLI) がシェルで使える。"
    "まずそれで調べ、必要なら他の道具で確かめること。"
    "呼び方: `dagayn tool query_graph_tool --arg pattern=callers_of "
    "--arg target='path/to/file.go::Type.Method'`。出力は 1 行の JSON。"
    "pattern は callers_of, callees_of, tests_for, source_of, importers_of, imports_of, "
    "children_of, inheritors_of など。"
    "target は qualified name (`path::Name`、`path::Type.Method`、Go なら `path::(*T).M` も可) "
    "かファイルのパス。depth (呼び出しをたどる深さ)、offset (続きのページ) も --arg で渡せる。"
    "返答の next は、次に呼ぶとよい呼び出し (tool と args) で、"
    "`dagayn tool <tool> --arg <key>=<value> ...` として実行できる。"
    "status が ok で results_complete が true、summary がグラフに欠けがあると言っていない返答は、"
    "その summary の範囲の完全な答えなので、grep で数え直さないこと。"
    "頼る行だけを source_of で読むこと。\n"
)


def tasks_file(name: str, inputs: Path, tree: Path, picks: list[dict]) -> dict:
    repo = REPOS[name]
    ix = Index(inputs, tree, name)
    mine = [p for p in picks if p["repository"] == name]
    has_kind4 = any(p["kind"] == 4 for p in mine)
    assert has_kind4 == ("change" in repo), "a kind 4 pick needs a change commit"
    tasks = {}
    for i, pick in enumerate(mine):
        t = g9keys.Target(pick["target"], tree)
        tasks[f"{repo['prefix']}{repo['first'] + i}"] = {
            "prompt": g8.prompt(pick["kind"], t),
            "kind": "symbols",
            "length": pick["length"],
            "stratum": pick["stratum"],
            "question_kind": pick["kind"],
            "truth": sorted(g8.key(pick["kind"], pick, ix, inputs, name, tree)),
        }
    change = repo.get("change")
    about = (
        f"G10 (eval/g10_go/PREREG.md): {len(mine)} Go questions on {name} {repo['version']} "
        f"({repo['commit'][:7]})"
        + (f" with the change in {change['patch']} as the last commit" if change else "")
        + ", drawn by keys/g10draw.py. Keys were built without dagayn (keys/g10keys.py)."
    )
    spec = json.loads((G10.parent / "g8_go" / "tasks_testify.json").read_text())
    preamble = spec["preamble"].replace(
        "github.com/stretchr/testify v1.12.1", f"{repo['module']} {repo['version']}"
    )
    out = {"_about": about, "source_commit": repo["commit"]}
    if change:
        out["change"] = change
    return {
        **out,
        "preamble": preamble,
        "condition_line": {**spec["condition_line"], "cli": CLI_LINE},
        "tasks": tasks,
    }


def main() -> int:
    inputs = Path(sys.argv[1])
    trees = dict(a.split("=", 1) for a in sys.argv[2:] if "=" in a)
    assert tuple(trees) == tuple(REPOS), f"pass {tuple(REPOS)} in that order"
    picks = json.loads((G10 / "draw.json").read_text())["picks"]
    bad = 0
    for name, tree in trees.items():
        spec = tasks_file(name, inputs, Path(tree), picks)
        out = G10 / f"tasks_{name}.json"
        if "--write" in sys.argv:
            out.write_text(json.dumps(spec, indent=1, ensure_ascii=False) + "\n")
            continue
        registered = json.loads(out.read_text())
        bad += spec != registered
        for tid, task in spec["tasks"].items():
            same = task == registered["tasks"].get(tid)
            print(f"{tid}: {'agrees' if same else 'DIFFERS'} ({len(task['truth'])} entries)")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
