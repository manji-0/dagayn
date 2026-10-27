from __future__ import annotations

import json
import shutil
import sqlite3
import subprocess
import sys
from pathlib import Path

import pytest

from dagayn.child_env import child_env
from dagayn.graph import GraphStore
from dagayn.incremental_build import full_build
from dagayn.scip_overlay import ScipOverlayReport, run_scip_overlay

FIXTURES = (
    Path(__file__).resolve().parents[1] / "crates" / "dagayn-scip-python" / "tests" / "fixtures"
)
SHIM = [sys.executable, "-P", "-m", "dagayn.scip_python"]


def _venv(path: Path, *, pydantic: bool) -> Path:
    """A virtual environment without an interpreter: ty reads ``pyvenv.cfg``
    and the ``site-packages`` layout."""
    site_packages = path / "lib" / "python3.12" / "site-packages"
    site_packages.mkdir(parents=True)
    (path / "pyvenv.cfg").write_text(
        f"home = {path.parent / 'no-python' / 'bin'}\nversion_info = 3.12.4\n"
    )
    if pydantic:
        shutil.copytree(FIXTURES / "site-packages" / "pydantic", site_packages / "pydantic")
    return path


def _shop(tmp_path: Path, *, venv: bool) -> Path:
    repo = tmp_path / "shop"
    shutil.copytree(FIXTURES / "shop", repo)
    if venv:
        _venv(repo / ".venv", pydantic=True)
    (repo / ".git").mkdir()
    return repo


def _overlay(repo: Path) -> tuple[ScipOverlayReport, Path]:
    db_path = repo / ".dagayn" / "graph.db"
    store = GraphStore(db_path)
    try:
        full_build(repo, store)
        report = run_scip_overlay(store, repo, db_path.parent / "scip")
    finally:
        store.close()
    return report, db_path


def _main_calls(db_path: Path) -> dict[str, dict]:
    with sqlite3.connect(db_path) as db:
        rows = db.execute(
            "select target_qualified, extra from edges "
            "where kind = 'CALLS' and file_path = 'shop/main.py'"
        ).fetchall()
    return {target: json.loads(extra) if extra else {} for target, extra in rows}


def test_calls_into_a_package_of_the_project_venv_settle_through_the_real_core(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.delenv("VIRTUAL_ENV", raising=False)
    report, db_path = _overlay(_shop(tmp_path, venv=True))
    assert report.warnings == []
    assert report.hints == []
    assert [(run["language"], run["documents"]) for run in report.runs] == [("python", 3)]
    calls = _main_calls(db_path)
    assert calls["shop/cart.py::total"]["resolved_by"] == "scip"
    assert calls["shop/cart.py::Cart.add"]["resolved_by"] == "scip"
    model_dump = calls["pydantic"]
    assert model_dump["resolved_by"] == "scip"
    assert model_dump["external_symbol"] == "BaseModel.model_dump"


def test_without_a_venv_the_project_is_still_indexed_with_a_hint(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.delenv("VIRTUAL_ENV", raising=False)
    report, db_path = _overlay(_shop(tmp_path, venv=False))
    assert report.warnings == []
    assert [hint.split(":")[0] for hint in report.hints] == ["python"]
    assert "virtual environment" in report.hints[0]
    assert [run["language"] for run in report.runs] == ["python"]
    calls = _main_calls(db_path)
    assert calls["shop/cart.py::total"]["resolved_by"] == "scip"


def test_the_shim_reports_an_environment_it_cannot_use(tmp_path: Path) -> None:
    repo = _shop(tmp_path, venv=False)
    output = tmp_path / "index.scip"
    completed = subprocess.run(
        [*SHIM, "index", "--output", str(output), "--python", str(tmp_path / "no-venv")],
        cwd=repo,
        env=child_env(),
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 1
    assert completed.stderr.startswith("error: ")
    assert not output.exists()


def test_the_shim_resolves_imports_in_the_project_not_in_dagayn(tmp_path: Path) -> None:
    # child_env puts dagayn's own directory on PYTHONPATH for the child to
    # import dagayn; ty reads PYTHONPATH too, and must not see it.
    repo = _shop(tmp_path, venv=False)
    (repo / "dagayn").mkdir()
    (repo / "dagayn" / "__init__.py").write_text("raise SystemExit('decoy dagayn imported')\n")
    (repo / "shop" / "uses_dagayn.py").write_text(
        "from dagayn.child_env import child_env\n\nchild_env()\n"
    )
    venv = _venv(tmp_path / "venv", pydantic=False)
    output = tmp_path / "index.scip"
    completed = subprocess.run(
        [*SHIM, "index", "--output", str(output), "--python", str(venv)],
        cwd=repo,
        env=child_env(),
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr
    report = json.loads(completed.stdout)
    assert report["environment"] == f"{venv.resolve()} (`--python` argument)"
    assert output.is_file()
    assert b"child_env" not in output.read_bytes()
