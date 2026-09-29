"""End-to-end: Python -> Rust bridges through PyO3 (maturin) and ctypes."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest

from dagayn.graph import GraphStore
from dagayn.incremental import full_build
from dagayn.postprocessing import run_post_processing
from dagayn.tools.query import get_impact_radius
from tests.store_sql import store_conn

KAHAN = """
fn kahan(xs: &[f64]) -> f64 {
    xs.iter().sum()
}
"""


def _write(root: Path, files: dict[str, str]) -> None:
    for rel, text in files.items():
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    subprocess.run(["git", "init", "-q"], cwd=root, check=True)


def _native_bridges(store: GraphStore) -> set[tuple[str, str, str]]:
    rows = (
        store_conn(store)
        .execute(
            "SELECT source_qualified, target_qualified, extra FROM edges "
            "WHERE kind = 'CROSS_ARTIFACT' "
            "AND json_extract(extra, '$.extractor') = 'native_bindings'"
        )
        .fetchall()
    )
    return {
        (
            row["source_qualified"],
            row["target_qualified"],
            json.loads(row["extra"])["relationship_role"],
        )
        for row in rows
    }


@pytest.fixture
def pyo3_repo(tmp_path: Path) -> Path:
    _write(
        tmp_path,
        {
            "pyproject.toml": (
                '[build-system]\nrequires = ["maturin>=1.9"]\nbuild-backend = "maturin"\n\n'
                '[project]\nname = "fastsum"\n\n'
                '[tool.maturin]\nmanifest-path = "rust/Cargo.toml"\nmodule-name = "fastsum._core"\n'
            ),
            "rust/Cargo.toml": (
                '[package]\nname = "fastsum-core"\nversion = "0.1.0"\n\n'
                '[lib]\nname = "_core"\ncrate-type = ["cdylib"]\n'
            ),
            "rust/src/lib.rs": (
                "use pyo3::prelude::*;\n\n"
                "#[pyfunction]\nfn fast_sum(xs: Vec<f64>) -> f64 {\n    kahan(&xs)\n}\n" + KAHAN
            ),
            "fastsum/__init__.py": (
                "from fastsum._core import fast_sum\n\n\n"
                "def total(values):\n    return fast_sum(list(values))\n"
            ),
            "fastsum/report.py": (
                "from fastsum import total\n\n\n"
                "def monthly_report(rows):\n    return total(r for r in rows)\n"
            ),
        },
    )
    return tmp_path


@pytest.fixture
def ctypes_repo(tmp_path: Path) -> Path:
    _write(
        tmp_path,
        {
            "native/Cargo.toml": (
                '[package]\nname = "fastsum"\nversion = "0.1.0"\n\n[lib]\ncrate-type = ["cdylib"]\n'
            ),
            "native/src/lib.rs": (
                "#[unsafe(no_mangle)]\n"
                'pub extern "C" fn fast_sum(ptr: *const f64, len: usize) -> f64 {\n'
                "    let xs = unsafe { std::slice::from_raw_parts(ptr, len) };\n    kahan(xs)\n}\n"
                + KAHAN
            ),
            "app/native.py": (
                "from ctypes import CDLL\n\n\n"
                'def load():\n    return CDLL("native/target/release/libfastsum.dylib")\n\n\n'
                "def total(values):\n"
                "    lib = load()\n"
                "    return lib.fast_sum(values, len(values))\n"
            ),
        },
    )
    return tmp_path


def _build(repo: Path) -> GraphStore:
    store = GraphStore(repo / ".dagayn" / "graph.db")
    full_build(repo, store)
    run_post_processing(store)
    return store


def test_pyo3_import_and_call_reach_the_rust_function(pyo3_repo: Path) -> None:
    store = _build(pyo3_repo)
    try:
        assert _native_bridges(store) == {
            ("fastsum/__init__.py", "rust/src/lib.rs", "loads_native_module"),
            ("fastsum/__init__.py::total", "rust/src/lib.rs::fast_sum", "calls_native_function"),
        }
        impact = get_impact_radius(
            changed_files=["rust/src/lib.rs"], repo_root=str(pyo3_repo), max_depth=2
        )
        assert "fastsum/report.py" in impact["impacted_files"]
    finally:
        store.close()


def test_ctypes_library_and_symbol_reach_the_rust_crate(ctypes_repo: Path) -> None:
    store = _build(ctypes_repo)
    try:
        assert _native_bridges(store) == {
            ("app/native.py::load", "native/src/lib.rs", "loads_shared_library"),
            ("app/native.py::total", "native/src/lib.rs::fast_sum", "calls_native_function"),
        }
        impact = get_impact_radius(
            changed_files=["native/src/lib.rs"], repo_root=str(ctypes_repo), max_depth=2
        )
        assert "app/native.py" in impact["impacted_files"]
    finally:
        store.close()
