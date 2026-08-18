"""FTS query sets and helpers for Python/Rust post-processing parity."""

from __future__ import annotations

import json
import shutil
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Any

from dagayn.graph import GraphStore as PythonGraphStore
from dagayn.search import rebuild_fts_index

if TYPE_CHECKING:
    from dagayn._core import GraphStore as RustGraphStore

# Curated queries per parity fixture: each must return at least one FTS hit on
# the Python-built graph so top-20 ordering can be compared across backends.
PARITY_FTS_QUERIES: dict[str, list[str]] = {
    "python_only": [
        "create_user",
        "User",
        "Address",
        "get_email",
        "run",
        "services.create_user",
        "models.User",
        "main.py::run",
    ],
    "terraform_only": [
        "aws_instance",
        "web",
        "instance_type",
        "region",
        "instance_id",
        "main.tf",
    ],
    "markdown_only": [
        "status",
        "build",
        "API",
        "Installation",
        "guide",
    ],
    "notebook": [
        "load_data",
        "analysis",
        "notebook",
    ],
    "mixed": [
        "build_graph",
        "run_analysis",
        "graph_store",
        "aws_s3_bucket",
        "environment",
        "README",
    ],
}

FTS_TOP_N = 20
_SCORE_PRECISION = 6


@dataclass(frozen=True)
class FtsTopSnapshot:
    match_mode: str
    hits: tuple[tuple[str, float], ...]

    def as_dict(self) -> dict[str, Any]:
        return {
            "match_mode": self.match_mode,
            "hits": list(self.hits),
        }


def copy_graph_db(source: Path, dest: Path) -> Path:
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, dest)
    return dest


def _qualified_hits(
    store: PythonGraphStore | RustGraphStore,
    hits: list[tuple[int, float]],
) -> tuple[tuple[str, float], ...]:
    if not hits:
        return ()
    node_ids = [node_id for node_id, _ in hits]
    by_id = store.get_nodes_by_ids(node_ids)
    out: list[tuple[str, float]] = []
    for node_id, score in hits:
        node = by_id.get(node_id)
        if node is None:
            raise AssertionError(f"missing node id {node_id} for FTS hit")
        out.append((node.qualified_name, round(score, _SCORE_PRECISION)))
    return tuple(out)


def fts_top_snapshot_python(store: PythonGraphStore, query: str) -> FtsTopSnapshot:
    result = store.fts_query(query, limit=FTS_TOP_N)
    return FtsTopSnapshot(
        match_mode=result.match_mode,
        hits=_qualified_hits(store, result.hits),
    )


def fts_top_snapshot_rust(store: RustGraphStore, query: str) -> FtsTopSnapshot:
    payload = json.loads(store.fts_query_json(query, FTS_TOP_N))
    hits = [(int(node_id), float(score)) for node_id, score in payload["hits"]]
    return FtsTopSnapshot(
        match_mode=str(payload["match_mode"]),
        hits=_qualified_hits(store, hits),
    )


def rebuild_fts_with_python(store: PythonGraphStore) -> int:
    return rebuild_fts_index(store)


def rebuild_fts_with_rust(store: RustGraphStore) -> int:
    return int(store.rebuild_fts_index())
