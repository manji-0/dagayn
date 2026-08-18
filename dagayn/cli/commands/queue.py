"""queue — background task queue for graph processing.

Hooks enqueue tasks instead of spawning their own ``dagayn update`` process;
a single detached worker per repository drains the queue (see
:mod:`dagayn.task_queue`).
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from ...task_queue import (
    DEFAULT_IDLE_SECONDS,
    TASK_KINDS,
    TaskQueue,
    ensure_worker,
    queue_db_path,
    run_worker,
)
from ._shared import _add_local_embedding_args


def register_commands(sub: argparse._SubParsersAction) -> dict:
    """Register the ``queue`` subcommand tree."""
    queue_cmd = sub.add_parser(
        "queue",
        help="Background task queue for graph processing (update/embed/postprocess)",
    )
    qsub = queue_cmd.add_subparsers(dest="queue_command")

    add_cmd = qsub.add_parser(
        "add",
        help="Enqueue a task and make sure a worker is running",
    )
    add_cmd.add_argument("kind", choices=list(TASK_KINDS), help="Task kind to enqueue")
    add_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")
    add_cmd.add_argument(
        "--priority",
        type=int,
        default=0,
        help="Higher-priority tasks are claimed first (default: 0)",
    )
    add_cmd.add_argument(
        "--no-worker",
        action="store_true",
        help="Only enqueue; do not check for / spawn a worker",
    )
    add_cmd.add_argument(
        "--idle-seconds",
        type=float,
        default=DEFAULT_IDLE_SECONDS,
        help="Idle window for the spawned worker (default: 60)",
    )
    # Embedding configuration; only meaningful for ``queue add embed``.
    _add_local_embedding_args(add_cmd)

    run_cmd = qsub.add_parser(
        "run",
        help="Run the queue worker until the queue has been empty for the idle window",
    )
    run_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")
    run_cmd.add_argument(
        "--idle-seconds",
        type=float,
        default=DEFAULT_IDLE_SECONDS,
        help="Exit after the queue has been empty this long (default: 60)",
    )
    run_cmd.add_argument(
        "--max-tasks",
        type=int,
        default=None,
        help="Exit after executing this many tasks (default: unbounded)",
    )

    status_cmd = qsub.add_parser("status", help="Show queued and recent tasks")
    status_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")
    status_cmd.add_argument(
        "--json",
        action="store_true",
        dest="as_json",
        help="Emit the queue state as JSON",
    )

    clear_cmd = qsub.add_parser("clear", help="Drop all queued tasks")
    clear_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")

    return {"queue": queue_cmd}


def _resolve_repo(repo: str | None) -> Path:
    if repo:
        return Path(repo).expanduser().resolve()
    from ...incremental_files import find_project_root

    found = find_project_root()
    if found is not None:
        return found
    return Path.cwd().resolve()


def _embed_payload(args: argparse.Namespace) -> dict:
    """Embedding configuration for an ``embed`` task, from the CLI flags."""
    return {
        "local_embedding": args.local_embedding,
        "local_embedding_mode": args.local_embedding_mode,
        "local_embedding_port": args.local_embedding_port,
        "local_embedding_bin": args.local_embedding_bin,
        "keep_local_embedding_server": bool(args.keep_local_embedding_server),
        "local_embedding_timeout": args.local_embedding_timeout,
        "local_embedding_request_timeout": args.local_embedding_request_timeout,
        "local_embedding_batch_size": args.local_embedding_batch_size,
    }


def handle(args: argparse.Namespace, queue_parser: argparse.ArgumentParser) -> None:
    """Handle queue subcommands."""
    command = getattr(args, "queue_command", None)
    if command is None:
        queue_parser.print_help()
        return

    root = _resolve_repo(getattr(args, "repo", None))

    if command == "add":
        payload = _embed_payload(args) if args.kind == "embed" else None
        queue = TaskQueue(queue_db_path(root))
        try:
            action, task_id = queue.enqueue(args.kind, payload=payload, priority=args.priority)
        finally:
            queue.close()
        message = f"queue: {action} {args.kind} task #{task_id}"
        if not args.no_worker:
            spawned = ensure_worker(root, idle_seconds=args.idle_seconds)
            message += "; worker spawned" if spawned else "; worker already running"
        print(message)
        return

    if command == "run":
        executed = run_worker(
            root,
            idle_seconds=args.idle_seconds,
            max_tasks=args.max_tasks,
        )
        print(f"queue worker: executed {executed} task(s)")
        return

    if command == "status":
        queue = TaskQueue(queue_db_path(root))
        try:
            stats = queue.stats()
        finally:
            queue.close()
        if args.as_json:
            print(json.dumps(stats))
            return
        counts = stats["counts"]
        print(
            "Queue: "
            f"{counts.get('pending', 0)} pending, "
            f"{counts.get('running', 0)} running, "
            f"{counts.get('dead', 0)} dead"
        )
        for entry in stats["recent"]:
            note = f" — {entry['note']}" if entry["note"] else ""
            print(
                f"  [{entry['at']}] task #{entry['task_id']} {entry['kind']} {entry['state']}{note}"
            )
        return

    if command == "clear":
        queue = TaskQueue(queue_db_path(root))
        try:
            removed = queue.clear()
        finally:
            queue.close()
        print(f"queue: removed {removed} task(s)")
        return

    queue_parser.print_help()
    sys.exit(2)
