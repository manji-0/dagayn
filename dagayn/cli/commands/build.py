"""build / update / postprocess / watch / status commands."""

from __future__ import annotations

import argparse

from ...hook_guard import DEFAULT_HOOK_BUDGET_SECONDS
from ._shared import CommandRegistry, _add_local_embedding_args


def register_commands(sub: argparse._SubParsersAction) -> CommandRegistry:
    """Register build/update/postprocess/watch/status subcommands."""

    # build
    build_cmd = sub.add_parser("build", help="Full graph build (re-parse all files)")
    build_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")
    build_cmd.add_argument(
        "--force-full-build",
        "--force",
        dest="force_full_build",
        action="store_true",
        help="Delete the existing graph database before rebuilding",
    )
    build_cmd.add_argument(
        "--skip-flows",
        action="store_true",
        help="Skip community detection (signatures + FTS only)",
    )
    build_cmd.add_argument(
        "--skip-postprocess",
        action="store_true",
        help="Skip all post-processing (raw parse only)",
    )
    build_cmd.add_argument(
        "--scip",
        action="store_true",
        help=(
            "Settle call targets with SCIP indexers before post-processing: "
            "dagayn's own TypeScript and Python indexers, and rust-analyzer, "
            "scip-go, ... when installed; prints install hints for missing ones"
        ),
    )
    _add_local_embedding_args(build_cmd)

    # update
    update_cmd = sub.add_parser("update", help="Incremental update (only changed files)")
    update_cmd.add_argument(
        "--base",
        default=None,
        help=("Git diff base (default: the commit the graph was built at, falling back to HEAD~1)"),
    )
    update_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")
    update_cmd.add_argument(
        "--skip-flows",
        action="store_true",
        help="Skip community detection (signatures + FTS only)",
    )
    update_cmd.add_argument(
        "--skip-postprocess",
        action="store_true",
        help="Skip all post-processing (raw parse only)",
    )
    update_cmd.add_argument(
        "--budget-seconds",
        type=float,
        default=None,
        help=(
            "Stop the update if it outlives this many seconds. Hook-triggered runs"
            f" (DAGAYN_HOOK_UPDATE=1) default to {DEFAULT_HOOK_BUDGET_SECONDS}s;"
            " manual runs are unbounded. Use 0 to disable."
        ),
    )
    _add_local_embedding_args(update_cmd)

    # postprocess
    pp_cmd = sub.add_parser(
        "postprocess",
        help="Run post-processing on existing graph (communities, FTS)",
    )
    pp_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")
    pp_cmd.add_argument("--no-communities", action="store_true", help="Skip community detection")
    pp_cmd.add_argument("--no-fts", action="store_true", help="Skip FTS rebuild")

    # watch
    watch_cmd = sub.add_parser("watch", help="Watch for changes and auto-update")
    watch_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")

    # status
    status_cmd = sub.add_parser("status", help="Show graph statistics")
    status_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")

    return {
        "build": build_cmd,
        "update": update_cmd,
        "postprocess": pp_cmd,
        "watch": watch_cmd,
        "status": status_cmd,
    }


def handle(args: argparse.Namespace) -> None:
    """Dispatch build/update/postprocess/watch/status commands."""
    from .build_handlers import execute_build_command

    execute_build_command(args)
