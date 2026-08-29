"""Shared post-build processing pipeline.

After the core Tree-sitter parse (full_build or incremental_update), post-
processing steps must run to populate derived tables and resolve bridges:

1. Compute node signatures
2. Rebuild FTS5 search index
3. Resolve Markdown → code CROSS_ARTIFACT candidates
4. Extract Layer-2 manifest-backed CROSS_ARTIFACT bridges
5. Trace execution flows
6. Detect code communities
7. Persist hub / bridge centrality scores

This module extracts that pipeline so every entry point — MCP tool, CLI
commands, and watch mode — produces identical results.
"""

from __future__ import annotations

import json
import logging
import sqlite3
from dataclasses import asdict
from pathlib import Path
from typing import Any, Callable, cast

from pydantic import ValidationError

from dagayn.graph import GraphStore, store_write_transaction
from dagayn.graph._sql import _edge_target_name
from dagayn.state_types import (
    DroppedMarkdownArtifactResolution,
    MarkdownArtifactResolution,
    PostprocessResult,
    ResolvedMarkdownArtifactResolution,
    build_markdown_artifact_resolution,
)

logger = logging.getLogger(__name__)

type PostprocessValue = Any
type PostprocessPayload = dict[str, PostprocessValue]
type ArtifactEdgeData = tuple[int, str, str, PostprocessPayload]
type EdgeUpdate = tuple[str, str, str, float | None, str | None, int]


_NODE_QUALIFIED_EDGE_KINDS = (
    "CALLS",
    "INHERITS",
    "IMPLEMENTS",
    "CONTAINS",
    "REFERENCES",
    "TESTED_BY",
)


def _native_method(store: GraphStore, name: str) -> Any | None:
    """Return a native GraphStore method when the current connection exposes it."""
    method = getattr(store, name, None)
    return method if callable(method) else None


def _demote_unresolved_endpoint_edges(
    store: GraphStore,
    result: PostprocessResult,
    warnings: list[str],
) -> None:
    """Lower confidence on edges whose node-qualified endpoints are absent."""
    native = _native_method(store, "demote_unresolved_endpoint_edges")
    if native is not None:
        try:
            result.unresolved_endpoint_edges_demoted = int(native())
        except (OSError, RuntimeError, TypeError, ValueError) as e:
            logger.warning("Unresolved endpoint demotion failed: %s", e)
            warnings.append(f"Unresolved endpoint demotion failed: {type(e).__name__}: {e}")
        return
    if not hasattr(store, "_conn"):
        result.unresolved_endpoint_edges_demoted = 0
        return

    try:
        conn = store._conn
        placeholders = ", ".join("?" for _ in _NODE_QUALIFIED_EDGE_KINDS)
        updated = conn.execute(
            f"""
            UPDATE edges
            SET confidence = MIN(confidence, 0.2),
                confidence_tier = 'LOW'
            WHERE kind IN ({placeholders})
              AND UPPER(COALESCE(confidence_tier, 'EXTRACTED')) NOT IN ('LOW', 'UNKNOWN')
              AND (
                target_qualified LIKE '<unresolved:%'
                OR source_qualified LIKE '<unresolved:%'
                OR NOT EXISTS (
                    SELECT 1 FROM nodes n WHERE n.qualified_name = edges.target_qualified
                )
                OR NOT EXISTS (
                    SELECT 1 FROM nodes n WHERE n.qualified_name = edges.source_qualified
                )
              )
            """,
            _NODE_QUALIFIED_EDGE_KINDS,
        ).rowcount
        conn.commit()
        invalidate = getattr(store, "_invalidate_cache", None)
        if callable(invalidate):
            invalidate()
        result.unresolved_endpoint_edges_demoted = int(updated)
    except (sqlite3.OperationalError, OSError, RuntimeError, TypeError, ValueError) as e:
        logger.warning("Unresolved endpoint demotion failed: %s", e)
        warnings.append(f"Unresolved endpoint demotion failed: {type(e).__name__}: {e}")


_MANIFEST_FILENAMES = frozenset({"pyproject.toml", "package.json", "openapitools.json"})


def _should_scan_manifests(changed_files: list[str] | None) -> bool:
    """Return False when an incremental update touched no manifest files."""
    if not changed_files:
        return True
    return any(Path(path).name in _MANIFEST_FILENAMES for path in changed_files)


def _discover_manifest_bridges(store: GraphStore) -> Any | None:
    """Discover manifest-backed bridge nodes/edges without mutating the graph."""
    from dagayn.parser.manifest_bridges import discover_manifest_bridges, refine_node_line_ends

    repo_root = _store_repo_root(store)
    if repo_root is None or not repo_root.is_dir():
        return None

    discovered = discover_manifest_bridges(repo_root)
    refine_node_line_ends(repo_root, discovered.nodes)
    return discovered


def run_post_processing(
    store: GraphStore,
    changed_files: list[str] | None = None,
) -> PostprocessResult:
    """Run all post-build steps on a populated graph.

    Each step is non-fatal: failures are logged and collected as warnings
    so the primary build result is never lost.

    Args:
        store: An open GraphStore with nodes and edges already populated.
        changed_files: When set, FTS, centrality, and manifest extraction
            follow the dirty-set path (changed files / affected communities)
            instead of rebuilding those derived tables for the whole graph.

    Returns:
        Typed summary with a counter for each step that ran and a
        ``warnings`` list (only populated when at least one step failed).
    """
    native = _native_method(store, "run_post_processing_json")
    if native is not None:
        from dagayn.parser.manifest_bridges import EXTRACTOR_ID

        manifest_nodes: list[dict[str, Any]] = []
        manifest_edges: list[dict[str, Any]] = []
        discovered = _discover_manifest_bridges(store)
        if discovered is not None and _should_scan_manifests(changed_files):
            manifest_nodes = [asdict(node) for node in discovered.nodes]
            manifest_edges = [asdict(edge) for edge in discovered.edges]
        try:
            raw = cast(Callable[..., str], native)(
                EXTRACTOR_ID,
                json.dumps(manifest_nodes),
                json.dumps(manifest_edges),
                2,
                list(changed_files) if changed_files else None,
            )
            payload = json.loads(raw)
            native_warnings = payload.pop("warnings", []) or []
            result = PostprocessResult(**payload)
            if native_warnings:
                result.warnings = list(native_warnings)
            return result
        except (
            OSError,
            RuntimeError,
            TypeError,
            ValueError,
            json.JSONDecodeError,
            ValidationError,
        ) as e:
            logger.warning("Rust post-processing failed, falling back to Python: %s", e)

    result = PostprocessResult()
    warnings: list[str] = []

    _compute_signatures(store, result, warnings)
    _rebuild_fts_index(store, result, warnings, changed_files)
    _resolve_bare_name_edges(store, result, warnings)
    _resolve_markdown_artifact_refs(store, result, warnings)
    _resolve_terraform_artifact_refs(store, result, warnings)
    _demote_unresolved_endpoint_edges(store, result, warnings)
    _apply_manifest_bridges(store, result, warnings, changed_files)
    _trace_flows(store, result, warnings)
    _detect_communities(store, result, warnings)
    _persist_centrality_scores(store, result, warnings, changed_files)

    if warnings:
        result.warnings = warnings
    return result


# -- Individual steps (private) ------------------------------------------


def _markdown_artifact_resolution(
    *,
    edge_id: int,
    current_target: str,
    symbol: str,
    extra: PostprocessPayload,
    matches: list[tuple[str, str]],
) -> MarkdownArtifactResolution:
    """Return the typed target state for one Markdown artifact edge."""
    unresolved_target = f"<unresolved:{symbol}>"
    is_implicit_code_span = (
        extra.get("evidence_kind") == "markdown_code_span"
        and extra.get("evidence_source") == "code_span"
    )

    if len(matches) == 1:
        qname, lang = matches[0]
        new_extra = dict(extra)
        new_extra["target_language"] = lang
        if is_implicit_code_span:
            confidence = 0.4
            confidence_tier = "MEDIUM"
        else:
            confidence = 0.8
            confidence_tier = "HIGH"
        new_extra["confidence"] = confidence
        new_extra["confidence_tier"] = confidence_tier
        return build_markdown_artifact_resolution(
            state="resolved" if current_target.startswith("<unresolved:") else "re_resolved",
            edge_id=edge_id,
            target_qualified=qname,
            target_language=lang,
            confidence=confidence,
            confidence_tier=confidence_tier,
            extra=new_extra,
        )

    if is_implicit_code_span:
        return build_markdown_artifact_resolution(state="dropped", edge_id=edge_id)

    if current_target == unresolved_target:
        return build_markdown_artifact_resolution(
            state="still_unresolved",
            edge_id=edge_id,
            target_qualified=unresolved_target,
            confidence=0.2,
            confidence_tier="LOW",
        )

    new_extra = dict(extra)
    new_extra.pop("target_language", None)
    new_extra["confidence"] = 0.2
    new_extra["confidence_tier"] = "LOW"
    return build_markdown_artifact_resolution(
        state="dropped",
        edge_id=edge_id,
        target_qualified=unresolved_target,
        confidence=0.2,
        confidence_tier="LOW",
        extra=new_extra,
    )


def _is_markdown_artifact_bridge(extra: PostprocessPayload) -> bool:
    """Return True for Markdown/documentation CROSS_ARTIFACT bridges only.

    Terraform ``handler`` / ``entry_point`` bridges also carry
    ``original_symbol_name`` but must be resolved by
    :func:`_resolve_terraform_artifact_refs` (Function/Test matching), not by
    the Markdown any-kind unique-name binder.
    """
    if extra.get("source_language") == "markdown":
        return True
    if extra.get("bridge_kind") == "documentation":
        return True
    return False


def _resolve_markdown_artifact_refs(
    store: GraphStore,
    result: PostprocessResult,
    warnings: list[str],
) -> None:
    """Idempotently resolve/update Markdown→code CROSS_ARTIFACT edges.

    Only documentation/markdown bridges are considered.  Every such edge
    emitted by the Markdown parser (or documentation directives) carries
    ``extra.original_symbol_name`` — the raw backtick span symbol.  This
    step runs on every postprocess call and brings each edge in line with
    the *current* state of the nodes table:

    - Unique non-markdown match → ``target_qualified`` = the node's
      ``qualified_name``; confidence promoted to HIGH (0.8).
    - Zero or 2+ matches from implicit Markdown code spans → delete the
      low-confidence candidate.  Code spans often contain ordinary vocabulary
      and should not appear as real graph data unless they resolve uniquely.
    - Zero or 2+ matches from explicit documentation directives → keep or
      demote to ``<unresolved:{sym}>`` because the author intentionally
      declared a dependency.

    Terraform and other non-documentation bridges that happen to carry
    ``original_symbol_name`` are left untouched for their dedicated resolvers.

    Result keys:
      ``markdown_artifact_refs_resolved``   — transitions unresolved→resolved
      ``markdown_artifact_refs_dropped``    — unresolved implicit candidates deleted
                                             or explicit refs demoted
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
            resolve = cast(Callable[[], tuple[int, int, int, int]], rust_resolve)
            rust_result = resolve()
            resolved, dropped = rust_result[:2]
            result.markdown_artifact_refs_resolved = resolved
            result.markdown_artifact_refs_dropped = dropped
            result.markdown_artifact_refs_re_resolved = (
                rust_result[2] if len(rust_result) > 2 else 0
            )
            result.markdown_artifact_refs_still_unresolved = (
                rust_result[3] if len(rust_result) > 3 else 0
            )
            return

        rows = store._conn.execute(
            "SELECT id, target_qualified, extra "
            "FROM edges "
            "WHERE kind='CROSS_ARTIFACT' AND extra LIKE '%original_symbol_name%'"
        ).fetchall()

        if not rows:
            result.markdown_artifact_refs_resolved = 0
            result.markdown_artifact_refs_dropped = 0
            result.markdown_artifact_refs_re_resolved = 0
            result.markdown_artifact_refs_still_unresolved = 0
            return

        # Parse extras and collect unique symbol names in one pass
        edge_data: list[ArtifactEdgeData] = []  # (id, current_target, sym, extra)
        symbols: set[str] = set()
        for row in rows:
            try:
                extra = json.loads(row["extra"] or "{}")
            except (json.JSONDecodeError, TypeError):
                continue
            if not _is_markdown_artifact_bridge(extra):
                continue
            sym = extra.get("original_symbol_name")
            if not sym:
                continue
            edge_data.append((row["id"], row["target_qualified"], sym, extra))
            symbols.add(sym)

        if not edge_data:
            result.markdown_artifact_refs_resolved = 0
            result.markdown_artifact_refs_dropped = 0
            result.markdown_artifact_refs_re_resolved = 0
            result.markdown_artifact_refs_still_unresolved = 0
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

        # Compute desired state; only UPDATE/DELETE rows where the state actually changes
        # (new_target, target_name, new_extra_json, confidence, tier, edge_id)
        to_update: list[EdgeUpdate] = []
        to_delete: list[tuple[int]] = []

        for edge_id, current_target, sym, extra in edge_data:
            matches = matches_by_sym.get(sym, [])
            decision = _markdown_artifact_resolution(
                edge_id=edge_id,
                current_target=current_target,
                symbol=sym,
                extra=extra,
                matches=matches,
            )

            if isinstance(decision, ResolvedMarkdownArtifactResolution):
                qname = decision.target_qualified
                if current_target == qname:
                    continue  # already correct — no-op
                to_update.append(
                    (
                        qname,
                        _edge_target_name(qname),
                        json.dumps(decision.extra),
                        decision.confidence,
                        decision.confidence_tier,
                        edge_id,
                    )
                )
                if decision.state == "resolved":
                    resolved += 1
                else:
                    re_resolved += 1
                continue

            if decision.state == "still_unresolved":
                still_unresolved += 1
                continue

            if isinstance(decision, DroppedMarkdownArtifactResolution):
                if decision.target_qualified is None:
                    to_delete.append((edge_id,))
                    demoted += 1
                    continue

                to_update.append(
                    (
                        decision.target_qualified,
                        _edge_target_name(decision.target_qualified),
                        json.dumps(decision.extra),
                        decision.confidence,
                        decision.confidence_tier,
                        edge_id,
                    )
                )
                demoted += 1

        if to_update:
            store._conn.executemany(
                "UPDATE edges "
                "SET target_qualified=?, target_name=?, extra=?, confidence=?, confidence_tier=? "
                "WHERE id=?",
                to_update,
            )
        if to_delete:
            store._conn.executemany("DELETE FROM edges WHERE id=?", to_delete)

        store.commit()
        result.markdown_artifact_refs_resolved = resolved
        result.markdown_artifact_refs_dropped = demoted
        result.markdown_artifact_refs_re_resolved = re_resolved
        result.markdown_artifact_refs_still_unresolved = still_unresolved
    except (sqlite3.OperationalError, RuntimeError) as e:
        logger.warning("Markdown artifact ref resolution failed: %s", e)
        warnings.append(f"Markdown artifact ref resolution failed: {type(e).__name__}: {e}")


def _store_repo_root(store: GraphStore) -> Path | None:
    """Resolve ``repo_root`` from Python or Rust GraphStore bindings."""
    getter = getattr(store, "get_repo_root", None)
    if callable(getter):
        root = cast(Callable[[], Path | None], getter)()
        if root is not None:
            return Path(root)
    get_meta = getattr(store, "get_metadata", None)
    if callable(get_meta):
        raw = cast(Callable[[str], str | None], get_meta)("repo_root")
        if raw:
            return Path(raw)
    return None


def _apply_manifest_bridges(
    store: GraphStore,
    result: PostprocessResult,
    warnings: list[str],
    changed_files: list[str] | None = None,
) -> None:
    """Idempotently extract Layer-2 manifest-backed CROSS_ARTIFACT bridges.

    Discovers the replacement set first, then atomically swaps prior
    ``extractor=manifest_bridges`` edges/nodes inside an explicit
    ``BEGIN IMMEDIATE`` transaction.  Failure leaves prior bridges intact.
    Existing parser ``File`` rows are left untouched so hash/mtime survive.
    Incremental updates that did not touch a known manifest skip the scan.
    """
    try:
        from dagayn.parser.manifest_bridges import (
            EXTRACTOR_ID,
            discover_manifest_bridges,
            refine_node_line_ends,
        )

        if not _should_scan_manifests(changed_files):
            return

        repo_root = _store_repo_root(store)
        if repo_root is None or not repo_root.is_dir():
            result.manifest_bridges_edges = 0
            result.manifest_bridges_nodes = 0
            return

        # Discover before mutating so a scan failure cannot wipe prior bridges.
        discovered = discover_manifest_bridges(repo_root)
        refine_node_line_ends(repo_root, discovered.nodes)

        native = _native_method(store, "replace_manifest_bridges_json")
        if native is not None:
            nodes_upserted = int(
                native(
                    EXTRACTOR_ID,
                    json.dumps([asdict(node) for node in discovered.nodes]),
                    json.dumps([asdict(edge) for edge in discovered.edges]),
                )
            )
            result.manifest_bridges_edges = discovered.edge_count
            result.manifest_bridges_nodes = nodes_upserted
            return

        if not hasattr(store, "_conn"):
            result.manifest_bridges_edges = 0
            result.manifest_bridges_nodes = 0
            return

        conn = store._conn
        with store_write_transaction(store):
            conn.execute(
                "DELETE FROM edges WHERE kind='CROSS_ARTIFACT' "
                "AND json_extract(extra, '$.extractor') = ?",
                (EXTRACTOR_ID,),
            )
            conn.execute(
                "DELETE FROM nodes WHERE kind='File' AND json_extract(extra, '$.extractor') = ?",
                (EXTRACTOR_ID,),
            )

            nodes_upserted = 0
            for node in discovered.nodes:
                # Skip existing File rows (typically parser-owned) so we do not
                # clobber file_hash / mtime_ns / extra used by incremental skip.
                if node.kind == "File" and store.get_node(node.file_path) is not None:
                    continue
                store.upsert_node(node)
                nodes_upserted += 1
            for edge in discovered.edges:
                store.upsert_edge(edge)
        invalidate = getattr(store, "_invalidate_cache", None)
        if callable(invalidate):
            invalidate()

        result.manifest_bridges_edges = discovered.edge_count
        result.manifest_bridges_nodes = nodes_upserted
    except (sqlite3.OperationalError, OSError, RuntimeError, TypeError, ValueError) as e:
        logger.warning("Manifest bridge extraction failed: %s", e)
        warnings.append(f"Manifest bridge extraction failed: {type(e).__name__}: {e}")


def _resolve_terraform_artifact_refs(
    store: GraphStore,
    result: PostprocessResult,
    warnings: list[str],
) -> None:
    """Resolve Terraform entrypoint CROSS_ARTIFACT edges to unique code symbols.

    Handles ``handler`` / ``entry_point`` bridges whose ``original_symbol_name``
    looks like ``module.attr`` (AWS Lambda-style) by matching a unique
    non-Markdown Function/Test named ``attr`` whose file stem is ``module``.
    Path-based Terraform bridges are already concrete and left untouched.
    """
    native = _native_method(store, "resolve_terraform_artifact_refs")
    if native is not None:
        try:
            resolved, still_unresolved = native()
            result.terraform_artifact_refs_resolved = int(resolved)
            result.terraform_artifact_refs_still_unresolved = int(still_unresolved)
        except (OSError, RuntimeError, TypeError, ValueError) as e:
            logger.warning("Terraform artifact ref resolution failed: %s", e)
            warnings.append(f"Terraform artifact ref resolution failed: {type(e).__name__}: {e}")
        return
    if not hasattr(store, "_conn"):
        result.terraform_artifact_refs_resolved = 0
        result.terraform_artifact_refs_still_unresolved = 0
        return

    resolved = 0
    still_unresolved = 0
    try:
        rows = store._conn.execute(
            "SELECT id, target_qualified, extra "
            "FROM edges "
            "WHERE kind='CROSS_ARTIFACT' AND extra LIKE '%original_symbol_name%'"
        ).fetchall()

        edge_data: list[tuple[int, str, str, PostprocessPayload]] = []
        for row in rows:
            try:
                extra = json.loads(row["extra"] or "{}")
            except (json.JSONDecodeError, TypeError):
                continue
            if extra.get("source_language") != "terraform":
                continue
            if extra.get("evidence_source") not in {"handler", "entry_point"}:
                continue
            if extra.get("relationship_role") != "maps_entrypoint":
                continue
            sym = extra.get("original_symbol_name")
            if not isinstance(sym, str) or not sym:
                continue
            edge_data.append((row["id"], row["target_qualified"], sym, extra))

        if not edge_data:
            result.terraform_artifact_refs_resolved = 0
            result.terraform_artifact_refs_still_unresolved = 0
            return

        to_update: list[EdgeUpdate] = []
        for edge_id, current_target, sym, extra in edge_data:
            match = _resolve_terraform_entrypoint_symbol(store, sym)
            if match is None:
                still_unresolved += 1
                continue
            qname, lang = match
            if current_target == qname:
                continue
            new_extra = dict(extra)
            new_extra["target_language"] = lang
            new_extra["confidence"] = 0.8
            new_extra["confidence_tier"] = "HIGH"
            to_update.append(
                (qname, _edge_target_name(qname), json.dumps(new_extra), 0.8, "HIGH", edge_id)
            )
            resolved += 1

        if to_update:
            store._conn.executemany(
                "UPDATE edges "
                "SET target_qualified=?, target_name=?, extra=?, confidence=?, confidence_tier=? "
                "WHERE id=?",
                to_update,
            )
            store.commit()

        result.terraform_artifact_refs_resolved = resolved
        result.terraform_artifact_refs_still_unresolved = still_unresolved
    except (sqlite3.OperationalError, RuntimeError) as e:
        logger.warning("Terraform artifact ref resolution failed: %s", e)
        warnings.append(f"Terraform artifact ref resolution failed: {type(e).__name__}: {e}")


def _resolve_terraform_entrypoint_symbol(
    store: GraphStore,
    symbol: str,
) -> tuple[str, str] | None:
    """Return ``(qualified_name, language)`` for a unique Terraform entrypoint."""
    symbol = symbol.strip()
    if not symbol or symbol.startswith("<"):
        return None

    if "." in symbol:
        module, _, attr = symbol.rpartition(".")
        if not module or not attr:
            return None
        rows = store._conn.execute(
            "SELECT qualified_name, language, file_path FROM nodes "
            "WHERE name = ? AND kind IN ('Function', 'Test') AND language != 'markdown'",
            (attr,),
        ).fetchall()
        matches = [
            (row["qualified_name"], row["language"] or "unknown")
            for row in rows
            if _terraform_module_matches_file(module, row["file_path"] or "")
        ]
    else:
        rows = store._conn.execute(
            "SELECT qualified_name, language FROM nodes "
            "WHERE name = ? AND kind IN ('Function', 'Test') AND language != 'markdown'",
            (symbol,),
        ).fetchall()
        matches = [(row["qualified_name"], row["language"] or "unknown") for row in rows]

    if len(matches) != 1:
        return None
    return matches[0]


def _terraform_module_matches_file(module: str, file_path: str) -> bool:
    """Return whether *file_path* plausibly implements Terraform handler module *module*."""
    if not file_path:
        return False
    path = file_path.replace("\\", "/")
    stem = path.rsplit("/", 1)[-1]
    if "." in stem:
        stem = stem.rsplit(".", 1)[0]
    if stem == module:
        return True
    # Allow package-style handlers such as app/hello/__init__.py for module "hello".
    parts = path.split("/")
    return module in parts[:-1]


def _compute_signatures(
    store: GraphStore,
    result: PostprocessResult,
    warnings: list[str],
) -> None:
    """Compute human-readable signatures for nodes that lack one."""
    try:
        rust_compute = getattr(store, "compute_missing_signatures", None)
        if callable(rust_compute):
            result.signatures_computed = cast(Callable[[], int], rust_compute)()
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
            elif kind == "DocSection":
                sig = f"# {name}"
            else:
                sig = name
            store.update_node_signature(node_id, sig[:512])
        store.commit()
        result.signatures_computed = len(rows)
    except (sqlite3.OperationalError, RuntimeError, TypeError, KeyError) as e:
        logger.warning("Signature computation failed: %s", e)
        warnings.append(f"Signature computation failed: {type(e).__name__}: {e}")


def _rebuild_fts_index(
    store: GraphStore,
    result: PostprocessResult,
    warnings: list[str],
    changed_files: list[str] | None = None,
) -> None:
    """Rebuild FTS, or refresh only *changed_files* when that set is non-empty."""
    try:
        if changed_files:
            rust_sync = getattr(store, "sync_fts_for_file_paths", None)
            if callable(rust_sync):
                fts_count = int(cast(int, rust_sync(changed_files)))
            elif hasattr(store, "_conn"):
                from dagayn.graph import store_write_transaction
                from dagayn.graph._fts_sync import sync_fts_for_file_paths

                with store_write_transaction(store):
                    fts_count = sync_fts_for_file_paths(
                        store._conn,
                        changed_files,
                        _store_repo_root(store),
                    )
            else:
                from dagayn.search import rebuild_fts_index

                fts_count = rebuild_fts_index(store)
        else:
            from dagayn.search import rebuild_fts_index

            fts_count = rebuild_fts_index(store)
        result.fts_indexed = fts_count
    except (sqlite3.OperationalError, ImportError, RuntimeError, TypeError) as e:
        logger.warning("FTS index rebuild failed: %s", e)
        warnings.append(f"FTS index rebuild failed: {type(e).__name__}: {e}")


def _resolve_bare_name_edges(
    store: GraphStore,
    result: PostprocessResult,
    warnings: list[str],
) -> None:
    """Resolve bare-name CALLS and INHERITS/IMPLEMENTS edges using import context."""
    native_calls = _native_method(store, "resolve_bare_call_targets")
    native_inherits = _native_method(store, "resolve_bare_inheritance_targets")
    if native_calls is not None and native_inherits is not None:
        try:
            result.bare_call_targets_resolved = int(native_calls())
            result.bare_inheritance_targets_resolved = int(native_inherits())
        except (OSError, RuntimeError, TypeError, AttributeError) as e:
            logger.warning("Bare-name edge resolution failed: %s", e)
            warnings.append(f"Bare-name edge resolution failed: {type(e).__name__}: {e}")
        return
    if not hasattr(store, "_conn"):
        result.bare_call_targets_resolved = 0
        result.bare_inheritance_targets_resolved = 0
        return

    try:
        from dagayn.bare_name_resolution import (
            resolve_bare_call_targets,
            resolve_bare_inheritance_targets,
        )

        result.bare_call_targets_resolved = resolve_bare_call_targets(store)
        result.bare_inheritance_targets_resolved = resolve_bare_inheritance_targets(store)
    except (sqlite3.OperationalError, RuntimeError, TypeError, AttributeError) as e:
        logger.warning("Bare-name edge resolution failed: %s", e)
        warnings.append(f"Bare-name edge resolution failed: {type(e).__name__}: {e}")


def _trace_flows(
    store: GraphStore,
    result: PostprocessResult,
    warnings: list[str],
) -> None:
    """Trace execution flows from entry points."""
    try:
        from dagayn.flows import rebuild_stored_flows

        count = rebuild_stored_flows(store)
        result.flows_detected = count
    except (sqlite3.OperationalError, ImportError) as e:
        logger.warning("Flow detection failed: %s", e)
        warnings.append(f"Flow detection failed: {type(e).__name__}: {e}")


def _detect_communities(
    store: GraphStore,
    result: PostprocessResult,
    warnings: list[str],
) -> None:
    """Detect code communities via Leiden algorithm or file grouping."""
    try:
        from dagayn.communities import detect_communities, store_communities

        comms = detect_communities(store)
        count = store_communities(store, comms)
        result.communities_detected = count
    except (sqlite3.OperationalError, ImportError) as e:
        logger.warning("Community detection failed: %s", e)
        warnings.append(f"Community detection failed: {type(e).__name__}: {e}")


def _persist_centrality_scores(
    store: GraphStore,
    result: PostprocessResult,
    warnings: list[str],
    changed_files: list[str] | None = None,
) -> None:
    """Persist query-time hub / bridge scores after graph post-processing."""
    try:
        from dagayn.analysis import persist_centrality_scores

        counts = persist_centrality_scores(store, changed_files=changed_files)
        result.hub_scores_persisted = counts.get("hub_scores_persisted", 0)
        result.bridge_scores_persisted = counts.get("bridge_scores_persisted", 0)
        result.hub_scores_code_persisted = counts.get("hub_scores_code_persisted", 0)
        result.bridge_scores_code_persisted = counts.get("bridge_scores_code_persisted", 0)
    except (sqlite3.OperationalError, ImportError, RuntimeError) as e:
        logger.warning("Centrality score persistence failed: %s", e)
        warnings.append(f"Centrality score persistence failed: {type(e).__name__}: {e}")
