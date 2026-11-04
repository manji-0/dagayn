"""G8's questions and keys from the draw, on the harness's targets.

    python make_keys.py INPUTS TESTIFY_TARGET COMPRESS_TARGET [--write]

INPUTS holds build_inputs.sh's files for both targets (the tag's archive plus
the G8 change commit). Without --write it recomputes every key and compares
it with tasks_testify.json and tasks_compress.json; with --write it writes
them. Prompts come from one template per kind, with only the target's names
filled in. Nothing here reads dagayn.

Keys:
- kind 1: the `Test` functions that executed the target in both coverage runs;
- kind 2: the functions holding a scip-go call of the target;
- kind 3: those, plus the calls of the interface method it alone implements,
  or of the fields that hold its value (an assignment is not a call);
- kind 4: the kind 2 key less the functions the change commit edited.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent.parent / "g7_go" / "keys"))
from edited_functions import edited  # noqa: E402
from pools import Index, reach  # noqa: E402

G8 = HERE.parent
REPOS = {
    "testify": {
        "module": "github.com/stretchr/testify",
        "version": "v1.12.1",
        "commit": "959dbdacf1533e155162811ea90c90117a420463",
        "ids": ("T5", "T6", "T7", "T8"),
        "message": "assert: ignore the whitespace around YAML documents in YAMLEq's shortcut",
    },
    "compress": {
        "module": "github.com/klauspost/compress",
        "version": "v1.20.1",
        "commit": "5d880f230c38a0fc806b9ca1613103a44feff0ac",
        "ids": ("C5", "C6", "C7", "C8"),
        "message": "zstd: keep a block header's size within its 21 bits",
    },
}
SCOPE = (
    "対象はリポジトリの .go ファイル (テストファイルを含む。Go のツールと同じく、"
    "`_` や `.` で始まるディレクトリと `testdata` は除く)。"
)
FORMAT = (
    '形式は "ファイルパス::関数名"、メソッドなら "ファイルパス::型名.メソッド名" '
    "(ファイルパスはリポジトリルートから。型名にポインタの `*` や括弧は付けない。"
    "呼び出しを囲む、トップレベルで宣言された関数・メソッド。"
    "関数リテラルの中の呼び出しは、それを囲む宣言のものとして数える)。"
)
CHANGED = (
    "「変更された」は、コミットの差分が、その関数・メソッドの定義 "
    "(`func` の行から本体の最後の行まで) の中の行を変えた・足した・消したこと。"
)


class Target:
    def __init__(self, qname: str, tree: Path):
        self.qname = qname
        self.path, name = qname.split("::")
        self.package = Path(self.path).parent.name
        self.method = "." in name
        self.type, _, self.name = name.rpartition(".")
        decl = re.compile(rf"^func \((\w+ )?(\*?){re.escape(self.type)}\b")
        lines = (tree / self.path).read_text().splitlines()
        self.pointer = self.method and any(
            (m := decl.match(line)) and m.group(2) and re.search(rf"\) {self.name}\(", line)
            for line in lines
        )

    @property
    def display(self) -> str:
        if not self.method:
            return self.name
        return f"({'*' if self.pointer else ''}{self.type}).{self.name}"

    @property
    def what(self) -> str:
        return "メソッド" if self.method else "関数"

    def receiver_rule(self) -> str:
        star = "*" if self.pointer else ""
        return (
            f"`x.{self.name}(...)` は、x の静的な型が {self.package} パッケージの "
            f"`{star}{self.type}` (または `{self.type}`) か、それを埋め込んだ型で、"
            "呼び出しがこのメソッドに解決されるなら数える。"
        )


def prompt(kind: int, t: Target) -> str:
    head = f"`{t.path}` の{t.what} `{t.display}`"
    if kind == 1:
        return (
            f"{head} の本体は、どのテストを実行したときに実行されるか。"
            "`func TestXxx(t *testing.T)` の形のテスト関数のうち、そのテストを 1 つだけ"
            "実行したとき (`go test -run '^TestXxx$'` で、そのサブテストを含む。"
            f"`-short` は付けない) に `{t.display}` の本体が実行されるものをすべて"
            "挙げてください。対象はリポジトリのすべてのパッケージのテスト。"
            "`Benchmark`・`Fuzz`・`Example` で始まる関数は数えない。"
            '形式は "ファイルパス::テスト関数名" (ファイルパスはリポジトリルートから)。'
        )
    if kind == 2:
        return (
            f"{head} を直接呼んでいる関数・メソッドをすべて挙げてください。{SCOPE}"
            f"{t.receiver_rule()}インタフェース型の値を通した呼び出し、ほかの型の "
            f"`{t.name}`、同じ名前の関数、標準ライブラリや外部パッケージの関数・メソッドは"
            f"数えない。コメントでの言及は呼び出しではない。{FORMAT}"
        )
    if kind == 3 and t.method:
        return (
            f"{head} を直接呼び得る関数・メソッドをすべて挙げてください。{SCOPE}"
            f"`x.{t.name}(...)` という呼び出しは、実行時にその呼び出しが `{t.display}` を"
            f"実行し得るなら数える (`{t.type}` を埋め込んだ型の値での呼び出しや、"
            f"インタフェース型の値を通した呼び出しを含む)。ほかの型の `{t.name}` や"
            f"コメントでの言及は数えない。{FORMAT}"
        )
    if kind == 3:
        return (
            f"{head} を直接呼び得る関数・メソッドをすべて挙げてください。{SCOPE}"
            "関数値 (変数・構造体のフィールド・引数に入った関数、メソッド式、メソッド値) を"
            f"通した呼び出しは、実行時にその値が `{t.display}` になり得るなら数える。"
            "値を代入したり渡したりするだけで呼び出さない関数は数えない。"
            f"コメントでの言及は呼び出しではない。{FORMAT}"
        )
    count = (
        f"{t.receiver_rule()}インタフェース型の値を通した呼び出し、ほかの型の `{t.name}` は"
        "数えない。"
        if t.method
        else "ほかのパッケージの同じ名前の関数や、名前の違う関数の呼び出しは数えない。"
    )
    return (
        f"最後のコミットは {head} の振る舞いを変えた。このコミットの時点で "
        f"`{t.display}` を直接呼んでいる関数・メソッドのうち、このコミットで変更されて"
        f"いないものをすべて挙げてください。{SCOPE}{count}{CHANGED}"
        f"コメントでの言及は呼び出しではない。{FORMAT}"
    )


def key(kind: int, pick: dict, ix: Index, inputs: Path, name: str, tree: Path) -> set[str]:
    q = pick["target"]
    if kind == 1:
        return reach(inputs, name).get(q, set())
    s = ix.symbol_of[q]
    found = ix.callers(s)
    if kind == 3:
        for other in pick.get("interface_methods", []) + pick.get("holders", []):
            # A scip-go symbol carries a version of the module that differs
            # between checkouts; the rest of it names the symbol.
            suffix = " " + other.split(" ", 4)[4]
            for sym in [x for x in ix.refs if x.endswith(suffix)]:
                found |= ix.callers(sym)
    if kind == 4:
        found = (found - {q}) - edited(tree, inputs / f"{name}.funcs.tsv")
    return found


def tasks_file(name: str, inputs: Path, tree: Path, picks: dict) -> dict:
    repo = REPOS[name]
    ix = Index(inputs, tree, name)
    tasks = {}
    for kind, tid in zip((1, 2, 3, 4), repo["ids"]):
        pick = picks[f"{name}/kind{kind}"]
        t = Target(pick["target"], tree)
        tasks[tid] = {
            "prompt": prompt(kind, t),
            "kind": "symbols",
            "truth": sorted(key(kind, pick, ix, inputs, name, tree)),
        }
    return {
        "_about": (
            f"G8 (eval/g8_go/PREREG.md): four Go questions on {name} {repo['version']} "
            f"({repo['commit'][:7]}) with the change in {name}-change.patch as the last "
            "commit, drawn by keys/draw.py. Keys were built without dagayn (keys/make_keys.py)."
        ),
        "source_commit": repo["commit"],
        "change": {"patch": f"{name}-change.patch", "message": repo["message"]},
        "preamble": (
            f"このリポジトリ (カレントディレクトリ。Go のモジュール {repo['module']} "
            f"{repo['version']} のチェックアウト) について、次の質問に答えてください。\n"
            "- リポジトリのファイルは変更しないこと。一時ファイルが必要なら /tmp の下に作ること。\n"
            "- テストやリポジトリのコードは実行しないこと (go test や go run は使わない。"
            "読む・検索する・解析用のスクリプトを書いて実行するのはよい)。\n"
            "- 回答の最後に、次の形式の JSON コードブロックを 1 つだけ置くこと: "
            '```json\n{"answer": [...]}\n```\n\n質問: '
        ),
        "condition_line": {
            "graph": (
                "- dagayn の MCP ツール (サーバ名 dagayn) が使える。"
                "まずそれで調べ、必要なら他の道具で確かめること。\n"
            ),
            "none": (
                "- dagayn の MCP ツールは使えない。"
                "標準の道具 (検索・ファイルの読み込み・シェル) で調べること。\n"
            ),
        },
        "tasks": tasks,
    }


def main() -> int:
    inputs = Path(sys.argv[1])
    trees = dict(zip(REPOS, map(Path, sys.argv[2:4])))
    picks = json.loads((G8 / "draw.json").read_text())["picks"]
    bad = 0
    for name, tree in trees.items():
        spec = tasks_file(name, inputs, tree, picks)
        out = G8 / f"tasks_{name}.json"
        if "--write" in sys.argv:
            out.write_text(json.dumps(spec, indent=1, ensure_ascii=False) + "\n")
            continue
        registered = json.loads(out.read_text())
        for tid, task in spec["tasks"].items():
            same = task == registered["tasks"][tid]
            bad += not same
            print(f"{tid}: {'agrees' if same else 'DIFFERS'} ({len(task['truth'])} entries)")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
