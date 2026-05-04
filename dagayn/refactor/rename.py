"""Rename preview for graph-powered refactoring."""

from __future__ import annotations

import logging
import time
import uuid
from typing import Any, Optional

from ..graph import GraphStore, _sanitize_name
from .pending import _cleanup_expired, _pending_refactors, _refactor_lock

logger = logging.getLogger(__name__)


def rename_preview(
    store: GraphStore,
    old_name: str,
    new_name: str,
) -> Optional[dict[str, Any]]:
    """Build a rename edit list for *old_name* -> *new_name*.

    Finds the node via ``store.search_nodes(old_name)``, collects
    definition and reference sites, generates a unique ``refactor_id``,
    and stores the preview in the thread-safe ``_pending_refactors`` dict.

    Returns:
        A refactor preview dict, or ``None`` if the node is not found.
    """
    candidates = store.search_nodes(old_name, limit=10)
    node = None
    for c in candidates:
        if c.name == old_name:
            node = c
            break
    if node is None and candidates:
        node = candidates[0]
    if node is None:
        logger.warning("rename_preview: node %r not found", old_name)
        return None
    exact_candidates = [c for c in candidates if c.name == old_name]

    edits: list[dict[str, Any]] = []

    edits.append(
        {
            "file": node.file_path,
            "line": node.line_start,
            "old": old_name,
            "new": new_name,
            "confidence": "high",
            "source": "definition",
        }
    )

    call_edges = store.get_edges_by_target(node.qualified_name)
    for edge in call_edges:
        if edge.kind == "CALLS":
            edits.append(
                {
                    "file": edge.file_path,
                    "line": edge.line,
                    "old": old_name,
                    "new": new_name,
                    "confidence": "high",
                    "source": "call",
                    "edge_kind": edge.kind,
                }
            )

    bare_edges = store.search_edges_by_target_name(old_name, kind="CALLS")
    seen = {(e["file"], e["line"]) for e in edits}
    for edge in bare_edges:
        key = (edge.file_path, edge.line)
        if key not in seen:
            edits.append(
                {
                    "file": edge.file_path,
                    "line": edge.line,
                    "old": old_name,
                    "new": new_name,
                    "confidence": "medium",
                    "source": "bare_call",
                    "edge_kind": edge.kind,
                }
            )
            seen.add(key)

    import_edges = store.get_edges_by_target(node.qualified_name)
    for edge in import_edges:
        if edge.kind == "IMPORTS_FROM":
            key = (edge.file_path, edge.line)
            if key not in seen:
                edits.append(
                    {
                        "file": edge.file_path,
                        "line": edge.line,
                        "old": old_name,
                        "new": new_name,
                        "confidence": "high",
                        "source": "import",
                        "edge_kind": edge.kind,
                    }
                )
                seen.add(key)

    stats = {"high": 0, "medium": 0, "low": 0}
    for e in edits:
        stats[e["confidence"]] += 1

    refactor_id = uuid.uuid4().hex[:8]
    preview: dict[str, Any] = {
        "refactor_id": refactor_id,
        "type": "rename",
        "old_name": _sanitize_name(old_name),
        "new_name": _sanitize_name(new_name),
        "target": {
            "name": _sanitize_name(node.name),
            "qualified_name": _sanitize_name(node.qualified_name),
            "kind": node.kind,
            "file": node.file_path,
            "line": node.line_start,
            "language": node.language,
        },
        "ambiguous": len(exact_candidates) > 1,
        "candidate_count": len(exact_candidates),
        "candidates": [
            {
                "qualified_name": _sanitize_name(c.qualified_name),
                "kind": c.kind,
                "file": c.file_path,
                "line": c.line_start,
                "language": c.language,
            }
            for c in exact_candidates
        ],
        "edits": edits,
        "stats": stats,
        "created_at": time.time(),
        "warnings": (
            ["Multiple exact symbol matches were found; preview uses the first match."]
            if len(exact_candidates) > 1
            else []
        ),
    }

    with _refactor_lock:
        _cleanup_expired()
        _pending_refactors[refactor_id] = preview

    logger.info(
        "rename_preview: created refactor %s (%s -> %s, %d edits)",
        refactor_id,
        old_name,
        new_name,
        len(edits),
    )
    return preview
