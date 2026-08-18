"""Phase 2 FTS parity: Python and Rust backends return identical top-20 FTS hits."""

from __future__ import annotations

from pathlib import Path

import pytest

from tests.conftest import PARITY_FIXTURE_NAMES
from tests.fts_parity_queries import (
    FTS_TOP_N,
    PARITY_FTS_QUERIES,
    copy_graph_db,
    fts_top_snapshot_python,
    fts_top_snapshot_rust,
    rebuild_fts_with_python,
    rebuild_fts_with_rust,
)

pytestmark = pytest.mark.usefixtures("parity_fixture_dbs")


def _require_rust_graph_store():
    try:
        from dagayn._core import GraphStore as RustGraphStore
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")  # ty: ignore[too-many-positional-arguments]
    return RustGraphStore


@pytest.mark.parametrize("fixture_name", PARITY_FIXTURE_NAMES)
def test_fts_rebuild_row_count_matches_python(
    fixture_name: str,
    parity_fixture_dbs: dict[str, Path],
    tmp_path: Path,
) -> None:
    """Rust FTS rebuild indexes the same number of rows as the Python path."""
    RustGraphStore = _require_rust_graph_store()
    from dagayn.graph import GraphStore as PythonGraphStore

    py_db = copy_graph_db(parity_fixture_dbs[fixture_name], tmp_path / f"{fixture_name}_py.db")
    rust_db = copy_graph_db(parity_fixture_dbs[fixture_name], tmp_path / f"{fixture_name}_rust.db")

    py_store = PythonGraphStore(py_db)
    rust_store = RustGraphStore(rust_db)
    try:
        py_count = rebuild_fts_with_python(py_store)
        rust_count = rebuild_fts_with_rust(rust_store)
    finally:
        py_store.close()
        rust_store.close()

    assert py_count > 0
    assert rust_count == py_count


@pytest.mark.parametrize(
    ("fixture_name", "query"),
    [
        (fixture_name, query)
        for fixture_name in PARITY_FIXTURE_NAMES
        for query in PARITY_FTS_QUERIES[fixture_name]
    ],
    ids=[
        f"{fixture_name}:{query}"
        for fixture_name in PARITY_FIXTURE_NAMES
        for query in PARITY_FTS_QUERIES[fixture_name]
    ],
)
def test_fts_top20_matches_between_python_and_rust(
    fixture_name: str,
    query: str,
    parity_fixture_dbs: dict[str, Path],
    tmp_path: Path,
) -> None:
    """Each curated query returns the same top-20 FTS ordering on both backends."""
    RustGraphStore = _require_rust_graph_store()
    from dagayn.graph import GraphStore as PythonGraphStore

    py_db = copy_graph_db(
        parity_fixture_dbs[fixture_name],
        tmp_path / f"{fixture_name}_{query.replace('/', '_')}_py.db",
    )
    rust_db = copy_graph_db(
        parity_fixture_dbs[fixture_name],
        tmp_path / f"{fixture_name}_{query.replace('/', '_')}_rust.db",
    )

    py_store = PythonGraphStore(py_db)
    rust_store = RustGraphStore(rust_db)
    try:
        rebuild_fts_with_python(py_store)
        rebuild_fts_with_rust(rust_store)

        py_snapshot = fts_top_snapshot_python(py_store, query)
        rust_snapshot = fts_top_snapshot_rust(rust_store, query)
    finally:
        py_store.close()
        rust_store.close()

    assert py_snapshot.hits, f"expected FTS hits for {fixture_name!r} query {query!r}"
    assert len(py_snapshot.hits) <= FTS_TOP_N
    assert py_snapshot == rust_snapshot, (
        f"FTS top-{FTS_TOP_N} mismatch for {fixture_name!r} query {query!r}\n"
        f"  python: {py_snapshot.as_dict()}\n"
        f"  rust:   {rust_snapshot.as_dict()}"
    )
