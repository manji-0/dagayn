"""Rename preview for graph-powered refactoring."""

from __future__ import annotations

import logging
import re
import time
import uuid
from pathlib import Path
from typing import Any, Optional

from ..graph import GraphStore, _sanitize_name
from ..graph.types import GraphEdge
from ..state_types import seal_missingness_item
from .pending import _cleanup_expired, _pending_refactors, _refactor_lock

logger = logging.getLogger(__name__)

type RenameValue = Any
type RenamePayload = dict[str, RenameValue]

_RENAME_GRAPH_LIMITED_MISSINGNESS = seal_missingness_item(
    {
        "reason_code": "rename_edits_graph_limited",
        "severity": "medium",
        "claim_effect": (
            "edit list covers graph-known call, reference, and import sites only; "
            "string-based access, getattr, re-exports, generated code, and "
            "non-code files are not included"
        ),
    }
)


def _import_statement_mentions_symbol(
    store: GraphStore,
    edge: GraphEdge,
    symbol_name: str,
) -> bool:
    """Return True when an IMPORTS_FROM edge plausibly imports *symbol_name*."""
    if edge.target_qualified == symbol_name or edge.target_qualified.endswith(f"::{symbol_name}"):
        return True

    paths_to_try: list[Path] = []
    file_path = Path(edge.file_path)
    if file_path.is_absolute():
        paths_to_try.append(file_path)
    repo_root = store.get_repo_root()
    if repo_root is not None:
        paths_to_try.append(repo_root / file_path)

    pattern = re.compile(rf"\b{re.escape(symbol_name)}\b")
    for path in paths_to_try:
        try:
            lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
        except OSError:
            continue
        idx = edge.line - 1
        if 0 <= idx < len(lines):
            return bool(pattern.search(lines[idx]))

    return False


def _append_edit(
    edits: list[RenamePayload],
    seen: set[tuple[str, int]],
    *,
    file: str,
    line: int,
    old: str,
    new: str,
    confidence: str,
    source: str,
    edge_kind: str | None = None,
) -> None:
    key = (file, line)
    if key in seen:
        return
    edit: RenamePayload = {
        "file": file,
        "line": line,
        "old": old,
        "new": new,
        "confidence": confidence,
        "source": source,
    }
    if edge_kind is not None:
        edit["edge_kind"] = edge_kind
    edits.append(edit)
    seen.add(key)


def rename_preview(
    store: GraphStore,
    old_name: str,
    new_name: str,
) -> Optional[RenamePayload]:
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

    edits: list[RenamePayload] = []
    seen: set[tuple[str, int]] = set()

    _append_edit(
        edits,
        seen,
        file=node.file_path,
        line=node.line_start,
        old=old_name,
        new=new_name,
        confidence="high",
        source="definition",
    )

    for edge in store.get_edges_by_target(node.qualified_name):
        if edge.kind == "CALLS":
            _append_edit(
                edits,
                seen,
                file=edge.file_path,
                line=edge.line,
                old=old_name,
                new=new_name,
                confidence="high",
                source="call",
                edge_kind=edge.kind,
            )
        elif edge.kind == "REFERENCES":
            _append_edit(
                edits,
                seen,
                file=edge.file_path,
                line=edge.line,
                old=old_name,
                new=new_name,
                confidence="high",
                source="reference",
                edge_kind=edge.kind,
            )

    for edge in store.search_edges_by_target_name(old_name, kind="CALLS"):
        _append_edit(
            edits,
            seen,
            file=edge.file_path,
            line=edge.line,
            old=old_name,
            new=new_name,
            confidence="medium",
            source="bare_call",
            edge_kind=edge.kind,
        )

    for edge in store.search_import_edges_for_symbol(node.file_path, old_name):
        if not _import_statement_mentions_symbol(store, edge, old_name):
            continue
        _append_edit(
            edits,
            seen,
            file=edge.file_path,
            line=edge.line,
            old=old_name,
            new=new_name,
            confidence="high",
            source="import",
            edge_kind=edge.kind,
        )

    for edge in store.search_edges_by_target_name(old_name, kind="IMPORTS_FROM"):
        if not _import_statement_mentions_symbol(store, edge, old_name):
            continue
        _append_edit(
            edits,
            seen,
            file=edge.file_path,
            line=edge.line,
            old=old_name,
            new=new_name,
            confidence="medium",
            source="import",
            edge_kind=edge.kind,
        )

    stats = {"high": 0, "medium": 0, "low": 0}
    for e in edits:
        stats[e["confidence"]] += 1

    refactor_id = uuid.uuid4().hex[:8]
    preview: RenamePayload = {
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
        "missingness": [_RENAME_GRAPH_LIMITED_MISSINGNESS],
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
