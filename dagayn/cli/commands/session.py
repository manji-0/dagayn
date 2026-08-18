"""session prepare — ensure a usable+synced graph at session start / relocate."""

from __future__ import annotations

import argparse
import json
import sys

from ._shared import DEFAULT_LOCAL_EMBEDDING_BIN, CommandRegistry, _add_local_embedding_args


def register_commands(sub: argparse._SubParsersAction) -> CommandRegistry:
    """Register ``session`` subcommands."""
    session_cmd = sub.add_parser(
        "session",
        help="Session lifecycle helpers (prepare a usable+synced graph)",
    )
    session_sub = session_cmd.add_subparsers(dest="session_command")

    prepare_cmd = session_sub.add_parser(
        "prepare",
        help="Ensure a usable+synced knowledge graph for this repository",
    )
    prepare_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")
    prepare_cmd.add_argument(
        "--from-hook",
        action="store_true",
        help="Read the agent hook JSON payload on stdin to locate the repository",
    )
    prepare_cmd.add_argument(
        "--force",
        action="store_true",
        help="Refresh even when the graph already looks synced",
    )
    prepare_cmd.add_argument(
        "--budget-seconds",
        type=int,
        default=None,
        help=(
            "Wall-clock budget for prepare (default: 45 for hooks, or "
            "DAGAYN_SESSION_PREPARE_BUDGET_SECONDS). Use 0 for no limit."
        ),
    )
    prepare_cmd.add_argument(
        "--embedding",
        choices=["auto", "defer", "skip", "inline"],
        default="auto",
        dest="embedding_policy",
        help=(
            "When to run embedding refresh: auto (if budget remains), defer "
            "(mark pending), skip, or inline (ignore remaining-budget gate)"
        ),
    )
    prepare_cmd.add_argument(
        "--json",
        action="store_true",
        dest="as_json",
        help="Emit the prepare result as JSON",
    )
    prepare_cmd.add_argument(
        "--no-seed-worktree",
        action="store_true",
        help="Skip linked-worktree graph inheritance before prepare",
    )
    _add_local_embedding_args(prepare_cmd)

    return {"session": session_cmd}


def handle(args: argparse.Namespace, session_parser: argparse.ArgumentParser) -> None:
    """Handle session subcommands."""
    command = getattr(args, "session_command", None)
    if command != "prepare":
        session_parser.print_help()
        return

    from ...tools.session_prepare import session_prepare

    budget = getattr(args, "budget_seconds", None)
    if budget is not None and budget <= 0:
        budget = None

    result = session_prepare(
        repo_root=getattr(args, "repo", None),
        force=bool(getattr(args, "force", False)),
        local_embedding=getattr(args, "local_embedding", "none"),
        local_embedding_mode=getattr(args, "local_embedding_mode", None),
        local_embedding_port=getattr(args, "local_embedding_port", None),
        local_embedding_bin=getattr(args, "local_embedding_bin", DEFAULT_LOCAL_EMBEDDING_BIN),
        keep_local_embedding_server=getattr(args, "keep_local_embedding_server", False),
        local_embedding_timeout=getattr(args, "local_embedding_timeout", 300),
        local_embedding_request_timeout=getattr(args, "local_embedding_request_timeout", 60),
        local_embedding_batch_size=getattr(args, "local_embedding_batch_size", 1),
        budget_seconds=budget,
        embedding_policy=getattr(args, "embedding_policy", "auto"),
        from_hook=bool(getattr(args, "from_hook", False)),
        seed_worktree=not bool(getattr(args, "no_seed_worktree", False)),
    )

    if getattr(args, "as_json", False):
        print(json.dumps(result))
        return

    summary = result.get("summary") or "session prepare complete"
    print(summary)
    sync = result.get("sync") or {}
    if sync:
        print(
            f"Sync: {sync.get('status')} "
            f"(repo={sync.get('repo_root')}, "
            f"head={(sync.get('current_head_sha') or '')[:12]})"
        )
    phases = result.get("phases") or {}
    if phases:
        print(f"Phases: structure={phases.get('structure')} embedding={phases.get('embedding')}")
    if result.get("status") == "error":
        sys.exit(1)
