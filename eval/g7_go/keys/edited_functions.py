"""Go functions whose definition the last commit changed.

    python edited_functions.py REPO FUNCS_TSV

FUNCS_TSV is gofuncs' listing at HEAD. A function is edited when a line the
commit added lies inside its definition at HEAD, or a line it removed lay
between two lines that are inside it (the hunk's position at HEAD).
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from per_test_cover import enclosing, load_funcs  # noqa: E402

HUNK = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")


def edited(repo: Path, funcs_tsv: Path) -> set[str]:
    funcs = load_funcs(funcs_tsv)
    diff = subprocess.run(
        ["git", "diff", "-U0", "HEAD~1", "HEAD", "--", "*.go"],
        cwd=repo, capture_output=True, text=True, check=True,
    ).stdout  # fmt: skip
    out: set[str] = set()
    path = ""
    for line in diff.splitlines():
        if line.startswith("+++ "):
            path = line[6:] if line.startswith("+++ b/") else ""
        elif (m := HUNK.match(line)) and path:
            start, count = int(m.group(1)), int(m.group(2) or 1)
            # A pure removal (count 0) sits after line `start`.
            lines = range(start, start + count) if count else (start, start + 1)
            for n in lines:
                fn = enclosing(funcs.get(path, []), n)
                if fn and (count or enclosing(funcs.get(path, []), start) == fn):
                    out.add(fn)
    return out


if __name__ == "__main__":
    for fn in sorted(edited(Path(sys.argv[1]), Path(sys.argv[2]))):
        print(fn)
