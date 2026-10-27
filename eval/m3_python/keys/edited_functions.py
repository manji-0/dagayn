"""Functions a commit edited: those whose span (def line to last line, decorators
included) holds a changed line, on the new side (added lines) or the old side
(removed lines).

    python edited_functions.py REPO [BASE] [HEAD]
"""

import ast
import re
import subprocess
import sys
from pathlib import Path

repo = Path(sys.argv[1])
base = sys.argv[2] if len(sys.argv) > 2 else "HEAD~1"
head = sys.argv[3] if len(sys.argv) > 3 else "HEAD"
GIT = "/usr/bin/git"


def spans(source: str):
    tree = ast.parse(source)
    out = []

    def walk(node, prefix):
        for child in ast.iter_child_nodes(node):
            if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)):
                start = min([child.lineno] + [d.lineno for d in child.decorator_list])
                out.append((prefix + child.name, start, child.end_lineno))
                walk(child, prefix + child.name + ".")
            elif isinstance(child, ast.ClassDef):
                walk(child, prefix + child.name + ".")

    walk(tree, "")
    return out


diff = subprocess.run(
    [GIT, "-C", str(repo), "diff", "-U0", base, head], capture_output=True, text=True, check=True
).stdout
changed = {}  # (side, path) -> set(lines)
old_path = new_path = None
for line in diff.splitlines():
    if line.startswith("--- "):
        old_path = line[6:] if line.startswith("--- a/") else None
    elif line.startswith("+++ "):
        new_path = line[6:] if line.startswith("+++ b/") else None
    elif line.startswith("@@"):
        m = re.match(r"@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@", line)
        o, oc, n, nc = int(m[1]), int(m[2] or 1), int(m[3]), int(m[4] or 1)
        if old_path and oc:
            changed.setdefault(("old", old_path), set()).update(range(o, o + oc))
        if new_path and nc:
            changed.setdefault(("new", new_path), set()).update(range(n, n + nc))
edited = set()
for (side, path), lines in changed.items():
    if not path.endswith(".py"):
        continue
    rev = base if side == "old" else head
    src = subprocess.run(
        [GIT, "-C", str(repo), "show", f"{rev}:{path}"], capture_output=True, text=True, check=True
    ).stdout
    for qual, start, end in spans(src):
        if lines & set(range(start, end + 1)):
            edited.add(f"{path}::{qual}")
for e in sorted(edited):
    print(e)
