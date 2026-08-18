"""Tests for CLI helpers."""

import argparse
import logging
import sqlite3
from importlib.metadata import PackageNotFoundError

import pytest

from dagayn import cli
from dagayn.cli import app as cli_app
from dagayn.graph.sqlite_errors import quarantine_corrupt_database


def test_get_version_logs_and_falls_back_to_dev(monkeypatch, caplog):
    def _raise_package_not_found(_dist_name: str) -> str:
        raise PackageNotFoundError("dagayn")

    monkeypatch.setattr(cli, "pkg_version", _raise_package_not_found)

    with caplog.at_level(logging.DEBUG, logger="dagayn.cli"):
        version = cli._get_version()

    assert version == "dev"
    assert "Package metadata unavailable" in caplog.text


class TestQuarantineCorruptDatabase:
    def test_moves_database_and_sidecars_aside(self, tmp_path) -> None:
        db = tmp_path / "graph.db"
        db.write_bytes(b"not a database")
        (tmp_path / "graph.db-wal").write_bytes(b"wal")
        (tmp_path / "graph.db-shm").write_bytes(b"shm")

        moved = quarantine_corrupt_database(db)

        assert moved is not None
        assert moved.read_bytes() == b"not a database"
        assert not db.exists()
        # -wal / -shm must follow, otherwise SQLite treats a fresh database as
        # the same WAL generation.
        assert not (tmp_path / "graph.db-wal").exists()
        assert not (tmp_path / "graph.db-shm").exists()
        assert moved.with_name(moved.name + "-wal").read_bytes() == b"wal"
        assert moved.with_name(moved.name + "-shm").read_bytes() == b"shm"

    def test_missing_database_is_a_noop(self, tmp_path) -> None:
        assert quarantine_corrupt_database(tmp_path / "absent.db") is None

    def test_in_memory_database_is_a_noop(self) -> None:
        assert quarantine_corrupt_database(":memory:") is None


class TestCorruptDatabaseReporting:
    def _args(self, repo) -> argparse.Namespace:
        return argparse.Namespace(command="update", repo=str(repo))

    def test_quarantines_and_points_at_rebuild(self, tmp_path, capsys) -> None:
        db_dir = tmp_path / ".dagayn"
        db_dir.mkdir()
        db = db_dir / "graph.db"
        db.write_bytes(b"corrupt")

        cli_app._report_corrupt_database(
            self._args(tmp_path),
            sqlite3.DatabaseError("database disk image is malformed"),
        )

        err = capsys.readouterr().err
        assert "graph database is corrupt" in err
        assert "dagayn build" in err
        assert not db.exists()
        assert list(db_dir.glob("graph.db.corrupt-*"))

    def test_reports_even_without_a_resolvable_database(
        self, capsys, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        # With repo=None the path resolver falls back to the project root of
        # the current working directory. Running from the dagayn checkout,
        # that is the real repository — and the quarantine step would move the
        # real graph.db aside. Pin the resolver to "unresolvable" so the test
        # matches its premise.
        monkeypatch.setattr(
            "dagayn.incremental_files.find_project_root",
            lambda *args, **kwargs: None,
        )

        cli_app._report_corrupt_database(
            argparse.Namespace(command="update", repo=None),
            sqlite3.DatabaseError("database disk image is malformed"),
        )

        assert "graph database is corrupt" in capsys.readouterr().err

    def test_graph_db_path_survives_a_broken_repo_argument(self) -> None:
        args = argparse.Namespace(command="update", repo="\0invalid")
        # Must degrade to None rather than masking the SQLite error.
        assert cli_app._graph_db_path(args) is None

    def test_corrupt_error_exits_without_traceback(self, tmp_path, monkeypatch, capsys) -> None:
        recorded: dict[str, object] = {}

        def _fake_report(args, exc) -> None:
            recorded["command"] = args.command
            recorded["exc"] = str(exc)

        monkeypatch.setattr(cli_app, "_report_corrupt_database", _fake_report)
        monkeypatch.setattr(
            cli_app,
            "_command_module",
            lambda name: _StubCommandModule(name),
        )
        monkeypatch.setattr("sys.argv", ["dagayn", "update", "--repo", str(tmp_path)])

        with pytest.raises(SystemExit) as excinfo:
            cli_app.main()

        assert excinfo.value.code == 1
        assert recorded["command"] == "update"
        assert "malformed" in str(recorded["exc"])

    def test_unrelated_sqlite_error_still_propagates(self, tmp_path, monkeypatch) -> None:
        monkeypatch.setattr(
            cli_app,
            "_command_module",
            lambda name: _StubCommandModule(name, exc=sqlite3.DatabaseError("disk I/O error")),
        )
        monkeypatch.setattr("sys.argv", ["dagayn", "update", "--repo", str(tmp_path)])

        with pytest.raises(sqlite3.DatabaseError, match="disk I/O error"):
            cli_app.main()


class _StubCommandModule:
    """Stands in for a lazily imported ``dagayn.cli.commands.*`` module."""

    def __init__(self, name: str, exc: BaseException | None = None) -> None:
        self._name = name
        self._exc = exc or sqlite3.DatabaseError("database disk image is malformed")

    def handle(self, args, *rest):
        if self._name == "build":
            raise self._exc

    def register_commands(self, sub):
        if self._name == "build":
            parser = sub.add_parser("update")
            parser.add_argument("--repo")
            return {}
        return {}

    def register_command(self, sub):
        return None

    def handle_hook_repo(self, args) -> None:
        pass
