"""Guards against hook-triggered updates running forever.

Regression cover for a hook-started ``dagayn update`` that burned CPU for over
an hour on a 153k-file monorepo: neither Claude Code's hook timeout nor
Cursor's detached ``afterFileEdit`` kills the dagayn process, so it has to
bound itself.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import time
from pathlib import Path

import pytest

from dagayn.cli.commands.build_handlers import _hook_update_budget
from dagayn.hook_guard import (
    BUDGET_EXCEEDED_EXIT_CODE,
    DEFAULT_HOOK_BUDGET_SECONDS,
    HOOK_SKIP_MARKER,
    HOOK_UPDATE_ENV,
    hook_updates_disabled,
    running_from_hook,
    start_budget_watchdog,
)


class TestRunningFromHook:
    def test_unset_env_is_manual(self, monkeypatch) -> None:
        monkeypatch.delenv(HOOK_UPDATE_ENV, raising=False)
        assert not running_from_hook()

    def test_zero_is_manual(self, monkeypatch) -> None:
        monkeypatch.setenv(HOOK_UPDATE_ENV, "0")
        assert not running_from_hook()

    def test_one_is_a_hook_run(self, monkeypatch) -> None:
        monkeypatch.setenv(HOOK_UPDATE_ENV, "1")
        assert running_from_hook()


class TestHookUpdatesDisabled:
    def test_absent_marker_allows_updates(self, tmp_path) -> None:
        (tmp_path / ".dagayn").mkdir()
        assert not hook_updates_disabled(tmp_path)

    def test_marker_disables_updates(self, tmp_path) -> None:
        (tmp_path / ".dagayn").mkdir()
        (tmp_path / ".dagayn" / HOOK_SKIP_MARKER).touch()
        assert hook_updates_disabled(tmp_path)

    def test_missing_dagayn_dir_allows_updates(self, tmp_path) -> None:
        assert not hook_updates_disabled(tmp_path)

    def test_worktree_inherits_the_main_checkout_marker(
        self,
        main_repo: Path,
        linked_worktree: Path,
    ) -> None:
        """A linked worktree has its own .dagayn/, so the marker must carry over."""
        (main_repo / ".dagayn").mkdir(exist_ok=True)
        (main_repo / ".dagayn" / HOOK_SKIP_MARKER).touch()
        (linked_worktree / ".dagayn").mkdir(exist_ok=True)

        assert hook_updates_disabled(linked_worktree)

    def test_worktree_without_a_main_marker_still_updates(
        self,
        main_repo: Path,
        linked_worktree: Path,
    ) -> None:
        (main_repo / ".dagayn").mkdir(exist_ok=True)
        (linked_worktree / ".dagayn").mkdir(exist_ok=True)

        assert not hook_updates_disabled(linked_worktree)

    def test_non_git_directory_does_not_raise(self, tmp_path) -> None:
        assert not hook_updates_disabled(tmp_path / "absent")


class TestBudgetWatchdog:
    def test_no_budget_returns_no_timer(self) -> None:
        assert start_budget_watchdog(None) is None
        assert start_budget_watchdog(0) is None
        assert start_budget_watchdog(-1) is None

    def test_timer_is_cancellable(self) -> None:
        timer = start_budget_watchdog(60)
        assert timer is not None
        try:
            assert timer.is_alive()
        finally:
            timer.cancel()

    def test_expiring_watchdog_kills_the_process(self, tmp_path) -> None:
        """The watchdog must terminate a wedged process, not just log."""
        script = tmp_path / "wedged.py"
        script.write_text(
            "import time\n"
            "from dagayn.hook_guard import start_budget_watchdog\n"
            "start_budget_watchdog(0.3, label='update')\n"
            "time.sleep(30)\n",
            encoding="utf-8",
        )
        started = time.monotonic()
        completed = subprocess.run(
            [sys.executable, str(script)],
            capture_output=True,
            text=True,
            timeout=30,
            cwd=str(Path(__file__).resolve().parent.parent),
        )
        assert completed.returncode == BUDGET_EXCEEDED_EXIT_CODE
        assert "budget" in completed.stderr
        assert time.monotonic() - started < 20


class TestHookUpdateBudget:
    def test_hook_run_is_bounded_by_default(self, monkeypatch) -> None:
        monkeypatch.setenv(HOOK_UPDATE_ENV, "1")
        args = argparse.Namespace(budget_seconds=None)
        assert _hook_update_budget(args) == float(DEFAULT_HOOK_BUDGET_SECONDS)

    def test_manual_run_is_unbounded(self, monkeypatch) -> None:
        monkeypatch.delenv(HOOK_UPDATE_ENV, raising=False)
        assert _hook_update_budget(argparse.Namespace(budget_seconds=None)) is None

    def test_explicit_budget_wins_over_manual_default(self, monkeypatch) -> None:
        monkeypatch.delenv(HOOK_UPDATE_ENV, raising=False)
        assert _hook_update_budget(argparse.Namespace(budget_seconds=5)) == 5.0

    def test_zero_disables_the_guard_even_for_hooks(self, monkeypatch) -> None:
        monkeypatch.setenv(HOOK_UPDATE_ENV, "1")
        assert _hook_update_budget(argparse.Namespace(budget_seconds=0)) is None


class TestUpdateCommandSkips:
    def test_marker_short_circuits_a_hook_update(self, tmp_path, monkeypatch, capsys) -> None:
        from dagayn.cli.commands import build_handlers

        (tmp_path / ".dagayn").mkdir()
        (tmp_path / ".dagayn" / HOOK_SKIP_MARKER).touch()
        monkeypatch.setenv(HOOK_UPDATE_ENV, "1")

        def _must_not_run(*_args, **_kwargs):
            pytest.fail("update ran despite the hook-skip marker")

        monkeypatch.setattr(build_handlers, "_run_update_command", _must_not_run)

        build_handlers.handle_update_command(
            argparse.Namespace(budget_seconds=None, repo=str(tmp_path), command="update"),
            tmp_path,
            tmp_path / ".dagayn" / "graph.db",
        )

        assert HOOK_SKIP_MARKER in capsys.readouterr().out

    def test_marker_does_not_block_a_manual_update(self, tmp_path, monkeypatch) -> None:
        from dagayn.cli.commands import build_handlers

        (tmp_path / ".dagayn").mkdir()
        (tmp_path / ".dagayn" / HOOK_SKIP_MARKER).touch()
        monkeypatch.delenv(HOOK_UPDATE_ENV, raising=False)
        ran: list[bool] = []
        monkeypatch.setattr(
            build_handlers,
            "_run_update_command",
            lambda *_args, **_kwargs: ran.append(True),
        )

        build_handlers.handle_update_command(
            argparse.Namespace(budget_seconds=None, repo=str(tmp_path), command="update"),
            tmp_path,
            tmp_path / ".dagayn" / "graph.db",
        )

        assert ran == [True]
