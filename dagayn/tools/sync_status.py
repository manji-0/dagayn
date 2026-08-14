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

#: Diff-tier verification stats every indexed file (cheap) but only hashes the
#: ones whose mtime moved. Above this many hash candidates the verification is
#: abandoned rather than paid for: a freshly seeded worktree has a graph whose
#: stored mtimes all came from the main checkout, and re-hashing the whole tree
#: on every assessment would cost more than the state is worth.
_MAX_HASH_CANDIDATES = 200

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


def _indexed_file_meta(store: Any) -> dict[str, tuple[str, int]]:
    """Return ``{repo_relative_path: (file_hash, mtime_ns)}`` for indexed files."""
    getter = getattr(store, "get_file_meta_map", None)
    if not callable(getter):
        return {}
    return dict(getter() or {})


def _classify_diff_tier(
    store: Any,
    root: Path,
    dirty_files: list[str],
    *,
    max_hash_candidates: int | None = _MAX_HASH_CANDIDATES,
) -> tuple[GraphSyncStateName, list[str], dict[str, Any]]:
    """Compare the graph's indexed content with the working tree.

    Every indexed file is checked, not just the files git calls dirty: the
    graph may hold content that no longer exists on disk (an edit hook indexed
    an uncommitted change and the change was then discarded with
    ``git checkout --``). HEAD still matches and git reports a clean tree, so
    neither the commit tier nor a dirty-only diff tier would notice.

    The first pass is ``stat`` only — a rewritten file always moves its mtime —
    and bytes are hashed just for the files whose mtime moved. Reuses the
    incremental pipeline's own filters so a file dagayn would never parse
    (ignored, binary, unsupported language) cannot pin the state to
    ``worktree_behind`` forever.

    *max_hash_candidates* caps that second pass; ``None`` removes the cap for
    callers that can afford a full verification (session prepare, which is
    about to re-index anyway). When the cap bites, the returned evidence says
    so rather than letting the dirty-only answer pass as a verified one.

    Returns the state, the files that drove it (the files needing a re-index
    for ``worktree_behind``, the verified dirty files for ``worktree_ahead``),
    and the verification evidence.
    """
    from ..incremental_build import (
        _classify_python_changed_files,
        _filter_incremental_candidates,
    )
    from ..incremental_files import _load_ignore_patterns

    dirty_state: GraphSyncStateName = "worktree_ahead" if dirty_files else "commit_synced"
    try:
        indexed = _indexed_file_meta(store)

        # Pass 1 (stat only): indexed files that moved or vanished on disk.
        stale: list[str] = []
        suspect: list[str] = []
        for rel_path, (_stored_hash, stored_mtime_ns) in indexed.items():
            try:
                current_mtime_ns = int((root / rel_path).stat().st_mtime_ns)
            except OSError:
                stale.append(rel_path)
                continue
            if current_mtime_ns != stored_mtime_ns:
                suspect.append(rel_path)

        # Dirty files dagayn would index that the graph has never seen, and
        # dirty deletions it still holds nodes for.
        candidates, removed = _filter_incremental_candidates(
            root,
            set(dirty_files),
            _load_ignore_patterns(root),
        )
        stale.extend(path for path in removed if path in indexed)
        unseen = [path for path in candidates if path not in indexed]

        to_hash = sorted(set(suspect) | set(unseen))
        if max_hash_candidates is not None and len(to_hash) > max_hash_candidates:
            # Too much to verify cheaply (typically a just-seeded worktree,
            # whose stored mtimes all came from the main checkout). Fall back to
            # the dirty-only answer rather than paying for a full re-hash or
            # claiming a drift that probably is not there -- but say that the
            # content was never checked, because a fresh checkout of any real
            # repository always lands here.
            logger.debug(
                "Skipping content verification: %d hash candidates exceed the %d limit",
                len(to_hash),
                max_hash_candidates,
            )
            return (
                dirty_state,
                sorted(candidates),
                {"content_verified": False, "unverified_file_count": len(to_hash)},
            )

        # Pass 2 (hash): mtime moved, but the bytes may still be identical.
        changed, _mtime_only = _classify_python_changed_files(root, to_hash, indexed)
        pending = sorted(set(changed) | set(stale))
    except Exception as exc:  # noqa: BLE001 — assessment must never raise
        logger.debug("Diff-tier classification failed, assuming behind: %s", exc)
        return "worktree_behind", [], {"content_verified": False}

    if pending:
        return "worktree_behind", pending, {}
    return dirty_state, sorted(candidates), {}


def commit_tier_freshness(store: Any, repo_root: str | Path) -> dict[str, Any]:
    """Return the cheap half of the freshness assessment.

    Only the commit tier plus git's own dirty flag: two ``git`` invocations and
    one metadata read, with none of :func:`_classify_diff_tier`'s per-file
    ``stat``/hash work. Read tools call this on every response, so it has to
    stay O(1) in repository size -- the price of the full assessment is only
    worth paying where a re-index may follow.
    """
    root = Path(repo_root)
    if detect_vcs(root) != "git":
        return {"state": None}
    stored_sha = store.get_metadata("git_head_sha") or None
    _branch, current_sha = _git_branch_info(root)
    if not current_sha:
        return {"state": None}
    try:
        dirty = bool(get_changed_file_sources(root, "HEAD").get("worktree") or [])
    except Exception:  # noqa: BLE001 — a status failure is not dirtiness
        dirty = False
    state: GraphSyncStateName = "commit_synced" if stored_sha == current_sha else "commit_drift"
    return {
        "state": state,
        "git_head_sha": stored_sha,
        "current_head_sha": current_sha,
        "worktree_dirty": dirty,
    }


def _seed_needs_verification(store: Any) -> bool:
    """True when this graph was seeded from another checkout and not yet checked."""
    try:
        from ..worktree import SEEDED_NEEDS_VERIFY_KEY

        return str(store.get_metadata(SEEDED_NEEDS_VERIFY_KEY) or "") == "1"
    except Exception:  # noqa: BLE001 — assessment must never raise
        return False


def _clear_seed_verification_flag(store: Any) -> None:
    """Drop the seed marker once content has been verified."""
    try:
        from ..worktree import SEEDED_NEEDS_VERIFY_KEY

        setter = getattr(store, "set_metadata", None)
        if callable(setter):
            setter(SEEDED_NEEDS_VERIFY_KEY, "0")
            commit = getattr(store, "commit", None)
            if callable(commit):
                commit()
    except Exception:  # noqa: BLE001 — best effort; a stale flag only costs a re-verify
        logger.debug("Could not clear the seed verification flag", exc_info=True)


def assess_graph_sync(
    store: Any,
    repo_root: str | Path,
    *,
    max_hash_candidates: int | None = _MAX_HASH_CANDIDATES,
) -> dict[str, Any]:
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

    if _seed_needs_verification(store):
        # A seeded worktree graph carries the parent's per-file mtimes, so a
        # fresh checkout always exceeds the hash-candidate cap and verification
        # would be skipped exactly where it matters most: the copy may hold
        # nodes an edit hook indexed from the parent's uncommitted files, and the
        # catch-up diff cannot see them. Pay for the one full verification.
        max_hash_candidates = None

    extra: dict[str, Any] = {}
    state: GraphSyncStateName
    if graph_empty:
        state = "unbuilt"
        dirty_files = []
    elif commit_drift or undated:
        state = "commit_drift"
    else:
        state, evidence, verification = _classify_diff_tier(
            store,
            root,
            dirty_files,
            max_hash_candidates=max_hash_candidates,
        )
        if state == "worktree_behind":
            extra["pending_files"] = evidence
        elif state == "worktree_ahead":
            extra["indexed_files"] = evidence
        extra.update(verification)
        if verification.get("content_verified") is not False:
            # Verified once; the stored mtimes now describe this worktree.
            _clear_seed_verification_flag(store)

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
