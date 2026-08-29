"""Native GraphStore Japanese FTS quality gates on the mixed search fixture."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from dagayn.parser import CodeParser

pytest.importorskip("dagayn._core")

FIXTURE = Path(__file__).resolve().parent / "fixtures" / "japanese_search"
QUERIES = FIXTURE / "queries.json"
SKIP_FILES = {"README.md", "queries.json"}


def _rust_store(tmp_path: Path):
    from dagayn._core import GraphStore as RustGraphStore

    return RustGraphStore(tmp_path / "graph.db")


def _index_fixture(store, root: Path) -> int:
    parser = CodeParser()
    store.set_metadata("repo_root", str(root))
    for path in sorted(root.rglob("*")):
        if not path.is_file() or path.name in SKIP_FILES:
            continue
        rel = path.relative_to(root).as_posix()
        nodes, edges = parser.parse_bytes(Path(rel), path.read_bytes())
        if not nodes:
            continue
        store.store_file_nodes_edges(rel, nodes, edges)
    store.commit()
    return store.rebuild_fts_index()


def _hits(store, query: str) -> tuple[list[tuple[str, str]], str]:
    result = store.fts_query(query, 10)
    ids = [node_id for node_id, _ in result.hits]
    by_id = store.get_nodes_by_ids(ids)
    records = [
        (by_id[node_id].name, by_id[node_id].file_path) for node_id in ids if node_id in by_id
    ]
    return records, result.match_mode


def _gate_matches(records: list[tuple[str, str]], gate: dict) -> bool:
    k = int(gate["k"])
    file_contains = gate.get("file_contains")
    name_any = gate.get("name_any")
    for name, file_path in records[:k]:
        file_ok = file_contains is None or file_contains in file_path
        name_ok = name_any is None or name in name_any
        if file_ok and name_ok:
            return True
    return False


def test_native_japanese_fts_quality_gates(tmp_path):
    spec = json.loads(QUERIES.read_text(encoding="utf-8"))
    store = _rust_store(tmp_path)
    try:
        indexed = _index_fixture(store, FIXTURE)
        assert indexed >= spec["min_indexed_nodes"]
        assert store.get_metadata("fts_segmenter") == "lindera"

        for gate in spec["gates"]:
            records, mode = _hits(store, gate["query"])
            assert _gate_matches(records, gate), (gate["query"], mode, records)
            if gate.get("match_mode"):
                assert mode == gate["match_mode"], (gate["query"], mode, records)

        hits, _ = _hits(store, "ユーザー取得")
        assert hits and hits[0][0] == "ユーザー取得"
    finally:
        store.close()
