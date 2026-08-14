"""worktree / hook-repo commands — argument registration and handlers.

``dagayn worktree sync`` makes a linked git worktree usable immediately: it
inherits the main checkout's graph and re-parses only the branch diff. Agent
hosts call it from a hook when a session enters a worktree (Claude Code's
``EnterWorktree`` tool, Cursor's ``sessionStart``).

``dagayn hook-repo`` prints the repository root a hook payload refers to. Hook
scripts use it because not every host runs hooks from the project directory —
Cursor user-level hooks run from ``~/.cursor``.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from ...paths import get_db_path
from ._shared import DEFAULT_LOCAL_EMBEDDING_BIN, _add_local_embedding_args


def register_commands(sub: argparse._SubParsersAction) -> dict:
    """Register the worktree and hook-repo subcommands."""
    worktree_cmd = sub.add_parser(
        "worktree",
        help="Git worktree support (inherit the graph, inspect worktree state)",
    )
    worktree_sub = worktree_cmd.add_subparsers(dest="worktree_command")

    sync_cmd = worktree_sub.add_parser(
        "sync",
        help="Inherit the main checkout's graph into this worktree and update it",
    )
    sync_cmd.add_argument("--repo", default=None, help="Worktree root (auto-detected)")
    sync_cmd.add_argument(
        "--from-hook",
        action="store_true",
        help="Read the agent hook JSON payload on stdin to locate the worktree",
    )
    sync_cmd.add_argument(
        "--base",
        default=None,
        help="Git diff base for the catch-up update (default: the inherited graph's commit)",
    )
    sync_cmd.add_argument(
        "--seed-only",
        action="store_true",
        help="Copy the graph without running the catch-up update",
    )
    sync_cmd.add_argument(
        "--no-copy-config",
        action="store_false",
        dest="copy_config",
        help="Do not copy the main checkout's gitignored MCP config into the worktree",
    )
    sync_cmd.add_argument(
        "--build-if-missing",
        action="store_true",
        help="Run a full build when the main checkout has no graph to inherit",
    )
    sync_cmd.add_argument(
        "--json",
        action="store_true",
        dest="as_json",
        help="Emit a JSON result object (for hook integrations)",
    )
    _add_local_embedding_args(sync_cmd)

    info_cmd = worktree_sub.add_parser("info", help="Show worktree and graph inheritance state")
    info_cmd.add_argument("--repo", default=None, help="Worktree root (auto-detected)")

    hook_repo_cmd = sub.add_parser(
        "hook-repo",
        help="Print the repository root an agent hook payload (stdin JSON) refers to",
    )
    hook_repo_cmd.add_argument(
        "--no-cwd-fallback",
        action="store_true",
        help="Fail instead of falling back to the current working directory",
    )

    return {"worktree": worktree_cmd, "hook-repo": hook_repo_cmd}


def _resolve_repo(args: argparse.Namespace) -> Path | None:
    """Resolve the target repository root for a worktree subcommand."""
    from ...worktree import parse_hook_payload, resolve_hook_repo

    if getattr(args, "repo", None):
        return Path(args.repo).expanduser()

    if getattr(args, "from_hook", False):
        payload = parse_hook_payload(sys.stdin.read() if not sys.stdin.isatty() else "")
        resolved = resolve_hook_repo(payload)
        if resolved is not None:
            return resolved

    from ...incremental import find_repo_root

    return find_repo_root()


def _print_json(payload: dict) -> None:
    import json

    print(json.dumps(payload))


def _handle_info(args: argparse.Namespace) -> None:
    from ...worktree import (
        git_hooks_dir,
        is_linked_worktree,
        main_worktree_root,
        seeding_disabled,
    )

    repo_root = _resolve_repo(args)
    if repo_root is None:
        print("Not inside a git repository.")
        return

    main = main_worktree_root(repo_root)
    linked = is_linked_worktree(repo_root)
    print(f"Working tree: {repo_root}")
    print(f"Main checkout: {main if main else 'unknown'}")
    print(f"Linked worktree: {'yes' if linked else 'no'}")
    hooks_dir = git_hooks_dir(repo_root)
    print(f"Git hooks dir: {hooks_dir if hooks_dir else 'unknown'}")

    graph = get_db_path(repo_root)
    print(f"Graph: {'present' if graph.exists() else 'missing'} ({graph})")
    if linked and main is not None:
        source = get_db_path(main)
        print(f"Inheritable graph: {'present' if source.exists() else 'missing'} ({source})")
        if seeding_disabled():
            print("Graph inheritance: disabled by DAGAYN_WORKTREE_SEED")


def _graph_head_sha(repo_root: Path) -> str | None:
    """Return the commit an existing graph in *repo_root* was built at."""
    from ...worktree import read_graph_metadata

    return read_graph_metadata(get_db_path(repo_root), "git_head_sha")


def _handle_sync(args: argparse.Namespace) -> None:
    from ...worktree import copy_worktree_config, graph_has_nodes, seed_worktree_graph

    as_json = getattr(args, "as_json", False)
    repo_root = _resolve_repo(args)
    if repo_root is None:
        message = "Not inside a git repository; nothing to sync."
        if as_json:
            _print_json({"status": "skipped", "reason": message})
        else:
            print(message)
        return

    copied: list[str] = []
    if getattr(args, "copy_config", True):
        copied = copy_worktree_config(repo_root)

    seed = seed_worktree_graph(repo_root)
    if not as_json:
        print(f"Worktree: {repo_root}")
        if copied:
            print(f"Copied config from the main checkout: {', '.join(copied)}")
        print(f"Graph inheritance: {seed.status} ({seed.reason})")

    # A schema-only stub left by ``dagayn status``/``serve`` is not a graph:
    # treating it as one picked the incremental path with base=HEAD~1, indexing
    # the last commit only and then stamping that partial graph as HEAD-synced.
    graph_exists = graph_has_nodes(get_db_path(repo_root))
    if not graph_exists and not getattr(args, "build_if_missing", False):
        result = {
            "status": seed.status,
            "reason": seed.reason,
            "repo_root": str(repo_root),
            "copied_config": copied,
            "updated": False,
        }
        if as_json:
            _print_json(result)
        else:
            print("No graph available — run 'dagayn build' in this worktree.")
        return

    if getattr(args, "seed_only", False):
        if as_json:
            _print_json(
                {
                    "status": seed.status,
                    "reason": seed.reason,
                    "repo_root": str(repo_root),
                    "copied_config": copied,
                    "updated": False,
                }
            )
        return

    full_rebuild = not graph_exists
    # Diff against the commit the graph actually describes: the inherited one
    # when we just seeded, otherwise whatever an earlier build recorded. HEAD~1
    # would silently skip commits when re-entering an existing worktree.
    base = args.base or seed.base_sha or _graph_head_sha(repo_root) or "HEAD~1"
    from ...tools.build import build_or_update_graph

    build_result = build_or_update_graph(
        full_rebuild=full_rebuild,
        repo_root=str(repo_root),
        base=base,
        postprocess="minimal",
        local_embedding=getattr(args, "local_embedding", "none"),
        local_embedding_mode=getattr(args, "local_embedding_mode", None),
        local_embedding_port=getattr(args, "local_embedding_port", None),
        local_embedding_bin=getattr(args, "local_embedding_bin", DEFAULT_LOCAL_EMBEDDING_BIN),
        keep_local_embedding_server=getattr(args, "keep_local_embedding_server", False),
        local_embedding_timeout=getattr(args, "local_embedding_timeout", 300),
        local_embedding_request_timeout=getattr(args, "local_embedding_request_timeout", 60),
        local_embedding_batch_size=getattr(args, "local_embedding_batch_size", 1),
    )

    updated = build_result.get("files_updated", build_result.get("files_parsed", 0))
    if as_json:
        _print_json(
            {
                "status": seed.status,
                "reason": seed.reason,
                "repo_root": str(repo_root),
                "copied_config": copied,
                "updated": True,
                "full_rebuild": full_rebuild,
                "base": base,
                "files": updated,
                "total_nodes": build_result.get("total_nodes", 0),
            }
        )
    else:
        kind = "Full build" if full_rebuild else f"Catch-up update (base={base[:12]})"
        print(f"{kind}: {updated} files, {build_result.get('total_nodes', 0)} nodes")


def handle(args: argparse.Namespace, worktree_parser: argparse.ArgumentParser) -> None:
    """Handle worktree subcommands."""
    import logging

    logging.basicConfig(level=logging.WARNING, format="%(levelname)s: %(message)s")

    command = getattr(args, "worktree_command", None)
    if not command:
        worktree_parser.print_help()
        return
    if command == "info":
        _handle_info(args)
    elif command == "sync":
        _handle_sync(args)


def handle_hook_repo(args: argparse.Namespace) -> None:
    """Print the repository root the hook payload on stdin refers to."""
    from ...worktree import parse_hook_payload, resolve_hook_repo

    raw = "" if sys.stdin.isatty() else sys.stdin.read()
    repo_root = resolve_hook_repo(
        parse_hook_payload(raw),
        fallback_cwd=not getattr(args, "no_cwd_fallback", False),
    )
    if repo_root is None:
        sys.exit(1)
    print(repo_root)
