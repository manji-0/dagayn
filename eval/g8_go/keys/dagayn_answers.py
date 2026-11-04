"""What dagayn answers to each G8 question and each G7 question, scored against the keys.

    DAGAYN=/path/to/dagayn python dagayn_answers.py G8_TESTIFY G8_COMPRESS G7_TESTIFY G7_COMPRESS

Each argument is a harness target (archive plus its change commit) with a
graph from `dagayn build --local-embedding none`, as the harness builds it.
The calls are the ones an agent would make first, at the default detail
level; they are G7's (`../../g7_go/keys/dagayn_answers.py`). The report also
records each graph's extractor versions.
"""

from __future__ import annotations

import json
import os
import sqlite3
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
DAGAYN = os.environ.get("DAGAYN", "dagayn")
PLAN = {
    "g8": {
        "T5": ("testify", "mock/mock.go::diffArguments", "reach"),
        "T6": ("testify", "assert/assertion_forward.go::Assertions.JSONEq", "callers"),
        "T7": ("testify", "suite/suite.go::Suite.SetT", "callers"),
        "T8": ("testify", "assert/assertions.go::YAMLEq", "change"),
        "C5": ("compress", "zip/reader.go::File.OpenRaw", "reach"),
        "C6": ("compress", "gzip/gzip.go::Writer.init", "callers"),
        "C7": ("compress", "gzhttp/writer/gzkp/gzkp.go::NewWriter", "callers"),
        "C8": ("compress", "zstd/blockenc.go::blockHeader.setSize", "change"),
    },
    "g7": {
        "T1": ("testify", "assert/assertions.go::CollectT.Errorf", "reach"),
        "T2": ("testify", "assert/assertion_forward.go::Assertions.Equal", "callers"),
        "T3": ("testify", "suite/suite.go::Suite.T", "callers"),
        "T4": ("testify", "assert/assertions.go::ObjectsAreEqual", "change"),
        "C1": ("compress", "flate/stateless.go::statelessEnc", "reach"),
        "C2": ("compress", "zstd/internal/xxhash/xxhash.go::Digest.Reset", "callers"),
        "C3": ("compress", "flate/deflate.go::compressor.storeFast", "callers"),
        "C4": ("compress", "flate/deflate.go::Writer.Reset", "change"),
    },
}


def tool(repo: Path, name: str, **args) -> dict:
    cmd = [DAGAYN, "tool", name]
    for k, v in args.items():
        cmd += ["--arg", f"{k}={json.dumps(v) if not isinstance(v, str) else v}"]
    out = subprocess.run(cmd, cwd=repo, capture_output=True, text=True, check=True).stdout
    return json.loads(out)


def rows(reply: dict) -> set[str]:
    return {r["qualified_name"] for r in reply.get("results") or []}


def top_level_tests(names: set[str]) -> set[str]:
    return {
        q
        for q in names
        if q.split("::")[0].endswith("_test.go") and q.split("::")[1].startswith("Test")
    }


def unchanged_callers(reply: dict) -> set[str]:
    found = set()
    for f in reply.get("findings") or []:
        if f.get("kind") == "unchanged_caller":
            found.update(f.get("targets") or [])
            if f.get("qualified_name"):
                found.add(f["qualified_name"])
    return found


def extractor_versions(repo: Path) -> dict:
    db = repo / ".dagayn" / "graph.db"
    with sqlite3.connect(f"file:{db}?mode=ro", uri=True) as conn:
        got = dict(conn.execute("select key, value from metadata").fetchall())
    versions = dict(kv.split("=") for kv in got.get("extractor_versions", "").split(",") if kv)
    return {
        "go_extractor": versions.get("go"),
        "last_build_type": got.get("last_build_type"),
        "postprocess_level": got.get("postprocess_level"),
    }


def answer(repo: Path, target: str, kind: str, truth: set[str]) -> dict:
    entry: dict = {"target": target}
    if kind == "reach":
        direct = tool(repo, "query_graph_tool", pattern="tests_for", target=target)
        deep = tool(repo, "query_graph_tool", pattern="callers_of", target=target, depth=6)
        got = top_level_tests(rows(direct)) | top_level_tests(rows(deep))
        entry["tests_for"] = {
            "rows": len(rows(direct)),
            "top_level_tests": len(top_level_tests(rows(direct))),
            "test_reach": direct.get("test_reach"),
        }
        entry["callers_of_depth6"] = {
            "rows": len(rows(deep)),
            "top_level_tests": len(top_level_tests(rows(deep))),
            "results_complete": deep.get("results_complete"),
        }
    else:
        reply = tool(repo, "query_graph_tool", pattern="callers_of", target=target)
        got = rows(reply)
        entry["callers_of"] = {
            "rows": len(got),
            "summary": reply.get("summary"),
            "results_complete": reply.get("results_complete"),
            "confidence": reply.get("confidence"),
        }
        if kind == "change":
            review = tool(repo, "review_tool", mode="changes")
            entry["review_summary"] = review.get("summary")
            entry["review_unchanged_callers"] = sorted(unchanged_callers(review))
            entry["edited_callers_in_rows"] = sorted(
                q for q in got - truth if q.split("::")[0].endswith(".go")
            )
    entry["found"] = len(got & truth)
    entry["key"] = len(truth)
    entry["missing"] = sorted(truth - got)
    entry["extra"] = sorted(got - truth)
    return entry


def main() -> int:
    repos = {
        ("g8", "testify"): Path(sys.argv[1]),
        ("g8", "compress"): Path(sys.argv[2]),
        ("g7", "testify"): Path(sys.argv[3]),
        ("g7", "compress"): Path(sys.argv[4]),
    }
    report: dict = {"graphs": {f"{e}/{n}": extractor_versions(p) for (e, n), p in repos.items()}}
    for exp, plan in PLAN.items():
        keys = {}
        for name in ("testify", "compress"):
            spec = json.loads((HERE.parent.parent / f"{exp}_go" / f"tasks_{name}.json").read_text())
            keys.update({t: set(s["truth"]) for t, s in spec["tasks"].items()})
        for task, (name, target, kind) in plan.items():
            report[task] = answer(repos[(exp, name)], target, kind, keys[task])
    print(json.dumps(report, indent=1, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
