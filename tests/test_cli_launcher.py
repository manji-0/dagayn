"""The ``dagayn`` console script: Rust CLI in ``_core`` first, Python otherwise."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest

from dagayn import _cli_launcher, _core

LAUNCHER = [sys.executable, "-c", "from dagayn._cli_launcher import main; main()"]
_HINT_ENVS = ("CRG_REPO_ROOT", "CRG_DATA_DIR", "CURSOR_PROJECT_DIR", "CLAUDE_PROJECT_DIR")


def _env(**overrides: str) -> dict[str, str]:
    env = {key: value for key, value in os.environ.items() if key not in _HINT_ENVS}
    env.pop("WORKSPACE_FOLDER_PATHS", None)
    env.pop("DAGAYN_PYTHON_CLI", None)
    env.update(overrides)
    return env


def _project(tmp_path: Path) -> Path:
    repo = tmp_path / "repo"
    (repo / ".git").mkdir(parents=True)
    (repo / "app.py").write_text("def main():\n    return helper()\n\n\ndef helper():\n    pass\n")
    return repo


def _launch(*args: str, **env: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(  # nosec B603
        [*LAUNCHER, *args], capture_output=True, text=True, env=_env(**env), timeout=120
    )


class TestRouting:
    def test_rust_exit_status_is_the_process_status(self, monkeypatch) -> None:
        calls: list[list[str]] = []

        def run_cli(argv: list[str]) -> int:
            calls.append(argv)
            return 3

        monkeypatch.setattr(_core, "run_cli", run_cli)
        monkeypatch.setattr(sys, "argv", ["dagayn", "status", "--repo", "."])
        monkeypatch.delenv("DAGAYN_PYTHON_CLI", raising=False)

        with pytest.raises(SystemExit) as exit_info:
            _cli_launcher.main()

        assert exit_info.value.code == 3
        assert calls == [["dagayn", "status", "--repo", "."]]

    def test_fallback_runs_the_python_cli(self, monkeypatch) -> None:
        ran: list[bool] = []
        monkeypatch.setattr(_core, "run_cli", lambda argv: None)
        monkeypatch.setattr("dagayn.cli.main", lambda: ran.append(True), raising=False)
        monkeypatch.delenv("DAGAYN_PYTHON_CLI", raising=False)

        _cli_launcher.main()

        assert ran == [True]

    def test_opt_out_never_calls_rust(self, monkeypatch) -> None:
        def run_cli(argv: list[str]) -> int:
            raise AssertionError("DAGAYN_PYTHON_CLI=1 must bypass the Rust CLI")

        ran: list[bool] = []
        monkeypatch.setattr(_core, "run_cli", run_cli)
        monkeypatch.setattr("dagayn.cli.main", lambda: ran.append(True), raising=False)
        monkeypatch.setenv("DAGAYN_PYTHON_CLI", "1")

        _cli_launcher.main()

        assert ran == [True]

    def test_core_declines_what_it_does_not_run(self) -> None:
        for argv in (["dagayn"], ["dagayn", "--version"], ["dagayn", "serve"]):
            assert _core.run_cli(argv) is None, argv


class TestEndToEnd:
    def test_version_and_unported_commands_reach_python(self) -> None:
        out = _launch("--version")
        assert out.returncode == 0
        assert out.stdout.startswith("dagayn ")
        assert "0.1.0" not in out.stdout

        out = _launch("repos")
        assert out.returncode == 0, out.stderr

    def test_rust_and_python_status_match(self, tmp_path) -> None:
        repo = _project(tmp_path)
        assert _launch("build", "--repo", str(repo)).returncode == 0

        rust = _launch("status", "--repo", str(repo))
        python = _launch("status", "--repo", str(repo), DAGAYN_PYTHON_CLI="1")

        assert rust.returncode == python.returncode == 0
        assert rust.stdout == python.stdout
        assert "Nodes: " in rust.stdout

    def test_corrupt_database_is_quarantined(self, tmp_path) -> None:
        repo = _project(tmp_path)
        db = repo / ".dagayn" / "graph.db"
        db.parent.mkdir()
        db.write_bytes(b"Z" * 8192)

        out = _launch("update", "--repo", str(repo), "--skip-flows")

        assert out.returncode == 1
        assert "graph database is corrupt" in out.stderr
        assert not db.exists()
        assert list(db.parent.glob("graph.db.corrupt-*"))
