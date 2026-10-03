"""The ``dagayn`` console script.

Commands that hooks run on every edit or session skip loading the Python
CLI (argparse and every command module):

* ``build``, ``update``, and ``status`` run in the Rust CLI compiled into
  ``dagayn._core``, which answers ``None`` for any command line it does not
  handle exactly as the Python CLI would;
* ``queue add`` of a payload-free kind with ``--repo`` calls the same
  ``TaskQueue`` and ``ensure_worker`` the ``queue`` command does.

Everything else, anything those decline, and every command under
``DAGAYN_PYTHON_CLI=1`` runs the Python CLI. ``python -m dagayn`` always does.
"""

import os
import signal
import sys

_RUST_COMMANDS = frozenset({"build", "update", "status"})
#: ``queue add`` kinds without a payload; ``embed`` takes the embedding flags.
_QUEUE_FAST_KINDS = frozenset({"update", "postprocess", "prepare"})


def main() -> None:
    args = sys.argv[1:]
    if os.environ.get("DAGAYN_PYTHON_CLI", "").strip().lower() not in ("1", "true", "yes"):
        if args and args[0] in _RUST_COMMANDS:
            _run_rust(args)
        elif args[:2] == ["queue", "add"]:
            _queue_add(args[2:])

    from dagayn.cli import main as cli_main

    cli_main()


def _run_rust(args: list[str]) -> None:
    """Exit with the Rust CLI's status, or return when it declines."""
    from dagayn import _core

    # Python only acts on SIGINT between bytecodes, which never come while
    # the Rust CLI runs; let Ctrl-C stop the process as it would the binary.
    handler = signal.signal(signal.SIGINT, signal.SIG_DFL)
    status = _core.run_cli(["dagayn", *args])
    if status is not None:
        sys.exit(status)
    signal.signal(signal.SIGINT, handler)


def _queue_add(args: list[str]) -> None:
    """``queue add`` as ``dagayn.cli.commands.queue.handle`` runs it; returns
    without side effects for a command line it does not recognise."""
    options = _parse_queue_add(args)
    if options is None:
        return
    kind, repo, priority, no_worker, idle_seconds = options

    from pathlib import Path

    from dagayn.task_queue import TaskQueue, ensure_worker, queue_db_path

    root = Path(repo).expanduser().resolve()
    try:
        queue = TaskQueue(queue_db_path(root))
        try:
            action, task_id = queue.enqueue(kind, priority=priority)
        finally:
            queue.close()
    except Exception:  # noqa: BLE001 - nothing was enqueued; let the CLI report it
        return
    message = f"queue: {action} {kind} task #{task_id}"
    if not no_worker:
        spawned = ensure_worker(root, idle_seconds=idle_seconds)
        message += "; worker spawned" if spawned else "; worker already running"
    print(message)
    sys.exit(0)


def _parse_queue_add(args: list[str]) -> tuple[str, str, int | None, bool, float] | None:
    """``(kind, repo, priority, no_worker, idle_seconds)``, or ``None`` for
    anything but the exact spellings hooks use (argparse also takes prefixes,
    ``--help``, and errors, which stay with the CLI)."""
    from dagayn.task_queue import DEFAULT_IDLE_SECONDS

    kind = repo = None
    priority: int | None = None
    no_worker = False
    idle_seconds = DEFAULT_IDLE_SECONDS
    tokens = iter(args)
    try:
        for token in tokens:
            name, sep, inline = token.partition("=")
            if name in ("--repo", "--priority", "--idle-seconds"):
                value = inline if sep else next(tokens)
                if name == "--repo":
                    repo = value
                elif name == "--priority":
                    priority = int(value)
                else:
                    idle_seconds = float(value)
            elif token == "--no-worker":  # nosec B105 - a flag name, not a password
                no_worker = True
            elif token.startswith("-") or kind is not None:
                return None
            else:
                kind = token
    except (StopIteration, ValueError):
        return None
    if kind is None or kind not in _QUEUE_FAST_KINDS or not repo:
        return None
    return kind, repo, priority, no_worker, idle_seconds
