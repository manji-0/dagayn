"""Tests for the ``queue`` CLI command (``dagayn.cli.commands.queue``).

The queue mechanics live in :mod:`dagayn.task_queue` (see
``tests/test_task_queue.py``); these tests cover the CLI surface: parser
registration, repo resolution, and the four subcommand handlers. No real
worker is spawned — ``ensure_worker`` / ``run_worker`` are faked.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

import pytest

from dagayn.cli.commands import queue as queue_cmd
from dagayn.task_queue import TaskQueue, queue_db_path


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="dagayn")
    sub = parser.add_subparsers(dest="command")
    queue_cmd.register_commands(sub)
    return parser


def _queue_subparser(parser: argparse.ArgumentParser) -> argparse.ArgumentParser:
    for action in parser._actions:
        if isinstance(action, argparse._SubParsersAction):
            return action.choices["queue"]
    raise AssertionError("queue subparser not registered")


def _handle(parser: argparse.ArgumentParser, *argv: str) -> None:
    queue_cmd.handle(parser.parse_args(["queue", *argv]), _queue_subparser(parser))


class TestParser:
    def test_add_defaults(self) -> None:
        args = _parser().parse_args(["queue", "add", "update"])
        assert args.command == "queue"
        assert args.queue_command == "add"
        assert args.kind == "update"
        assert args.repo is None
        assert args.priority is None
        assert args.no_worker is False
        assert args.idle_seconds == 60.0

    def test_add_flags(self) -> None:
        args = _parser().parse_args(
            [
                "queue",
                "add",
                "update",
                "--repo",
                "/tmp/repo",
                "--priority",
                "5",
                "--no-worker",
                "--idle-seconds",
                "30",
            ]
        )
        assert args.repo == "/tmp/repo"
        assert args.priority == 5
        assert args.no_worker is True
        assert args.idle_seconds == 30.0

    def test_add_rejects_unknown_kind(self) -> None:
        with pytest.raises(SystemExit):
            _parser().parse_args(["queue", "add", "nope"])

    def test_run_flags(self) -> None:
        args = _parser().parse_args(
            ["queue", "run", "--repo", "/x", "--idle-seconds", "5", "--max-tasks", "3"]
        )
        assert args.queue_command == "run"
        assert args.repo == "/x"
        assert args.idle_seconds == 5.0
        assert args.max_tasks == 3

    def test_status_json_flag(self) -> None:
        args = _parser().parse_args(["queue", "status", "--json"])
        assert args.queue_command == "status"
        assert args.as_json is True

    def test_clear_parser(self) -> None:
        args = _parser().parse_args(["queue", "clear", "--repo", "/x"])
        assert args.queue_command == "clear"
        assert args.repo == "/x"

    def test_embed_accepts_embedding_flags(self) -> None:
        args = _parser().parse_args(
            [
                "queue",
                "add",
                "embed",
                "--local-embedding",
                "low",
                "--local-embedding-port",
                "19090",
                "--keep-local-embedding-server",
            ]
        )
        assert args.kind == "embed"
        assert args.local_embedding == "low"
        assert args.local_embedding_port == 19090
        assert args.keep_local_embedding_server is True


class TestResolveRepo:
    def test_explicit_repo_is_resolved(self, tmp_path: Path) -> None:
        assert queue_cmd._resolve_repo(str(tmp_path)) == tmp_path.resolve()

    def test_falls_back_to_project_root(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        monkeypatch.setattr(
            "dagayn.incremental_files.find_project_root",
            lambda *args, **kwargs: tmp_path / "proj",
        )
        assert queue_cmd._resolve_repo(None) == (tmp_path / "proj").resolve()

    def test_falls_back_to_cwd(self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
        monkeypatch.setattr(
            "dagayn.incremental_files.find_project_root",
            lambda *args, **kwargs: None,
        )
        monkeypatch.chdir(tmp_path)
        assert queue_cmd._resolve_repo(None) == tmp_path.resolve()


class TestHandleAdd:
    def test_enqueues_and_spawns_worker(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
    ) -> None:
        spawned: list[tuple[Path, float]] = []

        def fake_ensure_worker(root: Path, *, idle_seconds: float) -> bool:
            spawned.append((root, idle_seconds))
            return True

        monkeypatch.setattr(queue_cmd, "ensure_worker", fake_ensure_worker)
        _handle(
            _parser(),
            "add",
            "update",
            "--repo",
            str(tmp_path),
            "--idle-seconds",
            "30",
        )

        out = capsys.readouterr().out
        assert "added" in out
        assert "worker spawned" in out
        assert spawned == [(tmp_path.resolve(), 30.0)]
        queue = TaskQueue(queue_db_path(tmp_path))
        try:
            assert queue.stats()["counts"]["pending"] == 1
        finally:
            queue.close()

    def test_no_worker_skips_worker_check(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
    ) -> None:
        called: list[Any] = []
        monkeypatch.setattr(queue_cmd, "ensure_worker", lambda *a, **k: called.append(1) or False)
        _handle(_parser(), "add", "update", "--repo", str(tmp_path), "--no-worker")

        assert called == []
        assert "worker" not in capsys.readouterr().out

    def test_embed_stores_payload(self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
        monkeypatch.setattr(queue_cmd, "ensure_worker", lambda *a, **k: False)
        _handle(
            _parser(),
            "add",
            "embed",
            "--repo",
            str(tmp_path),
            "--no-worker",
            "--local-embedding",
            "low",
            "--local-embedding-port",
            "19090",
            "--keep-local-embedding-server",
        )

        queue = TaskQueue(queue_db_path(tmp_path))
        try:
            task = queue.claim()
        finally:
            queue.close()
        assert task is not None
        payload = task["payload"]
        assert payload["local_embedding"] == "low"
        assert payload["local_embedding_port"] == 19090
        assert payload["keep_local_embedding_server"] is True
        assert payload["local_embedding_mode"] is None

    def test_non_embed_has_no_payload(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        monkeypatch.setattr(queue_cmd, "ensure_worker", lambda *a, **k: False)
        _handle(_parser(), "add", "update", "--repo", str(tmp_path), "--no-worker")

        queue = TaskQueue(queue_db_path(tmp_path))
        try:
            task = queue.claim()
        finally:
            queue.close()
        assert task is not None
        assert task["payload"] == {}

    def test_add_coalesces_pending_twin(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
    ) -> None:
        monkeypatch.setattr(queue_cmd, "ensure_worker", lambda *a, **k: False)
        parser = _parser()
        _handle(parser, "add", "update", "--repo", str(tmp_path), "--no-worker")
        _handle(parser, "add", "update", "--repo", str(tmp_path), "--no-worker")

        assert "coalesced" in capsys.readouterr().out
        queue = TaskQueue(queue_db_path(tmp_path))
        try:
            assert queue.stats()["counts"]["pending"] == 1
        finally:
            queue.close()

    def test_priority_defaults_to_kind_priority(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        monkeypatch.setattr(queue_cmd, "ensure_worker", lambda *a, **k: False)
        _handle(_parser(), "add", "update", "--repo", str(tmp_path), "--no-worker")

        queue = TaskQueue(queue_db_path(tmp_path))
        try:
            task = queue.claim()
        finally:
            queue.close()
        assert task is not None
        assert task["priority"] == 10


class TestHandleStatus:
    def test_human_readable_counts(
        self, tmp_path: Path, capsys: pytest.CaptureFixture[str]
    ) -> None:
        queue = TaskQueue(queue_db_path(tmp_path))
        queue.enqueue("update")
        queue.close()

        _handle(_parser(), "status", "--repo", str(tmp_path))

        out = capsys.readouterr().out
        assert "1 pending" in out
        assert "0 running" in out
        assert "0 dead" in out

    def test_json_output(self, tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
        _handle(_parser(), "status", "--repo", str(tmp_path), "--json")

        stats = json.loads(capsys.readouterr().out)
        assert stats["counts"] == {"pending": 0, "running": 0, "dead": 0}
        assert stats["recent"] == []

    def test_status_lists_recent_tasks(
        self, tmp_path: Path, capsys: pytest.CaptureFixture[str]
    ) -> None:
        queue = TaskQueue(queue_db_path(tmp_path))
        queue.enqueue("update")
        task = queue.claim()
        assert task is not None
        queue.complete(task, "done note")
        queue.close()

        _handle(_parser(), "status", "--repo", str(tmp_path))

        out = capsys.readouterr().out
        assert "update" in out
        assert "done note" in out


class TestHandleClear:
    def test_removes_and_reports(self, tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
        queue = TaskQueue(queue_db_path(tmp_path))
        queue.enqueue("update")
        queue.enqueue("embed")
        queue.close()

        _handle(_parser(), "clear", "--repo", str(tmp_path))

        assert "removed 2 task(s)" in capsys.readouterr().out
        queue = TaskQueue(queue_db_path(tmp_path))
        try:
            assert queue.stats()["counts"] == {"pending": 0, "running": 0, "dead": 0}
        finally:
            queue.close()


class TestHandleRun:
    def test_delegates_to_worker(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
    ) -> None:
        calls: list[tuple[Path, float, int | None]] = []

        def fake_run_worker(root: Path, *, idle_seconds: float, max_tasks: int | None) -> int:
            calls.append((root, idle_seconds, max_tasks))
            return 2

        monkeypatch.setattr(queue_cmd, "run_worker", fake_run_worker)
        _handle(
            _parser(),
            "run",
            "--repo",
            str(tmp_path),
            "--idle-seconds",
            "5",
            "--max-tasks",
            "7",
        )

        assert calls == [(tmp_path.resolve(), 5.0, 7)]
        assert "executed 2 task(s)" in capsys.readouterr().out


class TestNoSubcommand:
    def test_prints_help(self, capsys: pytest.CaptureFixture[str]) -> None:
        parser = _parser()
        _handle(parser)

        out = capsys.readouterr().out
        assert "add" in out
        assert "status" in out
        assert "clear" in out
