"""Phase 2 post-processing parity on canonical fixtures."""

from __future__ import annotations

from pathlib import Path

import pytest

from tests.conftest import PARITY_FIXTURE_NAMES
from tests.fts_parity_queries import copy_graph_db
from tests.postprocess_parity import (
    PARITY_BOUNDARY_AGREEMENT_MIN,
    PARITY_RELATIVE_TOLERANCE,
    community_boundary_agreement,
    detect_communities_python,
    detect_communities_rust,
    relative_count_delta,
    trace_flows_python,
    trace_flows_rust,
)

pytestmark = pytest.mark.usefixtures("parity_fixture_dbs")


def _require_rust_graph_store():
    try:
        from dagayn._core import GraphStore as RustGraphStore
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")  # ty: ignore[too-many-positional-arguments]
    return RustGraphStore


@pytest.mark.parametrize("fixture_name", PARITY_FIXTURE_NAMES)
def test_community_count_within_two_percent(
    fixture_name: str,
    parity_fixture_dbs: dict[str, Path],
    tmp_path: Path,
) -> None:
    """Rust community detection stays within ±2% of Python on parity fixtures."""
    RustGraphStore = _require_rust_graph_store()
    from dagayn.graph import GraphStore as PythonGraphStore

    py_db = copy_graph_db(parity_fixture_dbs[fixture_name], tmp_path / f"{fixture_name}_py.db")
    rust_db = copy_graph_db(parity_fixture_dbs[fixture_name], tmp_path / f"{fixture_name}_rust.db")

    py_store = PythonGraphStore(py_db)
    rust_store = RustGraphStore(rust_db)
    try:
        py_communities = detect_communities_python(py_store)
        rust_communities = detect_communities_rust(rust_store)
    finally:
        py_store.close()
        rust_store.close()

    delta = relative_count_delta(len(py_communities), len(rust_communities))
    assert delta <= PARITY_RELATIVE_TOLERANCE, (
        f"community count drift for {fixture_name!r}: "
        f"python={len(py_communities)} rust={len(rust_communities)} "
        f"delta={delta:.4f} > {PARITY_RELATIVE_TOLERANCE}"
    )


@pytest.mark.parametrize("fixture_name", PARITY_FIXTURE_NAMES)
def test_community_boundaries_within_two_percent(
    fixture_name: str,
    parity_fixture_dbs: dict[str, Path],
    tmp_path: Path,
) -> None:
    """Rust community partitions agree with Python on at least 98% of node pairs."""
    RustGraphStore = _require_rust_graph_store()
    from dagayn.graph import GraphStore as PythonGraphStore

    py_db = copy_graph_db(parity_fixture_dbs[fixture_name], tmp_path / f"{fixture_name}_py.db")
    rust_db = copy_graph_db(parity_fixture_dbs[fixture_name], tmp_path / f"{fixture_name}_rust.db")

    py_store = PythonGraphStore(py_db)
    rust_store = RustGraphStore(rust_db)
    try:
        py_communities = detect_communities_python(py_store)
        rust_communities = detect_communities_rust(rust_store)
    finally:
        py_store.close()
        rust_store.close()

    agreement = community_boundary_agreement(py_communities, rust_communities)
    assert agreement >= PARITY_BOUNDARY_AGREEMENT_MIN, (
        f"community boundary drift for {fixture_name!r}: "
        f"agreement={agreement:.4f} < {PARITY_BOUNDARY_AGREEMENT_MIN}"
    )


@pytest.mark.parametrize("fixture_name", PARITY_FIXTURE_NAMES)
def test_flow_count_within_two_percent(
    fixture_name: str,
    parity_fixture_dbs: dict[str, Path],
    tmp_path: Path,
) -> None:
    """Rust flow tracing stays within ±2% of Python on parity fixtures."""
    RustGraphStore = _require_rust_graph_store()
    from dagayn.graph import GraphStore as PythonGraphStore

    py_db = copy_graph_db(parity_fixture_dbs[fixture_name], tmp_path / f"{fixture_name}_py.db")
    rust_db = copy_graph_db(parity_fixture_dbs[fixture_name], tmp_path / f"{fixture_name}_rust.db")

    py_store = PythonGraphStore(py_db)
    rust_store = RustGraphStore(rust_db)
    try:
        py_flows = trace_flows_python(py_store)
        rust_flows = trace_flows_rust(rust_store)
    finally:
        py_store.close()
        rust_store.close()

    delta = relative_count_delta(len(py_flows), len(rust_flows))
    assert delta <= PARITY_RELATIVE_TOLERANCE, (
        f"flow count drift for {fixture_name!r}: "
        f"python={len(py_flows)} rust={len(rust_flows)} "
        f"delta={delta:.4f} > {PARITY_RELATIVE_TOLERANCE}"
    )
