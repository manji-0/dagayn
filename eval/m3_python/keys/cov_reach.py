"""Tests whose run executed a function's body, from a coverage file with test contexts.

    python cov_reach.py REPO COVERAGE_FILE PKG path::Qual.name

The coverage file comes from `pytest --cov --cov-context=test` with
COVERAGE_CORE=ctrace. A context is `nodeid|setup`, `nodeid|run`, or
`nodeid|teardown`; a test reaches a function when any of the three executed a
line of its body (the statements after the `def` line, a leading docstring
excluded). The test id drops the parametrize suffix and is `path::name` or
`path::Class.name`.
"""

import ast
import re
import sqlite3
import sys
from collections import defaultdict
from pathlib import Path


def functions(repo: Path, pkg: str):
    """(relative path, qualified name, body lines) for every function under PKG."""
    for path in sorted((repo / pkg).rglob("*.py")):
        rel = path.relative_to(repo).as_posix()
        tree = ast.parse(path.read_text(encoding="utf-8"))

        def walk(node, prefix):
            for child in ast.iter_child_nodes(node):
                if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    lines = set()
                    for stmt in child.body:
                        if isinstance(stmt, ast.Expr) and isinstance(stmt.value, ast.Constant):
                            continue
                        lines.update(range(stmt.lineno, (stmt.end_lineno or stmt.lineno) + 1))
                    yield rel, prefix + child.name, lines
                    yield from walk(child, prefix + child.name + ".<locals>.")
                elif isinstance(child, ast.ClassDef):
                    yield from walk(child, prefix + child.name + ".")

        yield from walk(tree, "")


def test_id(context: str) -> str:
    nodeid = re.sub(r"\[.*\]$", "", context.rsplit("|", 1)[0])
    path, _, rest = nodeid.partition("::")
    return path + "::" + rest.replace("::", ".")


def load(repo: Path, cov: Path) -> dict[str, dict[int, set[str]]]:
    from coverage.numbits import numbits_to_nums

    db = sqlite3.connect(cov)
    files = dict(db.execute("select id, path from file"))
    contexts = dict(db.execute("select id, context from context"))
    hits: dict[str, dict[int, set[str]]] = defaultdict(lambda: defaultdict(set))
    for file_id, context_id, numbits in db.execute(
        "select file_id, context_id, numbits from line_bits"
    ):
        if not contexts[context_id]:
            continue
        try:
            rel = Path(files[file_id]).resolve().relative_to(repo.resolve()).as_posix()
        except ValueError:
            continue
        test = test_id(contexts[context_id])
        for line in numbits_to_nums(numbits):
            hits[rel][line].add(test)
    return hits


def reach(repo: Path, cov: Path, pkg: str, target: str) -> set[str]:
    hits = load(repo, cov)
    for rel, qual, body in functions(repo, pkg):
        if f"{rel}::{qual}" == target:
            return set().union(*(hits.get(rel, {}).get(line, set()) for line in body))
    raise KeyError(target)


if __name__ == "__main__":
    for test in sorted(reach(Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3], sys.argv[4])):
        print(test)
