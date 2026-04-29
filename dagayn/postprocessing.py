"""Shared post-build processing pipeline.

After the core Tree-sitter parse (full_build or incremental_update), four
post-processing steps must run to populate derived tables:

1. Compute node signatures
2. Rebuild FTS5 search index
3. Trace execution flows
4. Detect code communities

This module extracts that pipeline so every entry point — MCP tool, CLI
commands, and watch mode — produces identical results.
"""

from __future__ import annotations

import logging
import sqlite3
from typing import Any

from .graph import GraphStore

logger = logging.getLogger(__name__)


def run_post_processing(store: GraphStore) -> dict[str, Any]:
    """Run all post-build steps on a populated graph.

    Each step is non-fatal: failures are logged and collected as warnings
    so the primary build result is never lost.

    Args:
        store: An open GraphStore with nodes and edges already populated.

    Returns:
        Dict with keys for each step's result count and a ``warnings``
        list (only present when at least one step failed).
    """
    result: dict[str, Any] = {}
    warnings: list[str] = []

    _compute_signatures(store, result, warnings)
    _rebuild_fts_index(store, result, warnings)
    _resolve_markdown_artifact_refs(store, result, warnings)
    _trace_flows(store, result, warnings)
    _detect_communities(store, result, warnings)

    if warnings:
        result["warnings"] = warnings
    return result


# -- Individual steps (private) ------------------------------------------


def _resolve_markdown_artifact_refs(
    store: GraphStore,
    result: dict[str, Any],
    warnings: list[str],
) -> None:
    """Resolve unresolved Markdown → code CROSS_ARTIFACT edges.

    Edges emitted by the Markdown parser carry a placeholder target
    ``<unresolved:{name}>`` and ``extra.unresolved_target_name``.  This step
    looks each name up in the nodes table:

    - Unique match → rewrite target to the node's qualified_name; promote
      confidence to HIGH (0.8).
    - Zero or multiple matches → delete the edge (strict / HIGH-only policy).
    """
    import json

    resolved = 0
    dropped = 0
    try:
        rows = store._conn.execute(
            "SELECT id, extra "
            "FROM edges "
            "WHERE kind='CROSS_ARTIFACT' "
            "AND extra LIKE '%unresolved_target_name%'"
        ).fetchall()

        if not rows:
            result["markdown_artifact_refs_resolved"] = 0
            result["markdown_artifact_refs_dropped"] = 0
            return

        # Parse extras and collect unique symbol names in one pass
        edge_data: list[tuple[int, str, dict]] = []  # (edge_id, sym, extra_dict)
        symbols: set[str] = set()
        for row in rows:
            try:
                extra = json.loads(row["extra"] or "{}")
            except (json.JSONDecodeError, TypeError):
                continue
            sym = extra.get("unresolved_target_name")
            if not sym:
                continue
            edge_data.append((row["id"], sym, extra))
            symbols.add(sym)

        if not edge_data:
            result["markdown_artifact_refs_resolved"] = 0
            result["markdown_artifact_refs_dropped"] = 0
            return

        # Batch-fetch node matches for all unique symbols (1 query per 450 symbols)
        batch_size = 450
        sym_list = list(symbols)
        matches_by_sym: dict[str, list[tuple[str, str]]] = {}
        for i in range(0, len(sym_list), batch_size):
            chunk = sym_list[i : i + batch_size]
            placeholders = ",".join("?" for _ in chunk)
            match_rows = store._conn.execute(  # nosec B608
                f"SELECT name, qualified_name, language FROM nodes "
                f"WHERE name IN ({placeholders}) AND language != 'markdown'",
                chunk,
            ).fetchall()
            for mr in match_rows:
                matches_by_sym.setdefault(mr["name"], []).append(
                    (mr["qualified_name"], mr["language"] or "unknown")
                )

        # Classify edges into updates vs deletes
        to_update: list[tuple] = []  # (new_target, new_extra_json, confidence, tier, edge_id)
        to_delete: list[int] = []
        for edge_id, sym, extra in edge_data:
            matches = matches_by_sym.get(sym, [])
            if len(matches) == 1:
                qname, lang = matches[0]
                new_extra = dict(extra)
                new_extra.pop("unresolved_target_name", None)
                new_extra["target_language"] = lang
                new_extra["confidence"] = 0.8
                new_extra["confidence_tier"] = "HIGH"
                to_update.append((qname, json.dumps(new_extra), 0.8, "HIGH", edge_id))
                resolved += 1
            else:
                to_delete.append(edge_id)
                dropped += 1

        if to_update:
            store._conn.executemany(
                "UPDATE edges "
                "SET target_qualified=?, extra=?, confidence=?, confidence_tier=? "
                "WHERE id=?",
                to_update,
            )
        if to_delete:
            for i in range(0, len(to_delete), batch_size):
                chunk = to_delete[i : i + batch_size]
                placeholders = ",".join("?" for _ in chunk)
                store._conn.execute(  # nosec B608
                    f"DELETE FROM edges WHERE id IN ({placeholders})",
                    chunk,
                )

        store.commit()
        result["markdown_artifact_refs_resolved"] = resolved
        result["markdown_artifact_refs_dropped"] = dropped
    except sqlite3.OperationalError as e:
        logger.warning("Markdown artifact ref resolution failed: %s", e)
        warnings.append(f"Markdown artifact ref resolution failed: {type(e).__name__}: {e}")


def _compute_signatures(
    store: GraphStore,
    result: dict[str, Any],
    warnings: list[str],
) -> None:
    """Compute human-readable signatures for nodes that lack one."""
    try:
        rows = store.get_nodes_without_signature()
        for row in rows:
            node_id, name, kind, params, ret = (
                row[0],
                row[1],
                row[2],
                row[3],
                row[4],
            )
            if kind in ("Function", "Test"):
                sig = f"def {name}({params or ''})"
                if ret:
                    sig += f" -> {ret}"
            elif kind == "Class":
                sig = f"class {name}"
            else:
                sig = name
            store.update_node_signature(node_id, sig[:512])
        store.commit()
        result["signatures_computed"] = len(rows)
    except (sqlite3.OperationalError, TypeError, KeyError) as e:
        logger.warning("Signature computation failed: %s", e)
        warnings.append(f"Signature computation failed: {type(e).__name__}: {e}")


def _rebuild_fts_index(
    store: GraphStore,
    result: dict[str, Any],
    warnings: list[str],
) -> None:
    """Rebuild the FTS5 full-text search index."""
    try:
        from .search import rebuild_fts_index

        fts_count = rebuild_fts_index(store)
        result["fts_indexed"] = fts_count
    except (sqlite3.OperationalError, ImportError) as e:
        logger.warning("FTS index rebuild failed: %s", e)
        warnings.append(f"FTS index rebuild failed: {type(e).__name__}: {e}")


def _trace_flows(
    store: GraphStore,
    result: dict[str, Any],
    warnings: list[str],
) -> None:
    """Trace execution flows from entry points."""
    try:
        from .flows import store_flows, trace_flows

        flows = trace_flows(store)
        count = store_flows(store, flows)
        result["flows_detected"] = count
    except (sqlite3.OperationalError, ImportError) as e:
        logger.warning("Flow detection failed: %s", e)
        warnings.append(f"Flow detection failed: {type(e).__name__}: {e}")


def _detect_communities(
    store: GraphStore,
    result: dict[str, Any],
    warnings: list[str],
) -> None:
    """Detect code communities via Leiden algorithm or file grouping."""
    try:
        from .communities import detect_communities, store_communities

        comms = detect_communities(store)
        count = store_communities(store, comms)
        result["communities_detected"] = count
    except (sqlite3.OperationalError, ImportError) as e:
        logger.warning("Community detection failed: %s", e)
        warnings.append(f"Community detection failed: {type(e).__name__}: {e}")
