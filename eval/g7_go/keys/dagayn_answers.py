"""What the shipped dagayn answers to each G7 question, scored against the key.

    python dagayn_answers.py TESTIFY_TARGET COMPRESS_TARGET

Each target is the harness's repository (archive plus the change commit)
with a graph from `dagayn build --local-embedding none`, as the harness
builds it. The calls are the ones an agent would make first, at the default
detail level.
"""

import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
DAGAYN = "dagayn"


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


def main() -> int:
    repos = {"testify": Path(sys.argv[1]), "compress": Path(sys.argv[2])}
    keys = {}
    for name in repos:
        spec = json.loads((HERE.parent / f"tasks_{name}.json").read_text())
        keys.update({t: set(s["truth"]) for t, s in spec["tasks"].items()})
    plan = {
        "T1": ("testify", "assert/assertions.go::CollectT.Errorf", "reach"),
        "T2": ("testify", "assert/assertion_forward.go::Assertions.Equal", "callers"),
        "T3": ("testify", "suite/suite.go::Suite.T", "callers"),
        "T4": ("testify", "assert/assertions.go::ObjectsAreEqual", "change"),
        "C1": ("compress", "flate/stateless.go::statelessEnc", "reach"),
        "C2": ("compress", "zstd/internal/xxhash/xxhash.go::Digest.Reset", "callers"),
        "C3": ("compress", "flate/deflate.go::compressor.storeFast", "callers"),
        "C4": ("compress", "flate/deflate.go::Writer.Reset", "change"),
    }
    report = {}
    for task, (name, target, kind) in plan.items():
        repo, truth = repos[name], keys[task]
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
        report[task] = entry
    print(json.dumps(report, indent=1, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
