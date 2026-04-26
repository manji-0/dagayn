"""daemon command — argument registration and handler."""

from __future__ import annotations

import argparse


def register_command(sub: argparse._SubParsersAction) -> argparse.ArgumentParser:
    """Register the daemon subcommand. Returns the subparser."""
    daemon_cmd = sub.add_parser(
        "daemon",
        help="Multi-repo watch daemon (start/stop/status/add/remove)",
    )
    daemon_sub = daemon_cmd.add_subparsers(dest="daemon_command")

    daemon_start = daemon_sub.add_parser("start", help="Start the watch daemon")
    daemon_start.add_argument(
        "--foreground",
        action="store_true",
        help="Run in foreground instead of daemonizing",
    )

    daemon_sub.add_parser("stop", help="Stop the watch daemon")

    daemon_restart = daemon_sub.add_parser("restart", help="Restart the watch daemon")
    daemon_restart.add_argument(
        "--foreground",
        action="store_true",
        help="Run in foreground instead of daemonizing",
    )

    daemon_sub.add_parser("status", help="Show daemon and watcher status")

    daemon_logs = daemon_sub.add_parser("logs", help="View daemon or watcher logs")
    daemon_logs.add_argument(
        "--repo",
        default=None,
        help="Show logs for a specific repo alias",
    )
    daemon_logs.add_argument(
        "--follow",
        action="store_true",
        help="Follow log output (tail -f)",
    )
    daemon_logs.add_argument(
        "--lines",
        type=int,
        default=50,
        help="Number of lines to show (default: 50)",
    )

    daemon_add = daemon_sub.add_parser("add", help="Add a repo to the watch config")
    daemon_add.add_argument("path", help="Path to the repository")
    daemon_add.add_argument("--alias", default=None, help="Short alias for the repo")

    daemon_remove = daemon_sub.add_parser("remove", help="Remove a repo from the watch config")
    daemon_remove.add_argument("path_or_alias", help="Repository path or alias to remove")

    return daemon_cmd


def handle(args: argparse.Namespace, daemon_parser: argparse.ArgumentParser) -> None:
    """Handle daemon subcommands."""
    if not args.daemon_command:
        daemon_parser.print_help()
        return

    from ...daemon_cli import (
        _handle_add,
        _handle_logs,
        _handle_remove,
        _handle_restart,
        _handle_start,
        _handle_status,
        _handle_stop,
    )

    handlers = {
        "start": _handle_start,
        "stop": _handle_stop,
        "restart": _handle_restart,
        "status": _handle_status,
        "logs": _handle_logs,
        "add": _handle_add,
        "remove": _handle_remove,
    }
    handler = handlers.get(args.daemon_command)
    if handler:
        handler(args)
