"""Tools 2, 3, 5, 6, 9: query / search / stats helpers."""

from __future__ import annotations

import logging
from collections.abc import Mapping
from pathlib import Path
from typing import Any

from ..cross_artifact import (
    is_low_confidence_unresolved_markdown_code_span,
)
from ..embeddings import EmbeddingStore
from ..graph import GraphNode, _sanitize_name, edge_to_dict, node_to_dict
from ..hints import generate_hints, get_session
from ..incremental import get_changed_files, get_db_path, get_staged_and_unstaged
from ..search import embedding_health_available, hybrid_search
from ..state_types import (
    MissingnessRecord,
    TraversalEntry,
    TraversalMode,
    seal_missingness_item,
    seal_reachability_info,
)
from ._common import (
    _BUILTIN_CALL_NAMES,
    _db_path_for_repo,
    _error_response,
    _get_store,
    apply_output_budget,
    graph_answerability_summary,
    guidance_actions_to_hints,
    handle_tool_runtime_error,
    is_sqlite_corrupt_error,
    make_guidance_item,
    make_response,
    missingness_from_answerability,
    recover_corrupt_graph,
)
from .query_graph_dispatch import (
    QueryGraphState,
    build_query_graph_response,
    execute_query_pattern,
    resolve_query_target,
)
from .query_graph_support import QUERY_PATTERNS, exactness_action, result_evidence_type

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Tool 2: get_impact_radius
# ---------------------------------------------------------------------------


def _partial_coverage_missingness(
    embedding_health: Mapping[str, Any] | None,
) -> MissingnessRecord | None:
    """Disclose that semantic ranking only covers part of the graph.

    An interrupted embedding run commits per batch and then raises, so the rows
    that finished are durable. Health only distinguished 0 vs >0 matching
    vectors, so a 5%-embedded corpus reported "available" and the unembedded
    95% looked like "not semantically relevant".
    """
    if not embedding_health or not embedding_health.get("partial_coverage"):
        return None
    coverage = embedding_health.get("embedding_coverage")
    missing = embedding_health.get("missing_embedding_count")
    return seal_missingness_item(
        {
            "reason_code": "partial_embeddings",
            "severity": "medium",
            "claim_effect": (
                "semantic ranking covers only part of the graph, so a node's absence"
                " from these results is not evidence it is irrelevant"
            ),
            "details": {
                "embedding_coverage": coverage,
                "missing_embedding_count": missing,
            },
        }
    )


def _is_low_confidence_unresolved_markdown_code_span(edge: Any) -> bool:
    return is_low_confidence_unresolved_markdown_code_span(edge)


def _semantic_search_guidance(
    *,
    query: str,
    result_count: int,
    search_mode: str,
    embedding_health: Mapping[str, Any],
) -> list[dict[str, Any]]:
    missingness_items: list[MissingnessRecord] = []
    if embedding_health and not embedding_health_available(embedding_health):
        missingness_items.append(
            {
                "reason_code": "missing_embeddings",
                "severity": "medium",
                "claim_effect": "semantic ranking may be keyword-only",
            }
        )
    partial = _partial_coverage_missingness(embedding_health)
    if partial is not None:
        missingness_items.append(partial)
    if result_count:
        return [
            make_guidance_item(
                claim=f"Hybrid search returned {result_count} candidate(s) for '{query}'.",
                evidence={
                    "type": "computed",
                    "query": query,
                    "result_count": result_count,
                    "search_mode": search_mode,
                },
                confidence="medium",
                missingness=missingness_items
                or [
                    {
                        "reason_code": "ranking_is_evidence_not_verdict",
                        "severity": "low",
                        "claim_effect": (
                            "scores rank leads; verify with callers_of or source reads"
                        ),
                    }
                ],
                action="query_graph_tool callers_of -- confirm the best candidate relationship",
                reason_codes=["hybrid_search"],
                counts={"result_count": result_count},
            )
        ]
    return [
        make_guidance_item(
            claim=f"No nodes matched '{query}' in the current graph.",
            evidence={"type": "computed", "query": query, "search_mode": search_mode},
            confidence="low",
            missingness=[
                *missingness_items,
                {
                    "reason_code": "not_found_in_current_graph",
                    "severity": "medium",
                    "claim_effect": (
                        "absence is graph-limited, not proof the symbol does not exist"
                    ),
                },
            ],
            action="dagayn update -- refresh graph coverage before concluding absence",
            reason_codes=["zero_result"],
            counts={"result_count": 0},
        )
    ]


def _normalized_repo_path(value: str, root: Path) -> str:
    """Return *value* as a repo-relative posix path when possible."""
    path = Path(value)
    if path.is_absolute():
        try:
            path = path.relative_to(root)
        except ValueError:
            return path.as_posix()
    return path.as_posix()


def _unmatched_changed_files(
    changed_files: list[str], changed_nodes: list[GraphNode], root: Path
) -> list[str]:
    """Return the changed files the graph holds no nodes for.

    Every indexed file has at least a ``File`` node, so "no changed node for
    this path" means the graph has never seen it -- a path typo, a file added
    since the last build, or an unsupported language. That is a very different
    answer from "this file has no dependents", and the two are otherwise
    reported identically.
    """
    matched = {
        _normalized_repo_path(str(file_path), root)
        for file_path in (getattr(node, "file_path", None) for node in changed_nodes)
        if file_path
    }
    return [f for f in changed_files if _normalized_repo_path(f, root) not in matched]


def get_impact_radius(
    changed_files: list[str] | None = None,
    max_depth: int = 2,
    max_results: int = 50,
    repo_root: str | None = None,
    base: str = "HEAD~1",
    detail_level: str = "standard",
) -> dict[str, Any]:
    """Analyze the blast radius of changed files.

    Args:
        changed_files: Explicit list of changed file paths (relative to repo root).
                       If omitted, auto-detects from git diff.
        max_depth: How many hops to traverse in the graph (default: 2).
        max_results: Maximum impacted nodes to return (default: 50).
        repo_root: Repository root path. Auto-detected if omitted.
        base: Git ref for auto-detecting changes (default: HEAD~1).
        detail_level: "standard" (full output) or "minimal" (summary only).

    Returns:
        Changed nodes, impacted nodes, impacted files, connecting edges,
        plus ``truncated`` flag and ``total_impacted`` count.
    """
    store = None
    try:
        store, root = _get_store(repo_root)
        answerability = graph_answerability_summary(store)
        missingness = missingness_from_answerability(answerability)
        if changed_files is None:
            changed_files = get_changed_files(root, base)
            if not changed_files:
                changed_files = get_staged_and_unstaged(root)

        if not changed_files:
            return {
                "status": "ok",
                "summary": "No changed files detected.",
                "changed_nodes": [],
                "impacted_nodes": [],
                "impacted_files": [],
                "truncated": False,
                "total_impacted": 0,
                "answerability": answerability,
                "missingness": missingness,
            }

        # Convert to absolute paths for graph lookup
        abs_files = [str(root / f) for f in changed_files]
        result = store.get_impact_radius(abs_files, max_depth=max_depth, max_nodes=max_results)

        changed_dicts = [node_to_dict(n) for n in result["changed_nodes"]]
        impacted_dicts = [node_to_dict(n) for n in result["impacted_nodes"]]
        edge_dicts = [
            edge_to_dict(e)
            for e in result["edges"]
            if not _is_low_confidence_unresolved_markdown_code_span(e)
        ]
        bridge_transitions = list(result.get("bridge_transitions") or [])
        low_confidence_bridges = list(result.get("low_confidence_bridges") or [])
        truncated = result["truncated"]
        total_impacted = result["total_impacted"]
        unmatched_files = _unmatched_changed_files(changed_files, result["changed_nodes"], root)

        summary_parts = [
            f"Blast radius for {len(changed_files)} changed file(s):",
            f"  - {len(changed_dicts)} nodes directly changed",
            f"  - {len(impacted_dicts)} nodes impacted (within {max_depth} hops)",
            f"  - {len(result['impacted_files'])} additional files affected",
        ]
        if unmatched_files:
            summary_parts.append(
                f"  - {len(unmatched_files)} of {len(changed_files)} changed file(s) are NOT in"
                " the graph: their blast radius is unknown, not zero"
            )
        if bridge_transitions:
            summary_parts.append(
                f"  - {len(bridge_transitions)} reportable cross-artifact bridge hop(s)"
            )
        if low_confidence_bridges:
            summary_parts.append(
                f"  - {len(low_confidence_bridges)} low-confidence bridge caveat(s)"
            )
        if truncated:
            summary_parts.append(
                f"  - Results truncated: showing {len(impacted_dicts)}"
                f" of {total_impacted} impacted nodes"
            )

        impact_missingness = [
            *missingness,
            *low_confidence_bridges,
        ]
        if unmatched_files:
            # Without this, "0 nodes impacted" for a file the graph has never
            # seen is indistinguishable from a genuinely dependency-free change,
            # and the caller reports the change as safe.
            impact_missingness.append(
                seal_missingness_item(
                    {
                        "reason_code": "changed_files_not_in_graph",
                        "severity": "high",
                        "claim_effect": (
                            "impact for these files is unknown, not zero -- run dagayn update"
                            " (or check the paths) before treating the change as safe"
                        ),
                        "details": {"unmatched_changed_files": unmatched_files[:20]},
                    }
                )
            )
        if bridge_transitions:
            impact_missingness.append(
                {
                    "reason_code": "cross_artifact_bridge_is_static_evidence",
                    "severity": "low",
                    "claim_effect": (
                        "bridge hops are graph-derived explainable paths, not runtime traces"
                    ),
                }
            )

        guidance = []
        if bridge_transitions:
            guidance.append(
                make_guidance_item(
                    claim=(
                        f"Impact crosses {len(bridge_transitions)} reportable "
                        "cross-artifact bridge(s)."
                    ),
                    evidence={
                        "type": "extracted",
                        "bridge_transitions": bridge_transitions[:5],
                    },
                    confidence="high",
                    missingness=[
                        {
                            "reason_code": "cross_artifact_bridge_is_static_evidence",
                            "severity": "low",
                            "claim_effect": (
                                "follow docs_for / implementations_of / bridge edges to confirm"
                            ),
                        }
                    ],
                    action=(
                        'query_graph_tool pattern="docs_for" -- follow contract docs; '
                        "also try implementations_of / CROSS_ARTIFACT neighbors"
                    ),
                    reason_codes=["cross_artifact_bridge_impact"],
                    counts={"bridge_transition_count": len(bridge_transitions)},
                )
            )
        if low_confidence_bridges:
            guidance.append(
                make_guidance_item(
                    claim="Low-confidence cross-artifact bridges are caveats, not hard impact.",
                    evidence={
                        "type": "extracted",
                        "caveat_count": len(low_confidence_bridges),
                        "examples": low_confidence_bridges[:3],
                    },
                    confidence="low",
                    missingness=low_confidence_bridges[:5],
                    action=(
                        'query_graph_tool pattern="docs_for" -- verify before treating as impact'
                    ),
                    reason_codes=["low_confidence_cross_artifact_bridge"],
                    counts={"low_confidence_bridge_count": len(low_confidence_bridges)},
                )
            )

        if detail_level == "minimal":
            impacted_count = len(impacted_dicts)
            if impacted_count > 20:
                risk = "high"
            elif impacted_count > 5:
                risk = "medium"
            else:
                risk = "low"
            key_entities = [n["name"] for n in impacted_dicts[:5]]
            return {
                "status": "ok",
                "summary": "\n".join(summary_parts),
                "risk": risk,
                "unmatched_changed_files": unmatched_files,
                "impacted_file_count": len(result["impacted_files"]),
                "key_entities": key_entities,
                "bridge_transition_count": len(bridge_transitions),
                "truncated": truncated,
                "answerability": answerability,
                "missingness": impact_missingness,
                "guidance": guidance,
            }

        payload = {
            "status": "ok",
            "summary": "\n".join(summary_parts),
            "changed_files": changed_files,
            "unmatched_changed_files": unmatched_files,
            "changed_nodes": changed_dicts,
            "impacted_nodes": impacted_dicts,
            "impacted_files": result["impacted_files"],
            "edges": edge_dicts,
            "bridge_transitions": bridge_transitions,
            "low_confidence_bridges": low_confidence_bridges,
            "truncated": truncated,
            "total_impacted": total_impacted,
            "answerability": answerability,
            "missingness": impact_missingness,
            "guidance": guidance,
        }
        apply_output_budget(
            payload,
            budget_tokens=8000,
            list_priorities=[
                "changed_files",
                "impacted_files",
                "changed_nodes",
                "impacted_nodes",
                "bridge_transitions",
                "edges",
                "low_confidence_bridges",
            ],
        )
        return payload
    except Exception as exc:
        return handle_tool_runtime_error(
            exc, logger=logger, context="get_impact_radius", repo_root=repo_root
        )
    finally:
        if store is not None:
            store.close()


# ---------------------------------------------------------------------------
# Tool 3: query_graph
# ---------------------------------------------------------------------------


def query_graph(
    pattern: str,
    target: str,
    repo_root: str | None = None,
    detail_level: str = "standard",
    *,
    _corrupt_retried: bool = False,
) -> dict[str, Any]:
    """Run a predefined graph query.

    Args:
        pattern: Query pattern. One of: callers_of, callees_of, imports_of,
                 importers_of, docs_for, implementations_of, bridges_from, children_of,
                 tests_for, inheritors_of, file_summary.
        target: The node name, qualified name, or file path to query about.
        repo_root: Repository root path. Auto-detected if omitted.
        detail_level: "standard" (full output) or "minimal" (summary only).

    Returns:
        Matching nodes and edges for the query.
    """
    store = None
    try:
        store, root = _get_store(repo_root)
        answerability = graph_answerability_summary(store)
        missingness = missingness_from_answerability(answerability)
        if pattern not in QUERY_PATTERNS:
            return {
                "status": "error",
                "error": (f"Unknown pattern '{pattern}'. Available: {list(QUERY_PATTERNS.keys())}"),
            }

        # For callers_of, skip common builtins early (bare names only)
        # "Who calls .map()?" returns hundreds of useless hits.
        # Qualified names (e.g. "utils.py::map") bypass this filter.
        if pattern == "callers_of" and target in _BUILTIN_CALL_NAMES and "::" not in target:
            return {
                "status": "ok",
                "pattern": pattern,
                "target": target,
                "description": QUERY_PATTERNS[pattern],
                "summary": (f"'{target}' is a common builtin — callers_of skipped to avoid noise."),
                "results": [],
                "edges": [],
                "answerability": answerability,
                "missingness": missingness,
            }

        state = QueryGraphState(
            store=store,
            root=root,
            pattern=pattern,
            original_target=target,
            target=target,
        )
        early_response = resolve_query_target(
            state,
            answerability=answerability,
            missingness=missingness,
        )
        if early_response is not None:
            return early_response

        execute_query_pattern(state)
        return build_query_graph_response(
            state,
            detail_level=detail_level,
            answerability=answerability,
            missingness=missingness,
        )
    except Exception as exc:
        if is_sqlite_corrupt_error(exc) and not _corrupt_retried:
            recover_corrupt_graph(_db_path_for_repo(repo_root))
            logger.warning(
                "query_graph: sqlite corrupt (%s); retrying after closing live stores",
                exc,
            )
            return query_graph(
                pattern,
                target,
                repo_root,
                detail_level,
                _corrupt_retried=True,
            )
        return handle_tool_runtime_error(
            exc,
            logger=logger,
            context="query_graph",
            repo_root=repo_root,
        )
    finally:
        if store is not None:
            store.close()


# ---------------------------------------------------------------------------
# Tool 5: semantic_search_nodes
# ---------------------------------------------------------------------------


#: Server-side ceiling for ``semantic_search_nodes``. Response size scales
#: linearly with the graph, so an unbounded caller limit returns megabytes.
_MAX_SEARCH_LIMIT = 200


def semantic_search_nodes(
    query: str,
    kind: str | None = None,
    limit: int = 20,
    repo_root: str | None = None,
    context_files: list[str] | None = None,
    model: str | None = None,
    provider: str | None = None,
    detail_level: str = "standard",
) -> dict[str, Any]:
    """Search for nodes by name, keyword, or semantic similarity.

    Uses hybrid search (FTS5 BM25 + vector embeddings merged via Reciprocal
    Rank Fusion) as the primary search path, with graceful fallback to
    keyword matching.

    Args:
        query: Search string to match against node names and qualified names.
        kind: Optional filter by node kind (File, Class, Function, Type, Test).
        limit: Maximum results to return (default: 20).
        repo_root: Repository root path. Auto-detected if omitted.
        context_files: Optional list of file paths. Nodes in these files
            receive a relevance boost.
        detail_level: "standard" (full output) or "minimal" (summary only).

    Returns:
        Ranked list of matching nodes.
    """
    store = None
    try:
        # An out-of-range ``limit`` used to produce a *claim about the graph*:
        # limit=0 returned zero_result_reason "not_found_in_current_graph", i.e.
        # "this symbol is absent", caused purely by the caller's argument.
        # limit=-1 dropped the last result via a negative slice while still
        # reporting truncated=True.
        if not isinstance(limit, int) or isinstance(limit, bool) or limit < 1:
            return _error_response(
                f"limit must be an integer >= 1 (got {limit!r})",
                status="error",
                limit=limit,
            )
        if limit > _MAX_SEARCH_LIMIT:
            limit = _MAX_SEARCH_LIMIT
        store, root = _get_store(repo_root)
        answerability = graph_answerability_summary(store)
        missingness = missingness_from_answerability(answerability)
        hs = hybrid_search(
            store,
            query,
            kind=kind,
            limit=limit,
            context_files=context_files,
            model=model,
            provider=provider,
        )
        results = hs["results"]
        search_mode = hs["mode"]
        embedding_health = hs.get("embedding_health", {})
        truncated = bool(hs.get("truncated", False))
        total = int(hs.get("total", len(results)))
        if embedding_health and not embedding_health_available(embedding_health):
            missingness.append(
                {
                    "reason_code": "missing_embeddings",
                    "severity": "medium",
                    "claim_effect": "semantic ranking may be keyword-only",
                }
            )
        partial_coverage = _partial_coverage_missingness(embedding_health)
        if partial_coverage is not None:
            missingness.append(partial_coverage)

        summary = f"Found {len(results)} node(s) matching '{query}'" + (
            f" (kind={kind})" if kind else ""
        )
        result_count = len(results)
        confidence = "medium" if results else "low"
        zero_result_reason = None if results else "not_found_in_current_graph"
        exact_matches = [
            r
            for r in results
            if query in {str(r.get("name", "")), str(r.get("qualified_name", ""))}
        ]
        ambiguity = "multiple_exact_matches" if len(exact_matches) > 1 else None
        next_action = exactness_action(query, len(exact_matches), len(results))
        guidance = _semantic_search_guidance(
            query=query,
            result_count=result_count,
            search_mode=search_mode,
            embedding_health=embedding_health if isinstance(embedding_health, dict) else {},
        )

        if detail_level == "minimal":
            minimal_results = [
                {
                    **{k: r[k] for k in ("name", "kind", "file_path", "score") if k in r},
                    "evidence_type": result_evidence_type(r),
                }
                for r in results[:5]
            ]
            hints = guidance_actions_to_hints(guidance)
            return {
                "status": "ok",
                "query": query,
                "search_mode": search_mode,
                "embedding_health": embedding_health,
                "answerability": answerability,
                "missingness": missingness,
                "result_count": result_count,
                "truncated": truncated,
                "total": total,
                "confidence": confidence,
                "zero_result_reason": zero_result_reason,
                "next_action": next_action,
                "exactness": {
                    "exact_match_count": len(exact_matches),
                    "ambiguity": ambiguity,
                    "source_arm": search_mode,
                    "next_action": next_action,
                },
                "summary": summary,
                "results": minimal_results,
                "guidance": guidance,
                "_hints": hints
                if hints["next_steps"]
                else generate_hints(
                    "semantic_search_nodes", {"status": "ok", "summary": summary}, get_session()
                ),
            }

        result: dict[str, object] = {
            "status": "ok",
            "query": query,
            "search_mode": search_mode,
            "embedding_health": embedding_health,
            "answerability": answerability,
            "missingness": missingness,
            "result_count": result_count,
            "truncated": truncated,
            "total": total,
            "confidence": confidence,
            "zero_result_reason": zero_result_reason,
            "next_action": next_action,
            "exactness": {
                "exact_match_count": len(exact_matches),
                "ambiguity": ambiguity,
                "source_arm": search_mode,
                "next_action": next_action,
            },
            "summary": summary,
            "results": results,
            "guidance": guidance,
        }
        hints = guidance_actions_to_hints(guidance)
        result["_hints"] = (
            hints
            if hints["next_steps"]
            else generate_hints("semantic_search_nodes", result, get_session())
        )
        return result
    except Exception as exc:
        return handle_tool_runtime_error(
            exc, logger=logger, context="semantic_search_nodes", repo_root=repo_root
        )
    finally:
        if store is not None:
            store.close()


# ---------------------------------------------------------------------------
# Tool 6: list_graph_stats
# ---------------------------------------------------------------------------


def list_graph_stats(repo_root: str | None = None) -> dict[str, Any]:
    """Get aggregate statistics about the knowledge graph.

    Args:
        repo_root: Repository root path. Auto-detected if omitted.

    Returns:
        Total nodes, edges, breakdown by kind, languages, and last update time.
    """
    store = None
    try:
        store, root = _get_store(repo_root)
        stats = store.get_stats()

        # Add embedding info if available
        emb_store = EmbeddingStore(get_db_path(root))
        try:
            emb_count = emb_store.count()
        finally:
            emb_store.close()

        return make_response(
            "ok",
            (
                f"Graph stats for {root.name}: {stats.total_nodes} nodes, "
                f"{stats.total_edges} edges, {stats.files_count} files, "
                f"{len(stats.languages)} language(s), {emb_count} embedding(s)."
            ),
            total_nodes=stats.total_nodes,
            total_edges=stats.total_edges,
            nodes_by_kind=stats.nodes_by_kind,
            edges_by_kind=stats.edges_by_kind,
            languages=stats.languages,
            files_count=stats.files_count,
            last_updated=stats.last_updated,
            embeddings_count=emb_count,
            next_tool_suggestions=[
                'architecture_analysis_tool mode="communities" -- inspect structure',
                'flow_tool mode="list" -- inspect critical reachable-set flows',
                "semantic_search_nodes_tool -- search for specific entities",
            ],
        )
    except Exception as exc:
        return handle_tool_runtime_error(
            exc, logger=logger, context="list_graph_stats", repo_root=repo_root
        )
    finally:
        if store is not None:
            store.close()


# ---------------------------------------------------------------------------
# Tool 9: find_large_functions
# ---------------------------------------------------------------------------


def find_large_functions(
    min_lines: int = 50,
    kind: str | None = None,
    file_path_pattern: str | None = None,
    limit: int = 50,
    repo_root: str | None = None,
) -> dict[str, Any]:
    """Find functions, classes, or files exceeding a line-count threshold.

    Useful for identifying decomposition targets, code-quality audits,
    and enforcing size limits during code review.

    Args:
        min_lines: Minimum line count to flag (default: 50).
        kind: Filter by node kind: Function, Class, File, or Test.
        file_path_pattern: Filter by file path substring (e.g. "components/").
        limit: Maximum results (default: 50).
        repo_root: Repository root path. Auto-detected if omitted.

    Returns:
        Oversized nodes with line counts, ordered largest first.
    """
    store = None
    try:
        store, root = _get_store(repo_root)
        nodes = store.get_nodes_by_size(
            min_lines=min_lines,
            kind=kind,
            file_path_pattern=file_path_pattern,
            limit=limit,
        )

        results = []
        for n in nodes:
            d = node_to_dict(n)
            d["line_count"] = (n.line_end - n.line_start + 1) if n.line_start and n.line_end else 0
            # Make file_path relative for readability
            try:
                rel = (
                    n.file_path
                    if not Path(n.file_path).is_absolute()
                    else str(Path(n.file_path).relative_to(root))
                )
            except ValueError:
                rel = n.file_path
            d["relative_path"] = rel
            # For File nodes the name IS the absolute path — replace with relative
            if n.kind == "File" and Path(d.get("name", "")).is_absolute():
                d["name"] = rel
            results.append(d)

        summary_parts = [
            f"Found {len(results)} node(s) with >= {min_lines} lines"
            + (f" (kind={kind})" if kind else "")
            + (f" matching '{file_path_pattern}'" if file_path_pattern else "")
            + ":",
        ]
        for r in results[:10]:
            summary_parts.append(
                f"  {r['line_count']:>4} lines | {r['kind']:>8} | "
                f"{r['name']} ({r['relative_path']}:{r['line_start']})"
            )
        if len(results) > 10:
            summary_parts.append(f"  ... and {len(results) - 10} more")

        return {
            "status": "ok",
            "summary": "\n".join(summary_parts),
            "total_found": len(results),
            "min_lines": min_lines,
            "results": results,
        }
    except Exception as exc:
        return handle_tool_runtime_error(
            exc, logger=logger, context="find_large_functions", repo_root=repo_root
        )
    finally:
        if store is not None:
            store.close()


# -------------------------------------------------------------------
# traverse_graph: free-form BFS / DFS traversal
# -------------------------------------------------------------------


def _estimate_traversal_entry_tokens(entry: Mapping[str, Any]) -> int:
    return (len(entry["qualified_name"]) + len(entry["file"]) + len(entry["name"]) + 30) // 4


def _traverse_dfs_lazy(
    store: Any,
    start_qn: str,
    depth: int,
    token_budget: int,
    make_entry: Any,
) -> tuple[dict[str, int], list[TraversalEntry], bool, list[str]]:
    """Depth-first traversal that hydrates only nodes it actually visits."""
    visited: dict[str, int] = {}
    traversal: list[TraversalEntry] = []
    traversal_index: dict[str, int] = {}
    unresolved_targets: list[str] = []
    unresolved_seen: set[str] = set()
    approx_tokens = 0
    budget_exceeded = False
    node_cache: dict[str, Any | None] = {}
    neighbor_cache: dict[str, list[str]] = {}
    stack: list[tuple[str, int]] = [(start_qn, 0)]

    def _get_node(qn: str) -> Any | None:
        if qn not in node_cache:
            node_cache[qn] = store.get_nodes_by_qualified_names([qn]).get(qn)
        return node_cache[qn]

    def _get_neighbors(qn: str) -> list[str]:
        if qn not in neighbor_cache:
            outgoing, incoming = store.get_edges_by_endpoints([qn])
            neighbors = [edge.target_qualified for edge in outgoing.get(qn, [])]
            neighbors.extend(edge.source_qualified for edge in incoming.get(qn, []))
            neighbor_cache[qn] = neighbors
        return neighbor_cache[qn]

    while stack and not budget_exceeded:
        current_qn, cur_depth = stack.pop()
        if cur_depth > depth:
            continue
        prev_depth = visited.get(current_qn)
        if prev_depth is not None and cur_depth >= prev_depth:
            continue

        node = _get_node(current_qn)
        if not node:
            visited[current_qn] = cur_depth
            if current_qn not in unresolved_seen:
                unresolved_targets.append(current_qn)
                unresolved_seen.add(current_qn)
            continue

        visited[current_qn] = cur_depth
        entry = make_entry(node, cur_depth)
        approx_tokens += _estimate_traversal_entry_tokens(entry)
        if approx_tokens > token_budget:
            budget_exceeded = True
            break
        if current_qn in traversal_index:
            traversal[traversal_index[current_qn]] = entry
        else:
            traversal_index[current_qn] = len(traversal)
            traversal.append(entry)

        if cur_depth + 1 > depth:
            continue
        neighbors = _get_neighbors(current_qn)
        for nb in reversed(neighbors):
            nb_prev = visited.get(nb)
            if nb_prev is None or cur_depth + 1 < nb_prev:
                stack.append((nb, cur_depth + 1))

    return visited, traversal, budget_exceeded, unresolved_targets


def traverse_graph_func(
    query: str,
    mode: TraversalMode = "bfs",
    depth: int = 3,
    token_budget: int = 2000,
    repo_root: str | None = None,
    model: str | None = None,
    provider: str | None = None,
) -> dict[str, Any]:
    """BFS/DFS traversal from best-matching node.

    Args:
        query: Search string to find the starting node.
        mode: "bfs" (breadth-first) or "dfs" (depth-first).
        depth: Max traversal depth (1-6). Default: 3.
        token_budget: Approximate token limit for results.
        repo_root: Repository root path.
        model: Embedding model for the initial hybrid search.
        provider: Embedding provider for the initial hybrid search.
    """
    store = None
    try:
        store, root = _get_store(repo_root)
        results = hybrid_search(
            store,
            query,
            limit=1,
            model=model,
            provider=provider,
        )["results"]
        if not results:
            reachability: dict[str, Any] = seal_reachability_info(
                {
                    "state": "not_found",
                    "truncated": False,
                    "max_depth": max(1, min(depth, 6)),
                    "nodes_visited": 0,
                }
            )
            return make_response(
                "not_found",
                f"No node matching '{query}'.",
                start_node=None,
                mode=mode,
                max_depth=max(1, min(depth, 6)),
                nodes_visited=0,
                traversal=[],
                truncated=False,
                reachability=reachability,
                next_tool_suggestions=[
                    "semantic_search_nodes_tool -- search more broadly for the symbol",
                    "query_graph_tool -- inspect a known qualified name directly",
                ],
            )

        start_qn = results[0]["qualified_name"]
        depth = max(1, min(depth, 6))

        # Traversal state shared by both modes.
        visited: dict[str, int] = {}
        traversal: list[TraversalEntry] = []
        unresolved_targets: list[str] = []
        unresolved_seen: set[str] = set()
        approx_tokens = 0
        budget_exceeded = False

        def _make_entry(node: Any, cur_depth: int) -> TraversalEntry:
            return {
                "name": _sanitize_name(node.name),
                "qualified_name": node.qualified_name,
                "kind": node.kind,
                "file": node.file_path,
                "depth": cur_depth,
            }

        if mode == "dfs":
            visited, traversal, budget_exceeded, unresolved_targets = _traverse_dfs_lazy(
                store,
                start_qn,
                depth,
                token_budget,
                _make_entry,
            )
        else:
            # BFS — process the entire current frontier in one batched
            # node + edge fetch per layer, instead of issuing 3 SQL
            # queries per visited node.
            current_frontier: list[str] = [start_qn]
            cur_depth = 0
            while current_frontier and cur_depth <= depth and not budget_exceeded:
                frontier_unique: list[str] = []
                seen_in_layer: set[str] = set()
                for qn in current_frontier:
                    if qn in visited or qn in seen_in_layer:
                        continue
                    seen_in_layer.add(qn)
                    frontier_unique.append(qn)

                if not frontier_unique:
                    break

                nodes_by_qn = store.get_nodes_by_qualified_names(frontier_unique)
                outgoing, incoming = store.get_edges_by_endpoints(frontier_unique)

                next_frontier: list[str] = []
                for current_qn in frontier_unique:
                    if current_qn in visited:
                        continue
                    visited[current_qn] = cur_depth
                    node = nodes_by_qn.get(current_qn)
                    if not node:
                        if current_qn not in unresolved_seen:
                            unresolved_targets.append(current_qn)
                            unresolved_seen.add(current_qn)
                        continue

                    entry = _make_entry(node, cur_depth)
                    approx_tokens += _estimate_traversal_entry_tokens(entry)
                    if approx_tokens > token_budget:
                        budget_exceeded = True
                        break

                    traversal.append(entry)

                    if cur_depth + 1 > depth:
                        continue
                    for e in outgoing.get(current_qn, []):
                        tgt = e.target_qualified
                        if tgt not in visited:
                            next_frontier.append(tgt)
                    for e in incoming.get(current_qn, []):
                        src = e.source_qualified
                        if src not in visited:
                            next_frontier.append(src)

                current_frontier = next_frontier
                cur_depth += 1

        unresolved_count = len(unresolved_targets)
        reachability_state = "truncated" if budget_exceeded or unresolved_count else "complete"
        reachability = seal_reachability_info(
            {
                "state": reachability_state,
                "truncated": budget_exceeded or bool(unresolved_count),
                "max_depth": depth,
                "nodes_visited": len(traversal),
                "unresolved_count": unresolved_count,
                "unresolved_targets": unresolved_targets,
            }
        )
        summary_suffix = ""
        if budget_exceeded:
            summary_suffix = " Output was truncated to fit the token budget."
        elif unresolved_count:
            summary_suffix = f" Traversal stopped at {unresolved_count} unresolvable endpoint(s)."
        return make_response(
            "ok",
            f"Traversed {len(traversal)} node(s) from '{start_qn}' up to depth {depth}."
            + summary_suffix,
            start_node=start_qn,
            mode=mode,
            max_depth=depth,
            nodes_visited=len(traversal),
            traversal=traversal,
            truncated=budget_exceeded,
            reachability=reachability,
            next_tool_suggestions=[
                "query_graph_tool callers_of -- focused relationship query",
                'review_tool mode="impact" -- blast radius analysis',
            ],
        )
    except Exception as exc:
        return handle_tool_runtime_error(
            exc, logger=logger, context="traverse_graph", repo_root=repo_root
        )
    finally:
        if store is not None:
            store.close()
