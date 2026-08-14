"""Tests for tools/parity_export.py — Phase 0 of the Rust core migration.

Two invariants are verified:

1. Build determinism: two independent full_build runs on the same fixture
   produce byte-identical canonical JSON exports.

2. Snapshot stability: the export of a freshly built fixture matches the
   committed baseline in tests/fixtures/parity/__snapshots__/.
   Snapshots are generated once and must not drift between Python releases.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent / "tools"))
from parity_export import export_db  # noqa: E402

from tests.conftest import PARITY_FIXTURE_DIR, PARITY_FIXTURE_NAMES, build_parity_fixture

SNAPSHOT_DIR = PARITY_FIXTURE_DIR / "__snapshots__"


@pytest.mark.parametrize("name", PARITY_FIXTURE_NAMES)
def test_build_is_deterministic(name, tmp_path_factory):
    """Two independent full_build runs produce byte-identical exports."""
    source = PARITY_FIXTURE_DIR / name
    db1 = build_parity_fixture(source, tmp_path_factory.mktemp(f"det1_{name}"))
    db2 = build_parity_fixture(source, tmp_path_factory.mktemp(f"det2_{name}"))
    assert export_db(db1) == export_db(db2), f"Build is not deterministic for fixture '{name}'"


@pytest.mark.parametrize("name", PARITY_FIXTURE_NAMES)
def test_export_matches_snapshot(name, parity_fixture_dbs):
    """Export of a freshly built fixture matches the committed baseline."""
    snapshot_path = SNAPSHOT_DIR / f"{name}.json"
    if not snapshot_path.exists():
        msg = (
            f"Snapshot not yet generated. Run:\n"
            f"  dagayn build --repo-dir tests/fixtures/parity/{name}\n"
            f"  uv run python tools/parity_export.py tests/fixtures/parity/{name} "
            f"--out tests/fixtures/parity/__snapshots__/{name}.json"
        )
        pytest.skip(reason=msg)

    actual = export_db(parity_fixture_dbs[name])
    expected = snapshot_path.read_text(encoding="utf-8")
    assert actual == expected, (
        f"Snapshot mismatch for '{name}'. Regenerate with:\n"
        f"  uv run python tools/parity_export.py tests/fixtures/parity/{name} "
        f"--out tests/fixtures/parity/__snapshots__/{name}.json"
    )
