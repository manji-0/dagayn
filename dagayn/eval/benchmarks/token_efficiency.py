"""Token efficiency benchmark: compares naive, standard, and graph-based token counts."""

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import Any

from dagayn.eval.git_utils import ensure_parent_available, run_git
from dagayn.eval.token_counter import count_tokens

logger = logging.getLogger(__name__)

type BenchmarkValue = Any
type BenchmarkPayload = dict[str, BenchmarkValue]


def _count_tokens(text: str) -> int:
    """Count tokens and keep the old private helper shape for callers/tests."""
    return count_tokens(text)[0]


def _get_changed_files(repo_path: Path, sha: str) -> list[str]:
    """Get list of changed files for a commit."""
    result = run_git(["diff", "--name-only", f"{sha}~1", sha], cwd=repo_path)
    return [f.strip() for f in result.stdout.strip().splitlines() if f.strip()]


def _count_file_tokens(repo_path: Path, files: list[str]) -> int:
    """Count tokens from full file contents (naive approach)."""
    total = 0
    for f in files:
        fp = repo_path / f
        if fp.is_file():
            try:
                total += count_tokens(fp.read_text(encoding="utf-8", errors="replace"))[0]
            except OSError:
                pass
    return total


def _count_diff_tokens(repo_path: Path, sha: str) -> int:
    """Count tokens from git diff output (standard approach)."""
    result = run_git(["diff", f"{sha}~1", sha], cwd=repo_path)
    return count_tokens(result.stdout)[0]


def run(repo_path: Path, store: Any, config: BenchmarkPayload) -> list[BenchmarkPayload]:
    """Run token efficiency benchmark."""
    results: list[BenchmarkPayload] = []
    for tc in config.get("test_commits", []):
        sha = str(tc["sha"])
        base = {
            "benchmark": "token_efficiency",
            "repo": config["name"],
            "commit": sha,
            "resolved_commit": config.get("resolved_commit", ""),
            "description": tc.get("description", ""),
        }
        try:
            ensure_parent_available(repo_path, sha)
            changed = _get_changed_files(repo_path, sha)
        except Exception as exc:
            results.append({**base, "status": "error", "error": str(exc)})
            continue
        if not changed:
            results.append({**base, "status": "skipped", "error": "no changed files"})
            continue

        naive_tokens = _count_file_tokens(repo_path, changed)
        diff_tokens = _count_diff_tokens(repo_path, sha)

        # Graph-based: use get_review_context
        try:
            from dagayn.tools import get_review_context

            ctx = get_review_context(changed_files=changed, repo_root=str(repo_path))
            graph_tokens, token_counter = count_tokens(json.dumps(ctx))
        except Exception as exc:
            logger.warning("get_review_context failed: %s", exc)
            _, token_counter = count_tokens("")
            results.append(
                {
                    **base,
                    "status": "error",
                    "error": str(exc),
                    "changed_files": len(changed),
                    "naive_changed_file_tokens": naive_tokens,
                    "diff_tokens": diff_tokens,
                    "token_counter": token_counter,
                }
            )
            continue

        results.append(
            {
                **base,
                "status": "ok",
                "changed_files": len(changed),
                "naive_changed_file_tokens": naive_tokens,
                "diff_tokens": diff_tokens,
                "graph_context_tokens": graph_tokens,
                "naive_tokens": naive_tokens,
                "standard_tokens": diff_tokens,
                "graph_tokens": graph_tokens,
                "changed_file_to_graph_ratio": round(naive_tokens / max(graph_tokens, 1), 1),
                "diff_to_graph_ratio": round(diff_tokens / max(graph_tokens, 1), 1),
                "naive_to_graph_ratio": round(naive_tokens / max(graph_tokens, 1), 1),
                "standard_to_graph_ratio": round(diff_tokens / max(graph_tokens, 1), 1),
                "token_counter": token_counter,
            }
        )
    return results
