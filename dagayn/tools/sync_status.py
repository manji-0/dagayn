"""Graph sync assessment relative to the current VCS working tree."""

from __future__ import annotations

from pathlib import Path
from typing import Any

from ..incremental import _git_branch_info, detect_vcs, get_changed_file_sources


def _graph_is_empty(stats: Any) -> bool:
    return (
        int(getattr(stats, "total_nodes", 0) or 0) == 0
        or int(getattr(stats, "files_count", 0) or 0) == 0
    )


def assess_graph_sync(store: Any, repo_root: str | Path) -> dict[str, Any]:
    """Return sync status for *repo_root*'s graph relative to the working tree.

    Status values:
    - ``empty`` — no nodes/files in the graph
    - ``git_drift`` — stored ``git_head_sha`` differs from current HEAD (or missing)
    - ``dirty_worktree`` — HEAD matches but staged/unstaged/untracked files exist
    - ``synced`` — non-empty, HEAD matches, clean worktree relative to HEAD
    """
    root = Path(repo_root)
    stats = store.get_stats()
    stored_sha = store.get_metadata("git_head_sha") or None
    last_updated = getattr(stats, "last_updated", None) or store.get_metadata("last_updated")

    current_branch = ""
    current_sha = ""
    vcs = detect_vcs(root)
    if vcs == "git":
        current_branch, current_sha = _git_branch_info(root)

    worktree_dirty = False
    if vcs == "git":
        try:
            worktree_dirty = bool(get_changed_file_sources(root, "HEAD").get("worktree"))
        except Exception:
            worktree_dirty = False

    if _graph_is_empty(stats):
        status = "empty"
    elif vcs == "git" and current_sha and stored_sha and stored_sha != current_sha:
        status = "git_drift"
    elif vcs == "git" and current_sha and not stored_sha:
        status = "git_drift"
    elif not last_updated and not _graph_is_empty(stats):
        status = "git_drift"
    elif worktree_dirty:
        status = "dirty_worktree"
    else:
        status = "synced"

    return {
        "status": status,
        "repo_root": str(root),
        "git_head_sha": stored_sha,
        "current_head_sha": current_sha or None,
        "current_branch": current_branch or None,
        "worktree_dirty": worktree_dirty,
        "last_updated": last_updated,
        "total_nodes": int(getattr(stats, "total_nodes", 0) or 0),
        "files_count": int(getattr(stats, "files_count", 0) or 0),
    }


def embedding_needs_refresh(db_path: str | Path, *, local_embedding: str | None) -> bool:
    """True when serve/install embedding mode is on but the index is incomplete."""
    from .build import _local_embedding_requested

    if not _local_embedding_requested(local_embedding):
        return False
    from ..embeddings_store import get_embedding_status

    status = get_embedding_status(db_path).get("status")
    return status in {"not_indexed", "empty", "partial", "stale", "unavailable"}


def needs_structure_prepare(sync: dict[str, Any], *, force: bool = False) -> bool:
    """True when Phase 1 (structure) should run."""
    if force:
        return True
    return sync.get("status") in {"empty", "git_drift", "dirty_worktree"}
