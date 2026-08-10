"""Graph sync assessment relative to the current VCS working tree.

Freshness is a state, not a pile of booleans, and it is decided in two tiers:

* **commit tier** — does the graph's ``git_head_sha`` equal HEAD? A graph built
  at another commit answers for the wrong tree (``commit_drift``).
* **diff tier** — once the commit tier agrees, are the uncommitted working-tree
  edits in the graph? ``worktree_behind`` when some are missing,
  ``worktree_ahead`` when the graph already describes them (an edit hook
  indexed them), ``commit_synced`` when the tree is clean.

``unbuilt`` precedes both tiers. See :data:`~dagayn.state_types.GraphSyncState`
for the discriminated union and ``docs/SESSION-GRAPH-FRESHNESS.md`` for how
each state maps onto the lifecycle use cases.
"""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Any

from ..incremental import _git_branch_info, detect_vcs, get_changed_file_sources
from ..state_types import GraphSyncStateName, seal_graph_sync_state

logger = logging.getLogger(__name__)

#: Diff-tier classification hashes every dirty candidate. A huge dirty set
#: (bulk checkout, generated tree) is reported ``worktree_behind`` without
#: hashing: prepare would re-parse it anyway, so the refinement buys nothing.
_MAX_DIFF_TIER_FILES = 200

#: Legacy 4-value status each state maps onto, for consumers written before
#: ``state`` existed (MCP clients, hook scripts, older docs).
LEGACY_STATUS_BY_STATE: dict[GraphSyncStateName, str] = {
    "unbuilt": "empty",
    "commit_drift": "git_drift",
    "commit_synced": "synced",
    "worktree_behind": "dirty_worktree",
    "worktree_ahead": "dirty_worktree",
}

#: Reverse mapping for dicts that only carry the legacy ``status``. The dirty
#: case cannot be told apart without the diff tier, so it degrades to the
#: conservative ``worktree_behind`` (prepare runs, as it did before).
_STATE_BY_LEGACY_STATUS: dict[str, GraphSyncStateName] = {
    "empty": "unbuilt",
    "git_drift": "commit_drift",
    "synced": "commit_synced",
    "dirty_worktree": "worktree_behind",
}

#: Structure is usable for analysis: the graph is HEAD-aligned.
_STRUCTURE_READY_STATES: frozenset[GraphSyncStateName] = frozenset(
    {"commit_synced", "worktree_behind", "worktree_ahead"}
)

#: Session-start / explicit prepare has work to do. ``worktree_ahead`` is
#: excluded on purpose: its edits are already indexed, so preparing again would
#: re-hash the same files on every session start.
_STRUCTURE_PREPARE_STATES: frozenset[GraphSyncStateName] = frozenset(
    {"unbuilt", "commit_drift", "worktree_behind"}
)

#: MCP first-tool auto_prepare only bootstraps when analysis would otherwise
#: run against a missing or wrong commit; a dirty tree is left to prepare/hooks.
_MCP_AUTO_PREPARE_STATES: frozenset[GraphSyncStateName] = frozenset({"unbuilt", "commit_drift"})


def _graph_is_empty(stats: Any) -> bool:
    return (
        int(getattr(stats, "total_nodes", 0) or 0) == 0
        or int(getattr(stats, "files_count", 0) or 0) == 0
    )


def sync_state(sync: dict[str, Any]) -> GraphSyncStateName | None:
    """Return the state name of an assessment, tolerating legacy dicts.

    Accepts payloads that only carry the pre-union ``status`` key so callers
    holding an older assessment (or a hand-built test fixture) keep working.
    """
    state = sync.get("state")
    if isinstance(state, str) and state in LEGACY_STATUS_BY_STATE:
        return state  # type: ignore[return-value]
    status = sync.get("status")
    if isinstance(status, str):
        return _STATE_BY_LEGACY_STATUS.get(status)
    return None


def _classify_diff_tier(
    store: Any,
    root: Path,
    dirty_files: list[str],
) -> tuple[GraphSyncStateName, list[str]]:
    """Split a dirty working tree into behind/ahead of the graph.

    Returns the state and the files that drove it: the not-yet-indexed files
    for ``worktree_behind``, the already-indexed ones for ``worktree_ahead``.
    Reuses the incremental pipeline's own filters so a file dagayn would never
    parse (ignored, binary, unsupported language) cannot pin the state to
    ``worktree_behind`` forever.
    """
    if len(dirty_files) > _MAX_DIFF_TIER_FILES:
        return "worktree_behind", dirty_files[:_MAX_DIFF_TIER_FILES]

    from ..incremental_build import (
        _classify_python_changed_files,
        _filter_incremental_candidates,
        _get_file_meta_for_candidates,
    )
    from ..incremental_files import _load_ignore_patterns

    try:
        candidates, removed = _filter_incremental_candidates(
            root,
            set(dirty_files),
            _load_ignore_patterns(root),
        )
        meta = _get_file_meta_for_candidates(store, candidates + removed)
        # A deleted file the graph still holds nodes for is unindexed work too.
        pending = [path for path in removed if path in meta]
        changed, _mtime_only = _classify_python_changed_files(root, candidates, meta)
        pending.extend(changed)
    except Exception as exc:  # noqa: BLE001 — assessment must never raise
        logger.debug("Diff-tier classification failed, assuming behind: %s", exc)
        return "worktree_behind", []

    if pending:
        return "worktree_behind", sorted(pending)
    return "worktree_ahead", sorted(candidates)


def assess_graph_sync(store: Any, repo_root: str | Path) -> dict[str, Any]:
    """Return the graph sync state for *repo_root* relative to its working tree.

    The payload is a sealed :data:`~dagayn.state_types.GraphSyncState`: a
    ``state`` discriminator plus the evidence behind it. ``status`` carries the
    legacy 4-value name for older consumers.
    """
    root = Path(repo_root)
    stats = store.get_stats()
    stored_sha = store.get_metadata("git_head_sha") or None
    last_updated = getattr(stats, "last_updated", None) or store.get_metadata("last_updated")

    current_branch = ""
    current_sha = ""
    dirty_files: list[str] = []
    vcs = detect_vcs(root)
    if vcs == "git":
        current_branch, current_sha = _git_branch_info(root)
        try:
            dirty_files = list(get_changed_file_sources(root, "HEAD").get("worktree") or [])
        except Exception:  # noqa: BLE001 — a status failure is not dirtiness
            dirty_files = []

    graph_empty = _graph_is_empty(stats)
    commit_drift = bool(vcs == "git" and current_sha and stored_sha != current_sha)
    undated = bool(not last_updated and not graph_empty)

    extra: dict[str, Any] = {}
    state: GraphSyncStateName
    if graph_empty:
        state = "unbuilt"
        dirty_files = []
    elif commit_drift or undated:
        state = "commit_drift"
    elif not dirty_files:
        state = "commit_synced"
    else:
        state, evidence = _classify_diff_tier(store, root, dirty_files)
        extra["pending_files" if state == "worktree_behind" else "indexed_files"] = evidence

    return seal_graph_sync_state(
        {
            "state": state,
            "status": LEGACY_STATUS_BY_STATE[state],
            "repo_root": str(root),
            "git_head_sha": stored_sha,
            "current_head_sha": current_sha or None,
            "current_branch": current_branch or None,
            "worktree_dirty": bool(dirty_files),
            "last_updated": last_updated,
            "total_nodes": int(getattr(stats, "total_nodes", 0) or 0),
            "files_count": int(getattr(stats, "files_count", 0) or 0),
            **extra,
        }
    )


def embedding_needs_refresh(db_path: str | Path, *, local_embedding: str | None) -> bool:
    """True when serve/install embedding mode is on but the index is incomplete."""
    from .build import _local_embedding_requested

    if not _local_embedding_requested(local_embedding):
        return False
    from ..embeddings_store import get_embedding_status

    status = get_embedding_status(db_path).get("status")
    return status in {"not_indexed", "empty", "partial", "stale", "unavailable"}


def needs_structure_prepare(sync: dict[str, Any], *, force: bool = False) -> bool:
    """True when Phase 1 (structure) should run.

    Includes ``worktree_behind`` so session-start / explicit prepare indexes
    uncommitted edits once, and excludes ``worktree_ahead`` because those edits
    are already in the graph.
    """
    if force:
        return True
    return sync_state(sync) in _STRUCTURE_PREPARE_STATES


def needs_mcp_auto_prepare(sync: dict[str, Any], *, force: bool = False) -> bool:
    """True when MCP first-tool auto_prepare should bootstrap the graph.

    Only ``unbuilt`` / ``commit_drift`` block analysis against the wrong or
    missing commit. A dirty tree is HEAD-aligned; ongoing dirty indexing is left
    to session-start prepare and edit hooks (``dagayn update --skip-flows``).
    """
    if force:
        return True
    return sync_state(sync) in _MCP_AUTO_PREPARE_STATES


def is_structure_ready(sync: dict[str, Any]) -> bool:
    """True when the graph is HEAD-aligned and usable for analysis.

    ``commit_synced``, ``worktree_behind`` and ``worktree_ahead`` all have a
    stored ``git_head_sha`` equal to HEAD. Uncommitted edits change how much
    the graph knows, not which commit it describes.
    """
    return sync_state(sync) in _STRUCTURE_READY_STATES
