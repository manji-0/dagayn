"""Shared pytest fixtures for the dagayn test suite."""

from __future__ import annotations

import shutil
from pathlib import Path

import pytest

from dagayn.graph import GraphStore
from dagayn.incremental import full_build

PARITY_FIXTURE_DIR = Path(__file__).parent / "fixtures" / "parity"
PARITY_FIXTURE_NAMES = [
    "python_only",
    "terraform_only",
    "markdown_only",
    "notebook",
    "mixed",
]


def build_parity_fixture(source: Path, dest: Path) -> Path:
    """Copy fixture source files to dest, stub a .git dir, run full_build.

    Returns the path to the resulting graph.db.
    """
    for item in source.iterdir():
        if item.name in (".git", ".dagayn"):
            continue
        target = dest / item.name
        if item.is_dir():
            shutil.copytree(item, target)
        else:
            shutil.copy2(item, target)

    # A non-empty .git dir signals to collect_all_files that this is a git
    # repo root, causing it to fall back to rglob when git ls-files returns
    # nothing (no commits yet).  The file set is still deterministic because
    # dest contains only our fixture files.
    (dest / ".git").mkdir()

    db_path = dest / ".dagayn" / "graph.db"
    store = GraphStore(db_path)
    full_build(dest, store)
    store.close()
    return db_path


@pytest.fixture(scope="session")
def parity_fixture_dbs(tmp_path_factory):
    """Build all parity fixtures once per session; return mapping name → db_path."""
    dbs: dict[str, Path] = {}
    for name in PARITY_FIXTURE_NAMES:
        source = PARITY_FIXTURE_DIR / name
        dest = tmp_path_factory.mktemp(f"parity_{name}")
        dbs[name] = build_parity_fixture(source, dest)
    return dbs
