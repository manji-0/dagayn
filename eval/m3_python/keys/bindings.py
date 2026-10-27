"""For every `.NAME(...)` call on a plain or `self.x` receiver, show where the
enclosing function binds that receiver (assignments, with-targets, parameters).

    python bindings.py REPO NAME DIR [DIR ...]
"""

import ast
import sys
from pathlib import Path

repo, name, dirs = Path(sys.argv[1]), sys.argv[2], sys.argv[3:]


def binds(func, recv):
    out = []
    for n in ast.walk(func):
        if isinstance(n, (ast.Assign, ast.AnnAssign, ast.AugAssign)):
            targets = n.targets if isinstance(n, ast.Assign) else [n.target]
            for t in targets:
                for x in ast.walk(t):
                    if ast.unparse(x) == recv:
                        out.append(f"L{n.lineno}: {ast.unparse(n)[:140]}")
        elif (
            isinstance(n, ast.withitem)
            and n.optional_vars is not None
            and ast.unparse(n.optional_vars) == recv
        ):
            out.append(f"with: {ast.unparse(n)[:140]}")
    if isinstance(func, (ast.FunctionDef, ast.AsyncFunctionDef)):
        for a in func.args.args + func.args.kwonlyargs:
            if a.arg == recv:
                out.append(f"param: {ast.unparse(a)}")
    return out


for d in dirs:
    for path in sorted((repo / d).rglob("*.py")):
        tree = ast.parse(path.read_text())
        rel = path.relative_to(repo).as_posix()
        funcs = [
            n for n in ast.walk(tree) if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef))
        ]
        for call in ast.walk(tree):
            if (
                isinstance(call, ast.Call)
                and isinstance(call.func, ast.Attribute)
                and call.func.attr == name
            ):
                recv = ast.unparse(call.func.value)
                inner = [f for f in funcs if f.lineno <= call.lineno <= f.end_lineno]
                func = min(inner, key=lambda f: f.end_lineno - f.lineno) if inner else tree
                fname = getattr(func, "name", "<module>")
                b = binds(func, recv) if recv != "self" else ["self"]
                print(f"{rel}:{call.lineno} [{fname}] {recv}.{name}()  <=  {' | '.join(b)[:300]}")
