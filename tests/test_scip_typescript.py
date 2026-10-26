from __future__ import annotations

import json
import shutil
import sqlite3
import subprocess
import sys
from pathlib import Path

from dagayn.child_env import child_env
from dagayn.graph import GraphStore
from dagayn.incremental_build import full_build
from dagayn.scip_overlay import ScipOverlayReport, run_scip_overlay

FIXTURE = (
    Path(__file__).resolve().parents[1]
    / "crates"
    / "dagayn-scip-typescript"
    / "tests"
    / "fixtures"
    / "shop"
)
SHIM = [sys.executable, "-P", "-m", "dagayn.scip_typescript"]


def _shop(tmp_path: Path, *, node_modules: bool) -> Path:
    repo = tmp_path / "shop"
    shutil.copytree(FIXTURE, repo)
    vendored = repo / "vendor_modules"
    if node_modules:
        vendored.rename(repo / "node_modules")
    else:
        shutil.rmtree(vendored)
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
            "where kind = 'CALLS' and file_path = 'src/main.ts'"
        ).fetchall()
    return {target: json.loads(extra) if extra else {} for target, extra in rows}


def test_the_bundled_indexer_settles_calls_through_the_real_core(tmp_path: Path) -> None:
    report, db_path = _overlay(_shop(tmp_path, node_modules=True))
    assert report.warnings == []
    assert report.hints == []
    assert [(run["language"], run["documents"]) for run in report.runs] == [("typescript", 2)]
    calls = _main_calls(db_path)
    assert calls["src/cart.ts::total"] == {"resolved_by": "scip"}
    assert calls["src/cart.ts::Cart.add"] == {"resolved_by": "scip"}
    assert "resolved_by" not in calls["src/cart.ts::discount"]
    assert calls["left-pad::default"]["external_package"] == "left-pad"


def test_without_node_modules_the_project_is_still_indexed_with_a_hint(tmp_path: Path) -> None:
    report, db_path = _overlay(_shop(tmp_path, node_modules=False))
    assert report.warnings == []
    assert [hint.split(":")[0] for hint in report.hints] == ["typescript"]
    assert "node_modules" in report.hints[0]
    assert [run["language"] for run in report.runs] == ["typescript"]
    calls = _main_calls(db_path)
    assert calls["src/cart.ts::total"] == {"resolved_by": "scip"}
    assert "resolved_by" not in calls["left-pad::default"]


def test_the_shim_reports_a_tsconfig_it_cannot_read(tmp_path: Path) -> None:
    repo = _shop(tmp_path, node_modules=True)
    output = tmp_path / "index.scip"
    completed = subprocess.run(
        [*SHIM, "index", "--output", str(output), "tsconfig.missing.json"],
        cwd=repo,
        env=child_env(),
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 1
    assert completed.stderr.startswith("error: ")
    assert "tsconfig.missing.json" in completed.stderr
    assert not output.exists()


def test_the_shim_ignores_a_dagayn_directory_in_the_project(tmp_path: Path) -> None:
    repo = _shop(tmp_path, node_modules=True)
    (repo / "dagayn").mkdir()
    (repo / "dagayn" / "__init__.py").write_text("raise SystemExit('decoy dagayn imported')\n")
    output = tmp_path / "index.scip"
    completed = subprocess.run(
        [*SHIM, "index", "--output", str(output), "tsconfig.json"],
        cwd=repo,
        env=child_env(),
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr
    assert json.loads(completed.stdout)["documents"] == 2
    assert output.is_file()
