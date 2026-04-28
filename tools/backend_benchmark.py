"""Benchmark Python and Rust dagayn graph backends.

The benchmark copies the current repository to a temporary directory for each
end-to-end run so graph databases do not reuse state. Writer-only runs parse the
copy once per backend and time only ``GraphStore.store_file_batch``.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from statistics import mean
from typing import Iterable

DEFAULT_IGNORES = {
    ".git",
    ".dagayn",
    ".venv",
    "target",
    "__pycache__",
    ".pytest_cache",
    ".ruff_cache",
    ".serena",
    ".hatch-vendor-grammars",
    "node_modules",
    "dagayn-vscode/node_modules",
}


def _copy_repo(source: Path, dest: Path) -> None:
    def ignore(dir_path: str, names: list[str]) -> set[str]:
        root = Path(dir_path)
        ignored: set[str] = set()
        for name in names:
            path = root / name
            try:
                rel = str(path.relative_to(source))
            except ValueError:
                rel = name
            if name in DEFAULT_IGNORES or rel in DEFAULT_IGNORES:
                ignored.add(name)
            elif name.endswith((".db", ".sqlite", ".db-wal", ".db-shm", ".pyc")):
                ignored.add(name)
        return ignored

    shutil.copytree(source, dest, ignore=ignore)
    (dest / ".git").mkdir(exist_ok=True)


def _run_python(code: str, *, backend: str, cwd: Path) -> dict:
    env = os.environ.copy()
    env["DAGAYN_BACKEND"] = backend
    env["PYTHONWARNINGS"] = "ignore"
    proc = subprocess.run(
        [sys.executable, "-c", code],
        cwd=cwd,
        env=env,
        text=True,
        capture_output=True,
        check=True,
    )
    return json.loads(proc.stdout.strip().splitlines()[-1])


def run_e2e(source: Path, postprocess: str, repeats: int) -> list[dict]:
    results: list[dict] = []
    for backend in ("python", "rust"):
        for iteration in range(1, repeats + 1):
            with tempfile.TemporaryDirectory(prefix="dagayn-backend-bench-") as tmp:
                repo = Path(tmp) / "repo"
                _copy_repo(source, repo)
                code = f"""
import json
import time
from dagayn.tools.build import build_or_update_graph

start = time.perf_counter()
result = build_or_update_graph(
    full_rebuild=True,
    repo_root={str(repo)!r},
    postprocess={postprocess!r},
)
elapsed = time.perf_counter() - start
print(json.dumps({{
    "mode": "e2e",
    "backend": {backend!r},
    "postprocess": {postprocess!r},
    "iteration": {iteration},
    "seconds": elapsed,
    "files": result.get("files_parsed"),
    "nodes": result.get("total_nodes"),
    "edges": result.get("total_edges"),
    "fts": result.get("fts_indexed"),
    "flows": result.get("flows_detected"),
    "communities": result.get("communities_detected"),
}}, sort_keys=True))
"""
                record = _run_python(code, backend=backend, cwd=source)
                results.append(record)
                print(json.dumps(record, sort_keys=True), flush=True)
    return results


def run_writer(source: Path, repeats: int) -> list[dict]:
    results: list[dict] = []
    with tempfile.TemporaryDirectory(prefix="dagayn-writer-bench-") as tmp:
        repo = Path(tmp) / "repo"
        _copy_repo(source, repo)
        for backend in ("python", "rust"):
            code = f"""
import hashlib
import json
import tempfile
import time
from pathlib import Path

from dagayn.graph import GraphStore
from dagayn.incremental import (
    _relativize_parsed_entities,
    _serialize_store_batch,
    collect_all_files,
)
from dagayn.parser import CodeParser

repo = Path({str(repo)!r}).resolve()
parser = CodeParser()
batch = []
for rel_path in collect_all_files(repo):
    path = repo / rel_path
    raw = path.read_bytes()
    nodes, edges = parser.parse_bytes(path, raw)
    nodes, edges = _relativize_parsed_entities(nodes, edges, repo)
    batch.append((rel_path, nodes, edges, hashlib.sha256(raw).hexdigest()))

runs = []
for _ in range({repeats}):
    db = Path(tempfile.mkdtemp(prefix="dagayn-writer-db-")) / "graph.db"
    store = GraphStore(db)
    start = time.perf_counter()
    if hasattr(store, "store_file_batch_json"):
        store.store_file_batch_json(_serialize_store_batch(batch))
    else:
        store.store_file_batch(batch)
    store.commit()
    runs.append(time.perf_counter() - start)
    store.close()

print(json.dumps({{
    "mode": "writer",
    "backend": {backend!r},
    "seconds": runs,
    "avg_seconds": sum(runs) / len(runs),
    "best_seconds": min(runs),
    "files": len(batch),
    "nodes": sum(len(item[1]) for item in batch),
    "edges": sum(len(item[2]) for item in batch),
}}, sort_keys=True))
"""
            record = _run_python(code, backend=backend, cwd=source)
            results.append(record)
            print(json.dumps(record, sort_keys=True), flush=True)
    return results


def summarize(records: Iterable[dict]) -> list[dict]:
    out: list[dict] = []
    groups: dict[tuple[str, str, str], list[float]] = {}
    metadata: dict[tuple[str, str, str], dict] = {}
    for record in records:
        mode = record["mode"]
        backend = record["backend"]
        postprocess = record.get("postprocess", "")
        key = (mode, backend, postprocess)
        values = record["seconds"] if isinstance(record["seconds"], list) else [record["seconds"]]
        groups.setdefault(key, []).extend(values)
        metadata[key] = record
    for key, values in sorted(groups.items()):
        mode, backend, postprocess = key
        meta = metadata[key]
        out.append(
            {
                "mode": mode,
                "backend": backend,
                "postprocess": postprocess or None,
                "runs": len(values),
                "avg_seconds": round(mean(values), 4),
                "best_seconds": round(min(values), 4),
                "files": meta.get("files"),
                "nodes": meta.get("nodes"),
                "edges": meta.get("edges"),
            }
        )
    return out


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--mode", choices=("e2e", "writer", "all"), default="all")
    parser.add_argument("--postprocess", choices=("none", "minimal", "full"), default="none")
    parser.add_argument("--repeats", type=int, default=3)
    args = parser.parse_args()

    source = args.repo.resolve()
    records: list[dict] = []
    if args.mode in ("e2e", "all"):
        records.extend(run_e2e(source, args.postprocess, args.repeats))
    if args.mode in ("writer", "all"):
        records.extend(run_writer(source, args.repeats))

    print("SUMMARY")
    for row in summarize(records):
        print(json.dumps(row, sort_keys=True))


if __name__ == "__main__":
    main()
