"""Every attribute call `.NAME(...)` and every attribute read `.NAME` in the given dirs.

    python sweep.py REPO NAME DIR [DIR ...] [--refs]

Prints `path:line<TAB>enclosing function (path::Qual.name)<TAB>receiver source`.
With --refs, attribute reads that are not calls are listed too (a method passed
as a value).
"""

import ast
import sys
from pathlib import Path

args = sys.argv[1:]
refs = "--refs" in args
args = [a for a in args if a != "--refs"]
repo, name, dirs = Path(args[0]), args[1], args[2:]


def scan(path: Path):
    src = path.read_text(encoding="utf-8")
    tree = ast.parse(src)
    rel = path.relative_to(repo).as_posix()

    def walk(node, stack):
        for child in ast.iter_child_nodes(node):
            if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                walk(child, stack + [child])
                continue
            if isinstance(child, ast.Attribute) and child.attr == name:
                parent_call = getattr(child, "_is_func", False)
                if parent_call or refs:
                    funcs = [s for s in stack if not isinstance(s, ast.ClassDef)]
                    qual = ".".join(s.name for s in stack) if funcs else "<module>"
                    kind = "call" if parent_call else "ref"
                    receiver = ast.get_source_segment(src, child.value)
                    print(f"{rel}:{child.lineno}\t{rel}::{qual}\t{kind}\t{receiver}")
            if isinstance(child, ast.Call) and isinstance(child.func, ast.Attribute):
                child.func._is_func = True
            walk(child, stack)

    # Mark call funcs before visiting attributes.
    for n in ast.walk(tree):
        if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute):
            n.func._is_func = True
    walk(tree, [])


for d in dirs:
    for path in sorted((repo / d).rglob("*.py")):
        scan(path)
