"""register / unregister / repos commands — argument registration and handler."""

from __future__ import annotations

import argparse
import logging
import sys


def register_commands(sub: argparse._SubParsersAction) -> dict:
    """Register register/unregister/repos subcommands. Returns {cmd_name: subparser} dict."""

    register_cmd = sub.add_parser(
        "register", help="Register a repository in the multi-repo registry"
    )
    register_cmd.add_argument("path", help="Path to the repository root")
    register_cmd.add_argument("--alias", default=None, help="Short alias for the repository")

    unregister_cmd = sub.add_parser(
        "unregister", help="Remove a repository from the multi-repo registry"
    )
    unregister_cmd.add_argument("path_or_alias", help="Repository path or alias to remove")

    repos_cmd = sub.add_parser("repos", help="List registered repositories")

    return {
        "register": register_cmd,
        "unregister": unregister_cmd,
        "repos": repos_cmd,
    }


def handle(args: argparse.Namespace) -> None:
    """Handle register/unregister/repos commands."""
    logging.basicConfig(level=logging.INFO, format="%(levelname)s: %(message)s")
    from ...registry import Registry

    registry = Registry()
    if args.command == "register":
        try:
            entry = registry.register(args.path, alias=args.alias)
            alias_info = f" (alias: {entry['alias']})" if entry.get("alias") else ""
            print(f"Registered: {entry['path']}{alias_info}")
        except ValueError as exc:
            logging.error(str(exc))
            sys.exit(1)
    elif args.command == "unregister":
        if registry.unregister(args.path_or_alias):
            print(f"Unregistered: {args.path_or_alias}")
        else:
            print(f"Not found: {args.path_or_alias}")
            sys.exit(1)
    elif args.command == "repos":
        repos = registry.list_repos()
        if not repos:
            print("No repositories registered.")
            print("Use: dagayn register <path> [--alias name]")
        else:
            for entry in repos:
                alias = entry.get("alias", "")
                alias_str = f"  ({alias})" if alias else ""
                print(f"  {entry['path']}{alias_str}")
