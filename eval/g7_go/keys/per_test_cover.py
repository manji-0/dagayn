"""Which functions each top-level Go test executes, from one coverage run per test.

    python per_test_cover.py REPO FUNCS_TSV OUT_JSON [--short] [-j N] [--timeout S]

For every package with tests, the test binary is built once with
`go test -c -cover -coverpkg=<module>/...`, `go test -list .` names its
top-level tests, and the binary runs each `Test*` and `Fuzz*` function alone
(`-test.run '^Name$'`, from the package directory, as `go test` does) with
`-test.coverprofile`. A function counts as executed by a test when a profile
block with a count above 0 starts inside the function's lines (FUNCS_TSV
from gofuncs). Subtests belong to their top-level test.

OUT_JSON maps "dir/file_test.go::TestName" to the executed functions
("path::Recv.Method" or "path::Func"), and records each run's exit status.
Go comes from the environment (GOTOOLCHAIN).
"""

from __future__ import annotations

import argparse
import bisect
import json
import os
import re
import subprocess
import sys
import tempfile
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

BLOCK = re.compile(r"^(.+\.go):(\d+)\.(\d+),(\d+)\.(\d+) (\d+) (\d+)$")
TEST_DECL = re.compile(r"^func ((?:Test|Fuzz)\w*)\(", re.M)


def module_path(repo: Path) -> str:
    for line in (repo / "go.mod").read_text().splitlines():
        if line.startswith("module "):
            return line.split()[1]
    raise SystemExit("no module line")


def load_funcs(path: Path) -> dict[str, list[tuple[int, int, str]]]:
    by_file: dict[str, list[tuple[int, int, str]]] = defaultdict(list)
    for line in path.read_text().splitlines():
        rel, name, start, end = line.split("\t")
        by_file[rel].append((int(start), int(end), f"{rel}::{name}"))
    for spans in by_file.values():
        spans.sort()
    return by_file


def enclosing(spans: list[tuple[int, int, str]], line: int) -> str | None:
    i = bisect.bisect_right(spans, (line, 1 << 30, "")) - 1
    while i >= 0:
        start, end, name = spans[i]
        if start <= line <= end:
            return name
        i -= 1
    return None


def test_files(pkg_dir: Path) -> dict[str, str]:
    """Top-level test name -> the _test.go file that declares it."""
    out = {}
    for f in sorted(pkg_dir.glob("*_test.go")):
        for m in TEST_DECL.finditer(f.read_text(errors="replace")):
            out[m.group(1)] = f.name
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("repo", type=Path)
    ap.add_argument("funcs", type=Path)
    ap.add_argument("out", type=Path)
    ap.add_argument("--short", action="store_true")
    ap.add_argument("-j", type=int, default=6)
    ap.add_argument("--timeout", type=int, default=600)
    ap.add_argument("--packages", default="./...")
    a = ap.parse_args()
    repo = a.repo.resolve()
    mod = module_path(repo)
    funcs = load_funcs(a.funcs)
    fmt = "{{.Dir}}\t{{len .TestGoFiles}}\t{{len .XTestGoFiles}}"
    listed = subprocess.run(
        ["go", "list", "-f", fmt, *a.packages.split()],
        cwd=repo, capture_output=True, text=True, check=True,
    ).stdout  # fmt: skip
    pkgs = [
        Path(d)
        for d, t, x in (line.split("\t") for line in listed.splitlines())
        if int(t) + int(x) > 0
    ]
    work = Path(tempfile.mkdtemp(prefix="g7cover-"))
    jobs = []
    for i, d in enumerate(pkgs):
        binary = work / f"p{i}.test"
        built = subprocess.run(
            ["go", "test", "-c", "-cover", f"-coverpkg={mod}/...", "-o", str(binary), "."],
            cwd=d, capture_output=True, text=True,
        )  # fmt: skip
        if built.returncode != 0:
            print(f"build failed: {d}\n{built.stderr[-2000:]}", file=sys.stderr)
            continue
        names = subprocess.run(
            [str(binary), "-test.list", "."], cwd=d, capture_output=True, text=True
        ).stdout.split()
        files = test_files(d)
        for n in names:
            if n.startswith(("Test", "Fuzz")) and n in files:
                jobs.append((d, binary, n, files[n], work / f"p{i}-{n}.cov"))
    print(f"{len(pkgs)} packages, {len(jobs)} tests", file=sys.stderr)

    def run(job):
        d, binary, name, file, prof = job
        argv = [str(binary), "-test.run", f"^{name}$", f"-test.coverprofile={prof}"]
        if a.short:
            argv.append("-test.short")
        try:
            p = subprocess.run(argv, cwd=d, capture_output=True, timeout=a.timeout)
            status = p.returncode
        except subprocess.TimeoutExpired:
            status = "timeout"
        ran: set[str] = set()
        if prof.exists():
            for line in prof.read_text().splitlines():
                m = BLOCK.match(line)
                if not m or int(m.group(7)) == 0:
                    continue
                rel = m.group(1).removeprefix(mod + "/")
                fn = enclosing(funcs.get(rel, []), int(m.group(2)))
                if fn:
                    ran.add(fn)
            prof.unlink()
        key = f"{os.path.relpath(d, repo)}/{file}::{name}".removeprefix("./")
        return key, status, sorted(ran)

    out = {"tests": {}, "status": {}}
    with ThreadPoolExecutor(max_workers=a.j) as pool:
        for key, status, ran in pool.map(run, jobs):
            out["tests"][key] = ran
            out["status"][key] = status
    a.out.write_text(json.dumps(out, indent=0) + "\n")
    bad = {k: v for k, v in out["status"].items() if v != 0}
    print(f"non-zero exits: {len(bad)} {bad}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
