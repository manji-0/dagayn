"""Guards that stop a hook-triggered ``dagayn update`` from running forever.

Neither editor kills the process it started:

* Claude Code's ``PostToolUse`` timeout only abandons the hook's shell; the
  ``dagayn`` child is reparented to PID 1 and keeps running.
* Cursor's ``afterFileEdit`` hook deliberately detaches, so nothing is
  watching it at all.

On a 153k-file monorepo that produced a single ``dagayn update`` burning CPU
for over an hour while its WAL grew into the gigabytes, with a fresh one
started on every edit. dagayn therefore has to bound *itself*:

* :func:`start_budget_watchdog` terminates the process once a wall-clock
  budget is spent. WAL journalling makes the interrupted write roll back, so
  the graph stays consistent — the update simply has to be redone.
* :func:`hook_updates_disabled` lets a repository opt out of edit-triggered
  updates entirely, for trees where an incremental update can never be cheap.
"""

from __future__ import annotations

import logging
import os
import subprocess
import sys
import threading
from pathlib import Path

logger = logging.getLogger(__name__)

#: Set by generated hooks so dagayn can tell an unattended run from a manual one.
HOOK_UPDATE_ENV = "DAGAYN_HOOK_UPDATE"

#: Opt-out marker, looked up under ``.dagayn/`` in the repository (and in the
#: main checkout when called from a linked worktree).
HOOK_SKIP_MARKER = "hook-skip"

#: Wall-clock budget applied to hook-triggered updates when none is given.
DEFAULT_HOOK_BUDGET_SECONDS = 120

#: Exit status used when the watchdog fires. Generated hooks append ``|| true``,
#: so this never fails the surrounding tool call.
BUDGET_EXCEEDED_EXIT_CODE = 75


def running_from_hook() -> bool:
    """True when the current process was started by a generated hook."""
    return os.environ.get(HOOK_UPDATE_ENV, "") not in ("", "0")


def _main_checkout(repo_root: Path) -> Path | None:
    """Main checkout for *repo_root*, or ``None`` when it is not a worktree.

    A linked worktree keeps its own ``.dagayn/``, so a marker placed in the
    main checkout would otherwise be invisible to worktree sessions — and
    those are created and thrown away constantly.
    """
    try:
        completed = subprocess.run(
            [
                "git",
                "-C",
                str(repo_root),
                "rev-parse",
                "--path-format=absolute",
                "--git-common-dir",
            ],
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if completed.returncode != 0:
        return None
    common_dir = completed.stdout.strip()
    if not common_dir or not common_dir.endswith("/.git"):
        return None
    main_root = Path(common_dir[: -len("/.git")])
    return None if main_root == Path(repo_root) else main_root


def hook_updates_disabled(repo_root: str | Path) -> bool:
    """True when *repo_root* opted out of hook-triggered updates.

    Only consulted for hook runs; an explicit ``dagayn update`` typed by a
    human always proceeds.
    """
    root = Path(repo_root)
    if (root / ".dagayn" / HOOK_SKIP_MARKER).exists():
        return True
    main_root = _main_checkout(root)
    return main_root is not None and (main_root / ".dagayn" / HOOK_SKIP_MARKER).exists()


def start_budget_watchdog(
    budget_seconds: float | None,
    *,
    label: str = "update",
) -> threading.Timer | None:
    """Terminate this process if it outlives *budget_seconds*.

    Returns the timer so a caller that finishes in time can cancel it, or
    ``None`` when no budget applies.
    """
    if budget_seconds is None or budget_seconds <= 0:
        return None

    def _expire() -> None:
        # os._exit, not sys.exit: the work happens inside Rust/SQLite calls that
        # would swallow or delay an exception, which is exactly the hang being
        # cut short. WAL rolls the interrupted transaction back.
        sys.stderr.write(
            f"dagayn: {label} exceeded its {budget_seconds:g}s budget and was stopped;"
            " the graph is unchanged\n"
        )
        sys.stderr.flush()
        os._exit(BUDGET_EXCEEDED_EXIT_CODE)

    timer = threading.Timer(float(budget_seconds), _expire)
    timer.daemon = True
    timer.start()
    return timer
