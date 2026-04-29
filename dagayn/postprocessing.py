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
    """Idempotently resolve/update all Markdown→code CROSS_ARTIFACT edges.

    Every CROSS_ARTIFACT edge emitted by the Markdown parser carries
    ``extra.original_symbol_name`` — the raw backtick span symbol.  This
    step runs on every postprocess call and brings each edge in line with
    the *current* state of the nodes table:

    - Unique non-markdown match → ``target_qualified`` = the node's
      ``qualified_name``; confidence promoted to HIGH (0.8).
    - Zero or 2+ matches → ``target_qualified`` = ``<unresolved:{sym}>``;
      confidence stays LOW (0.2).  The edge is **kept** so a future run
      can resolve it once the symbol situation changes.

    Edges are never deleted here; they persist until the source MD file is
    re-parsed (which replaces them).  This makes the resolver idempotent:
    running it twice in a row produces the same DB state.

    Result keys:
      ``markdown_artifact_refs_resolved``   — transitions unresolved→resolved
      ``markdown_artifact_refs_dropped``    — transitions resolved→unresolved (demoted)
      ``markdown_artifact_refs_re_resolved`` — resolved but to a different qname
      ``markdown_artifact_refs_still_unresolved`` — unchanged unresolved count
    """
    resolved = 0
    demoted = 0
    re_resolved = 0
    still_unresolved = 0

    try:
        rust_resolve = getattr(store, "resolve_markdown_artifact_refs", None)
        if callable(rust_resolve):
            rust_result = rust_resolve()
            resolved, dropped = rust_result[:2]
            result["markdown_artifact_refs_resolved"] = int(resolved)
            result["markdown_artifact_refs_dropped"] = int(dropped)
            result["markdown_artifact_refs_re_resolved"] = (
                int(rust_result[2]) if len(rust_result) > 2 else 0
            )
            result["markdown_artifact_refs_still_unresolved"] = (
                int(rust_result[3]) if len(rust_result) > 3 else 0
            )
            return

        import json

        rows = store._conn.execute(
            "SELECT id, target_qualified, extra "
            "FROM edges "
            "WHERE kind='CROSS_ARTIFACT' AND extra LIKE '%original_symbol_name%'"
        ).fetchall()

        if not rows:
            result["markdown_artifact_refs_resolved"] = 0
            result["markdown_artifact_refs_dropped"] = 0
            result["markdown_artifact_refs_re_resolved"] = 0
            result["markdown_artifact_refs_still_unresolved"] = 0
            return

        # Parse extras and collect unique symbol names in one pass
        edge_data: list[tuple[int, str, str, dict]] = []  # (id, current_target, sym, extra)
        symbols: set[str] = set()
        for row in rows:
            try:
                extra = json.loads(row["extra"] or "{}")
            except (json.JSONDecodeError, TypeError):
                continue
            sym = extra.get("original_symbol_name")
            if not sym:
                continue
            edge_data.append((row["id"], row["target_qualified"], sym, extra))
            symbols.add(sym)

        if not edge_data:
            result["markdown_artifact_refs_resolved"] = 0
            result["markdown_artifact_refs_dropped"] = 0
            result["markdown_artifact_refs_re_resolved"] = 0
            result["markdown_artifact_refs_still_unresolved"] = 0
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

        # Compute desired state; only UPDATE rows where the state actually changes
        to_update: list[tuple] = []  # (new_target, new_extra_json, confidence, tier, edge_id)

        for edge_id, current_target, sym, extra in edge_data:
            matches = matches_by_sym.get(sym, [])
            unresolved_target = f"<unresolved:{sym}>"

            if len(matches) == 1:
                qname, lang = matches[0]
                if current_target == qname:
                    continue  # already correct — no-op
                new_extra = dict(extra)
                new_extra["target_language"] = lang
                new_extra["confidence"] = 0.8
                new_extra["confidence_tier"] = "HIGH"
                to_update.append((qname, json.dumps(new_extra), 0.8, "HIGH", edge_id))
                if current_target.startswith("<unresolved:"):
                    resolved += 1
                else:
                    re_resolved += 1
            else:
                if current_target == unresolved_target:
                    still_unresolved += 1
                    continue  # already correct — no-op
                new_extra = dict(extra)
                new_extra.pop("target_language", None)
                new_extra["confidence"] = 0.2
                new_extra["confidence_tier"] = "LOW"
                to_update.append((unresolved_target, json.dumps(new_extra), 0.2, "LOW", edge_id))
                demoted += 1

        if to_update:
            store._conn.executemany(
                "UPDATE edges "
                "SET target_qualified=?, extra=?, confidence=?, confidence_tier=? "
                "WHERE id=?",
                to_update,
            )

        store.commit()
        result["markdown_artifact_refs_resolved"] = resolved
        result["markdown_artifact_refs_dropped"] = demoted
        result["markdown_artifact_refs_re_resolved"] = re_resolved
        result["markdown_artifact_refs_still_unresolved"] = still_unresolved
    except (sqlite3.OperationalError, RuntimeError) as e:
        logger.warning("Markdown artifact ref resolution failed: %s", e)
        warnings.append(f"Markdown artifact ref resolution failed: {type(e).__name__}: {e}")


def _compute_signatures(
    store: GraphStore,
    result: dict[str, Any],
    warnings: list[str],
) -> None:
    """Compute human-readable signatures for nodes that lack one."""
    try:
        rust_compute = getattr(store, "compute_missing_signatures", None)
        if callable(rust_compute):
            result["signatures_computed"] = int(rust_compute())
            return

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
    except (sqlite3.OperationalError, RuntimeError, TypeError, KeyError) as e:
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
