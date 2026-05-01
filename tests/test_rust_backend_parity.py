"""Rust backend parity tests for Rust-owned parser paths."""

from __future__ import annotations

import os
import shutil
import sys
from pathlib import Path

import pytest

from dagayn.incremental import full_build, incremental_update

sys.path.insert(0, str(Path(__file__).parent.parent / "tools"))
from parity_export import export_db  # noqa: E402

from tests.conftest import PARITY_FIXTURE_DIR

RUST_OWNED_PARITY_FIXTURES = [
    "terraform_only",
    "markdown_only",
    "python_only",
    "mixed",
]


def _copy_fixture(source: Path, dest: Path) -> None:
    for item in source.iterdir():
        if item.name in (".git", ".dagayn"):
            continue
        target = dest / item.name
        if item.is_dir():
            shutil.copytree(item, target)
        else:
            shutil.copy2(item, target)
    (dest / ".git").mkdir()


@pytest.mark.parametrize("name", RUST_OWNED_PARITY_FIXTURES)
def test_rust_backend_matches_python_parity_snapshots(name, tmp_path_factory, monkeypatch):
    """Rust-owned parser paths must preserve the Python graph contract."""
    try:
        from dagayn._core import GraphStore
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")

    monkeypatch.setenv("DAGAYN_BACKEND", "rust")
    source = PARITY_FIXTURE_DIR / name
    repo = tmp_path_factory.mktemp(f"rustparity_{name}")
    _copy_fixture(source, repo)

    db_path = repo / ".dagayn" / "graph.db"
    store = GraphStore(db_path)
    try:
        full_build(repo, store)
    finally:
        store.close()

    actual = export_db(db_path)
    expected = (PARITY_FIXTURE_DIR / "__snapshots__" / f"{name}.json").read_text(encoding="utf-8")
    assert actual == expected


def test_rust_backend_incremental_touch_updates_mtime_without_reparse(
    tmp_path_factory, monkeypatch
):
    """Rust-owned unchanged content should not cross back into Python parsing."""
    try:
        from dagayn._core import GraphStore
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")

    monkeypatch.setenv("DAGAYN_BACKEND", "rust")
    source = PARITY_FIXTURE_DIR / "markdown_only"
    repo = tmp_path_factory.mktemp("rustparity_touch")
    _copy_fixture(source, repo)

    db_path = repo / ".dagayn" / "graph.db"
    store = GraphStore(db_path)
    try:
        full_build(repo, store)
        target = repo / "api.md"
        new_mtime_ns = int(target.stat().st_mtime_ns) + 2_000_000_000
        target.touch()
        # Force a deterministic mtime bump while keeping file content unchanged.
        os.utime(target, ns=(new_mtime_ns, new_mtime_ns))

        result = incremental_update(repo, store, changed_files=["api.md"])
        assert result["total_nodes"] == 0
        assert result["total_edges"] == 0
        assert result["errors"] == []
        assert store.get_file_meta_map()["api.md"][1] == new_mtime_ns
    finally:
        store.close()
