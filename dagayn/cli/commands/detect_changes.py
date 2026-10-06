"""detect-changes command — argument registration and handler."""

from __future__ import annotations

import argparse
import json
import logging
import sys
from pathlib import Path
from typing import Any


def register_command(sub: argparse._SubParsersAction) -> argparse.ArgumentParser:
    """Register the detect-changes subcommand. Returns the subparser."""
    detect_cmd = sub.add_parser("detect-changes", help="Analyze change impact")
    detect_cmd.add_argument(
        "--base",
        default=None,
        help=(
            "Git diff base (default: HEAD while tracked files have uncommitted "
            "changes, else HEAD~1)"
        ),
    )
    detect_cmd.add_argument(
        "--brief", action="store_true", help="Show the summary and one line per finding"
    )
    detect_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")
    return detect_cmd


def _finding_line(finding: dict[str, Any]) -> str:
    """One line for a ``review_tool`` finding: kind, place, claim."""
    place = finding.get("qualified_name") or finding.get("file") or ""
    line = f"  - {finding.get('kind')}: {place}"
    if finding.get("claim"):
        line += f" — {finding['claim']}"
    if finding.get("command"):
        line += f" ($ {finding['command']})"
    return line


def handle(args: argparse.Namespace) -> None:
    """Report what the change needs checking beyond the diff (``review_tool``)."""
    logging.basicConfig(level=logging.INFO, format="%(levelname)s: %(message)s")

    from ...incremental_files import find_repo_root
    from ...tools.review_dispatcher import review_func

    repo_root = Path(args.repo) if args.repo else find_repo_root()
    if not repo_root:
        logging.error("Not in a git repository. 'detect-changes' requires git for diffing.")
        logging.error("Use 'build' for a full parse, or run 'git init' first.")
        sys.exit(1)

    result = review_func(
        mode="changes",
        base=args.base,
        repo_root=str(repo_root),
        detail_level="minimal" if args.brief else "standard",
    )
    if not args.brief:
        print(json.dumps(result, indent=2, default=str))
        return
    print(result.get("summary") or "No summary available.")
    for finding in result.get("findings") or []:
        print(_finding_line(finding))
    omitted = result.get("findings_omitted") or {}
    for kind, count in omitted.items():
        print(f"  ... {count} more {kind}")
