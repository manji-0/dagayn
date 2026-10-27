"""What the shipped dagayn answers to each question, scored against the key.

    python dagayn_answers.py HTTPX_TARGET DAGAYN_TARGET

Each target is the harness's repository (archive plus the change commit) with
a graph from `dagayn build --scip`. The calls are the ones an agent would make
first, at the default detail level.
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


def rows(reply: dict, tests_only: bool = False) -> set[str]:
    found = {r["qualified_name"] for r in reply.get("results") or []}
    if tests_only:
        found = {q for q in found if q.startswith("tests/")}
    return found


def unchanged_callers(reply: dict) -> set[str]:
    found = set()
    for f in reply.get("findings") or []:
        if f.get("kind") == "unchanged_caller" or f.get("type") == "unchanged_caller":
            for key in ("caller", "callers", "node", "qualified_name"):
                v = f.get(key)
                if isinstance(v, str):
                    found.add(v)
                elif isinstance(v, list):
                    found.update(
                        x if isinstance(x, str) else x.get("qualified_name", "") for x in v
                    )
    return found


def main() -> int:
    httpx, dagayn = Path(sys.argv[1]), Path(sys.argv[2])
    keys = {}
    for repo in ("httpx", "dagayn"):
        for task, spec in json.loads((HERE.parent / f"tasks_{repo}.json").read_text())[
            "tasks"
        ].items():
            keys[task] = set(spec["truth"])
    plan = {
        "X1": (httpx, "httpx/_main.py::format_response_headers", "reach"),
        "X2": (httpx, "httpx/_models.py::Headers.get", "callers"),
        "X3": (httpx, "httpx/_decoders.py::GZipDecoder.decode", "callers"),
        "X4": (httpx, "httpx/_models.py::Response.close", "change"),
        "D1": (dagayn, "dagayn/scip_overlay.py::_timeout", "reach"),
        "D2": (dagayn, "dagayn/embeddings_store.py::EmbeddingStore.close", "callers"),
        "D3": (dagayn, "dagayn/tools/queue_worker.py::_execute_embed", "callers"),
        "D4": (dagayn, "dagayn/task_queue.py::WorkerLock.acquire", "change"),
    }
    report = {}
    for task, (repo, target, kind) in plan.items():
        truth = keys[task]
        entry: dict = {"target": target}
        if kind == "reach":
            direct = tool(repo, "query_graph_tool", pattern="tests_for", target=target)
            deep = tool(repo, "query_graph_tool", pattern="callers_of", target=target, depth=6)
            got = rows(direct) | rows(deep, tests_only=True)
            entry["tests_for"] = {"rows": len(rows(direct)), "test_reach": direct.get("test_reach")}
            entry["callers_of_depth6_tests"] = len(rows(deep, tests_only=True))
            entry["summary"] = deep.get("summary")
        elif kind == "callers":
            reply = tool(repo, "query_graph_tool", pattern="callers_of", target=target)
            got = rows(reply)
            entry["summary"] = reply.get("summary")
        else:
            reply = tool(repo, "query_graph_tool", pattern="callers_of", target=target)
            review = tool(repo, "review_tool", mode="changes")
            entry["callers_of"] = sorted(rows(reply))
            entry["review_findings"] = [
                {
                    k: f.get(k)
                    for k in ("kind", "type", "summary", "title", "node", "caller", "callers")
                    if f.get(k)
                }
                for f in review.get("findings") or []
            ]
            got = unchanged_callers(review)
            entry["summary"] = reply.get("summary")
        entry["found"] = len(got & truth)
        entry["key"] = len(truth)
        entry["missing"] = sorted(truth - got)
        entry["extra"] = sorted(got - truth)
        report[task] = entry
    print(json.dumps(report, indent=1, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
