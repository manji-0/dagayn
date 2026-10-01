"""The SCIP overlay orchestrator: which indexers run, and how failures are reported."""

from __future__ import annotations

import json
import shlex
import sys
from pathlib import Path

import pytest

from dagayn.scip_overlay import plan_jobs, run_scip_overlay


class _Store:
    def __init__(self) -> None:
        self.calls: list[tuple[str, str, str]] = []

    def apply_scip_overlay_json(self, index_path: str, prefix: str, repo_root: str) -> str:
        self.calls.append((Path(index_path).name, prefix, repo_root))
        return json.dumps({"documents": 1, "calls": 2, "confirmed": 2})


def _repo(tmp_path: Path) -> Path:
    (tmp_path / "Cargo.toml").write_text("[workspace]\n")
    web = tmp_path / "web"
    (web / "node_modules").mkdir(parents=True)
    (web / "tsconfig.json").write_text("{}")
    (web / "tsconfig.test.json").write_text("{}")
    # A tsconfig without node_modules is not indexed.
    (tmp_path / "docs").mkdir()
    (tmp_path / "docs" / "tsconfig.json").write_text("{}")
    return tmp_path


def _writer(tmp_path: Path) -> str:
    """A stub indexer: writes a file at the path after ``--output``."""
    script = tmp_path / "stub_indexer.py"
    script.write_text(
        "import sys\nargs = sys.argv[1:]\nopen(args[args.index('--output') + 1], 'wb').write(b'')\n"
    )
    return f"{shlex.quote(sys.executable)} {shlex.quote(str(script))}"


def test_jobs_follow_the_repository_and_the_overrides(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _repo(tmp_path)
    monkeypatch.setenv("DAGAYN_SCIP_RUST", "ra")
    monkeypatch.setenv("DAGAYN_SCIP_TYPESCRIPT", "scip-ts")
    jobs, skipped = plan_jobs(repo, tmp_path / "out")
    assert skipped == []
    assert [(job.language, job.prefix, job.cwd) for job in jobs] == [
        ("rust", "", repo),
        ("typescript", "web/", repo / "web"),
    ]
    assert jobs[0].command[:2] == ["ra", "scip"]
    assert jobs[1].command[:3] == ["scip-ts", "index", "--output"]
    assert jobs[1].command[-2:] == ["tsconfig.json", "tsconfig.test.json"]


def test_a_missing_indexer_is_a_warning(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    (tmp_path / "Cargo.toml").write_text("[workspace]\n")
    monkeypatch.delenv("DAGAYN_SCIP_RUST", raising=False)
    monkeypatch.setattr("dagayn.scip_overlay.shutil.which", lambda _name: None)
    jobs, skipped = plan_jobs(tmp_path, tmp_path / "out")
    assert jobs == []
    assert skipped and "rust-analyzer not found" in skipped[0]


def test_indexes_are_applied_and_failures_reported(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _repo(tmp_path)
    monkeypatch.setenv("DAGAYN_SCIP_RUST", _writer(tmp_path))
    monkeypatch.setenv(
        "DAGAYN_SCIP_TYPESCRIPT", f"{shlex.quote(sys.executable)} -c 'raise SystemExit(3)'"
    )
    store = _Store()
    report = run_scip_overlay(store, repo, tmp_path / "out")
    assert store.calls == [("rust.scip", "", str(repo))]
    assert report.runs == [
        {"language": "rust", "prefix": "", "documents": 1, "calls": 2, "confirmed": 2}
    ]
    assert len(report.warnings) == 1
    assert "typescript" in report.warnings[0] and "exited 3" in report.warnings[0]


def test_a_store_without_the_overlay_is_a_warning(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    (tmp_path / "Cargo.toml").write_text("[workspace]\n")
    monkeypatch.setenv("DAGAYN_SCIP_RUST", _writer(tmp_path))
    report = run_scip_overlay(object(), tmp_path, tmp_path / "out")
    assert report.runs == []
    assert "index not applied" in report.warnings[0]
