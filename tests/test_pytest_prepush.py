"""Pre-push pytest should target tests related to the files being pushed."""

from __future__ import annotations

import importlib.util
from pathlib import Path

_REPO = Path(__file__).resolve().parents[1]
_SPEC = importlib.util.spec_from_file_location(
    "pytest_prepush", _REPO / "scripts" / "pytest_prepush.py"
)
assert _SPEC is not None and _SPEC.loader is not None
_prepush = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(_prepush)
select_test_files = _prepush.select_test_files


def test_maps_write_lock_via_import(tmp_path: Path) -> None:
    tests = tmp_path / "tests"
    tests.mkdir()
    (tests / "test_concurrency.py").write_text(
        "from dagayn.write_lock import graph_read_lock\n",
        encoding="utf-8",
    )
    (tests / "test_unrelated.py").write_text(
        "from dagayn.parser import NodeInfo\n",
        encoding="utf-8",
    )
    selected = select_test_files(
        ["dagayn/write_lock.py"],
        tests_dir=tests,
        repo_root=tmp_path,
    )
    names = {path.name for path in selected}
    assert names == {"test_concurrency.py"}


def test_maps_cli_build_by_filename(tmp_path: Path) -> None:
    tests = tmp_path / "tests"
    tests.mkdir()
    (tests / "test_cli_build.py").write_text("# cli build tests\n", encoding="utf-8")
    (tests / "test_cli.py").write_text("# cli tests\n", encoding="utf-8")
    selected = select_test_files(
        ["dagayn/cli/commands/build.py"],
        tests_dir=tests,
        repo_root=tmp_path,
    )
    names = {path.name for path in selected}
    assert "test_cli_build.py" in names
    assert "test_cli.py" in names


def test_changed_test_file_is_included(tmp_path: Path) -> None:
    tests = tmp_path / "tests"
    tests.mkdir()
    (tests / "test_enrich.py").write_text("def test_ok():\n    assert True\n", encoding="utf-8")
    selected = select_test_files(
        ["tests/test_enrich.py", "docs/USAGE.md"],
        tests_dir=tests,
        repo_root=tmp_path,
    )
    assert [path.as_posix() for path in selected] == ["tests/test_enrich.py"]


def test_rust_change_runs_parity_smoke(tmp_path: Path) -> None:
    tests = tmp_path / "tests"
    tests.mkdir()
    (tests / "test_rust_backend_parity.py").write_text("# rust parity\n", encoding="utf-8")
    selected = select_test_files(
        ["crates/dagayn-graph/src/core.rs"],
        tests_dir=tests,
        repo_root=tmp_path,
    )
    assert any(path.name == "test_rust_backend_parity.py" for path in selected)


def test_generic_core_stem_does_not_match_everything(tmp_path: Path) -> None:
    tests = tmp_path / "tests"
    tests.mkdir()
    (tests / "test_graph.py").write_text("# graph store\n", encoding="utf-8")
    (tests / "test_coverage.py").write_text("coverage of core behavior\n", encoding="utf-8")
    selected = select_test_files(
        ["dagayn/graph/core.py"],
        tests_dir=tests,
        repo_root=tmp_path,
    )
    names = {path.name for path in selected}
    assert "test_graph.py" in names
    assert "test_coverage.py" not in names
