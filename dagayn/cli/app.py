"""dagayn CLI main entry point."""

from __future__ import annotations

import sys
from importlib import import_module

# Python version check — must come before any other imports
if sys.version_info < (3, 12):
    print("dagayn requires Python 3.12 or higher.")
    print(f"  You are running Python {sys.version}")
    print()
    print("Install Python 3.12+: https://www.python.org/downloads/")
    sys.exit(1)

import argparse

from .utils import _get_version, _print_banner

# Build commands that are handled by build.py's handle()
_BUILD_COMMANDS = frozenset(
    {
        "build",
        "update",
        "postprocess",
        "watch",
        "status",
        "visualize",
        "detect-adp",
        "sdp-metrics",
        "detect-sdp",
        "sap-metrics",
        "detect-sap",
    }
)
_TOOL_COMMANDS = frozenset({"tool"})


def _command_module(name: str):
    """Load a command module lazily to keep cli package imports acyclic."""
    return import_module(f"dagayn.cli.commands.{name}")


def _graph_db_path(args: argparse.Namespace):
    """Best-effort graph database path for *args*, or ``None``.

    Used only for corruption reporting, so every failure mode collapses to
    ``None`` instead of masking the original SQLite error.
    """
    from pathlib import Path

    try:
        from ..incremental_files import find_project_root
        from ..paths import get_db_path

        repo = getattr(args, "repo", None)
        repo_root = Path(repo) if repo else find_project_root()
        if repo_root is None:
            return None
        return get_db_path(Path(repo_root))
    except Exception:  # noqa: BLE001 — reporting must not raise
        return None


def _report_corrupt_database(args: argparse.Namespace, exc: BaseException) -> None:
    """Quarantine a corrupt graph database and explain the next step.

    Without this, an unattended hook (``PostToolUse``, Cursor's
    ``afterFileEdit``) prints a full traceback on every single invocation and
    never recovers, because the corrupt file stays in place.
    """
    from ..graph.sqlite_errors import quarantine_corrupt_database

    db_path = _graph_db_path(args)
    moved = quarantine_corrupt_database(db_path) if db_path is not None else None

    print(f"dagayn: graph database is corrupt ({exc})", file=sys.stderr)
    if moved is not None:
        print(f"dagayn: moved the corrupt file aside: {moved}", file=sys.stderr)
        print(
            "dagayn: run 'dagayn build' to rebuild the graph"
            " (or delete the .corrupt-* file once you no longer need it)",
            file=sys.stderr,
        )
    elif db_path is not None:
        print(
            f"dagayn: could not move {db_path} aside; delete it and run 'dagayn build'",
            file=sys.stderr,
        )
    else:
        print("dagayn: delete .dagayn/graph.db and run 'dagayn build'", file=sys.stderr)


def main() -> None:
    """Main CLI entry point."""
    init = _command_module("init")
    build = _command_module("build")
    serve = _command_module("serve")
    detect_changes = _command_module("detect_changes")
    wiki = _command_module("wiki")
    registry = _command_module("registry")
    daemon = _command_module("daemon")
    eval_cmd = _command_module("eval_cmd")
    profile = _command_module("profile")
    tool = _command_module("tool")
    worktree = _command_module("worktree")
    session = _command_module("session")

    ap = argparse.ArgumentParser(
        prog="dagayn",
        description="Persistent incremental knowledge graph for code reviews",
    )
    ap.add_argument("-v", "--version", action="store_true", help="Show version and exit")
    sub = ap.add_subparsers(dest="command")

    # Register all subcommands
    init.register_commands(sub)
    build.register_commands(sub)
    serve_parser = serve.register_command(sub)
    detect_changes.register_command(sub)
    wiki.register_command(sub)
    registry.register_commands(sub)
    daemon_parser = daemon.register_command(sub)
    eval_cmd.register_command(sub)
    profile.register_command(sub)
    tool.register_command(sub)
    worktree_parsers = worktree.register_commands(sub)
    session_parsers = session.register_commands(sub)

    args = ap.parse_args()

    if args.version:
        print(f"dagayn {_get_version()}")
        return

    if not args.command:
        _print_banner()
        return

    # A corrupt graph image raises on every open, so an unattended hook would
    # otherwise print a traceback on each edit forever. Quarantine and explain.
    import sqlite3

    from ..graph.sqlite_errors import is_sqlite_corrupt_error

    try:
        if args.command in ("install", "init"):
            init.handle(args)
        elif args.command in _BUILD_COMMANDS:
            build.handle(args)
        elif args.command == "serve":
            serve.handle(args, serve_parser)
        elif args.command == "detect-changes":
            detect_changes.handle(args)
        elif args.command == "wiki":
            wiki.handle(args)
        elif args.command in ("register", "unregister", "repos"):
            registry.handle(args)
        elif args.command == "daemon":
            daemon.handle(args, daemon_parser)
        elif args.command == "eval":
            eval_cmd.handle(args)
        elif args.command == "profile":
            rc = profile.handle(args)
            if rc:
                sys.exit(rc)
        elif args.command in _TOOL_COMMANDS:
            tool.handle(args)
        elif args.command == "worktree":
            worktree.handle(args, worktree_parsers["worktree"])
        elif args.command == "hook-repo":
            worktree.handle_hook_repo(args)
        elif args.command == "session":
            session.handle(args, session_parsers["session"])
    except sqlite3.DatabaseError as exc:
        if not is_sqlite_corrupt_error(exc):
            raise
        _report_corrupt_database(args, exc)
        sys.exit(1)
