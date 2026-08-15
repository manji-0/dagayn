"""wiki command — argument registration and handler."""

from __future__ import annotations

import argparse
import logging
from pathlib import Path


def register_command(sub: argparse._SubParsersAction) -> argparse.ArgumentParser:
    """Register the wiki subcommand. Returns the subparser."""
    wiki_cmd = sub.add_parser("wiki", help="Generate markdown wiki from community structure")
    wiki_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")
    wiki_cmd.add_argument(
        "--force",
        action="store_true",
        help="Regenerate all pages even if content unchanged",
    )
    return wiki_cmd


def handle(args: argparse.Namespace) -> None:
    """Generate markdown wiki from community structure."""
    logging.basicConfig(level=logging.INFO, format="%(levelname)s: %(message)s")

    from ...graph import GraphStore
    from ...incremental import find_project_root, get_data_dir, get_db_path

    repo_root = Path(args.repo) if args.repo else find_project_root()
    db_path = get_db_path(repo_root)
    from ...write_lock import graph_read_lock

    with graph_read_lock(db_path):
        store = GraphStore(db_path)

        try:
            from ...wiki import generate_wiki

            wiki_dir = get_data_dir(repo_root) / "wiki"
            result = generate_wiki(store, wiki_dir, force=args.force)
            total = result["pages_generated"] + result["pages_updated"] + result["pages_unchanged"]
            print(
                f"Wiki: {result['pages_generated']} new, "
                f"{result['pages_updated']} updated, "
                f"{result['pages_unchanged']} unchanged "
                f"({total} total pages)"
            )
            print(f"Output: {wiki_dir}")
        finally:
            store.close()
