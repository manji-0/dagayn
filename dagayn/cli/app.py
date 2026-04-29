"""dagayn CLI main entry point."""

from __future__ import annotations

import sys

# Python version check — must come before any other imports
if sys.version_info < (3, 12):
    print("dagayn requires Python 3.12 or higher.")
    print(f"  You are running Python {sys.version}")
    print()
    print("Install Python 3.12+: https://www.python.org/downloads/")
    sys.exit(1)

import argparse

from .commands import (
    build,
    daemon,
    detect_changes,
    eval_cmd,
    init,
    profile,
    registry,
    serve,
    wiki,
)
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


def main() -> None:
    """Main CLI entry point."""
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

    args = ap.parse_args()

    if args.version:
        print(f"dagayn {_get_version()}")
        return

    if not args.command:
        _print_banner()
        return

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
