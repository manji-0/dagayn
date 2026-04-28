"""detect-changes command — argument registration and handler."""

from __future__ import annotations

import argparse
import json
import logging
import sys
from pathlib import Path


def register_command(sub: argparse._SubParsersAction) -> argparse.ArgumentParser:
    """Register the detect-changes subcommand. Returns the subparser."""
    detect_cmd = sub.add_parser("detect-changes", help="Analyze change impact")
    detect_cmd.add_argument("--base", default="HEAD~1", help="Git diff base (default: HEAD~1)")
    detect_cmd.add_argument("--brief", action="store_true", help="Show brief summary only")
    detect_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")
    return detect_cmd


def handle(args: argparse.Namespace) -> None:
    """Analyze change impact."""
    logging.basicConfig(level=logging.INFO, format="%(levelname)s: %(message)s")

    from ...graph import GraphStore
    from ...incremental import find_repo_root, get_db_path

    repo_root = Path(args.repo) if args.repo else find_repo_root()
    if not repo_root:
        logging.error("Not in a git repository. 'detect-changes' requires git for diffing.")
        logging.error("Use 'build' for a full parse, or run 'git init' first.")
        sys.exit(1)

    db_path = get_db_path(repo_root)
    store = GraphStore(db_path)

    try:
        from ...changes import analyze_changes
        from ...incremental import get_changed_files, get_staged_and_unstaged

        base = args.base
        changed = get_changed_files(repo_root, base)
        if not changed:
            changed = get_staged_and_unstaged(repo_root)

        if not changed:
            print("No changes detected.")
        else:
            result = analyze_changes(
                store,
                changed,
                repo_root=str(repo_root),
                base=base,
            )
            if args.brief:
                print(result.get("summary", "No summary available."))
            else:
                print(json.dumps(result, indent=2, default=str))
    finally:
        store.close()
