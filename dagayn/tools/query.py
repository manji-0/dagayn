"""Tools 2, 3, 5, 6, 9: query / search / stats helpers."""

from __future__ import annotations

import logging
from collections.abc import Mapping
from pathlib import Path
from typing import Any

from ..coverage import infer_tests_for_node
from ..embeddings import EmbeddingStore
from ..graph import _sanitize_name, edge_to_dict, node_to_dict
from ..hints import generate_hints, get_session
from ..incremental import get_changed_files, get_db_path, get_staged_and_unstaged
from ..search import hybrid_search
from ..state_types import TraversalEntry, TraversalMode, seal_reachability_info
from ._common import (
    _BUILTIN_CALL_NAMES,
    _get_store,
    apply_output_budget,
    graph_answerability_summary,
    guidance_actions_to_hints,
    handle_tool_runtime_error,
    make_guidance_item,
    make_response,
    missingness_from_answerability,
)

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Tool 2: get_impact_radius
# ---------------------------------------------------------------------------

_QUERY_PATTERNS = {
    "callers_of": "Find all functions that call a given function",
    "callees_of": "Find all functions called by a given function",
    "imports_of": "Find all imports of a given file or module",
    "importers_of": "Find all files that import a given file or module",
    "docs_for": "Find documentation linked to a code, Terraform, or artifact node",
    "implementations_of": "Find implementation artifacts linked to a document node",
    "children_of": "Find all nodes contained in a file or class",
    "tests_for": "Find all tests for a given function or class",
    "inheritors_of": "Find all classes that inherit from a given class",
    "file_summary": "Get a summary of all nodes in a file",
}

_DOC_TO_ARTIFACT_ROLES = {
    "implemented_by": "implements_contract",
    "describes_symbol": "described_by",
    "discusses_artifact": "discussed_by",
    "raises_issue_for": "has_issue_note",
}

_ARTIFACT_TO_DOC_ROLES = {
    "implements_contract": "implemented_by",
    "explained_by": "explains",
    "has_runbook": "runbook_for",
    "problem_described_by": "describes_problem_in",
    "discussed_by": "discusses",
}


def _node_dicts_for_edges(
    store: Any,
    edges: list[Any],
    *,
    qualified_attr: str,
) -> list[dict[str, Any]]:
    """Batch-resolve edge endpoints to node dicts while preserving edge order."""
    if not edges:
        return []

    qualified_names = [getattr(edge, qualified_attr) for edge in edges]
    nodes_by_qn = store.get_nodes_by_qualified_names(qualified_names)
    results: list[dict[str, Any]] = []
    for edge in edges:
        node = nodes_by_qn.get(getattr(edge, qualified_attr))
        if node is not None:
            results.append(node_to_dict(node))
    return results


def _cross_artifact_role(edge: Any) -> str | None:
    if edge.kind != "CROSS_ARTIFACT":
        return None
    extra = edge.extra if isinstance(edge.extra, dict) else {}
    role = extra.get("relationship_role")
    return role if isinstance(role, str) else None


def _is_low_confidence_unresolved_markdown_code_span(edge: Any) -> bool:
    if getattr(edge, "kind", None) != "CROSS_ARTIFACT":
        return False
    extra = getattr(edge, "extra", None)
    if not isinstance(extra, dict):
        return False
    role = extra.get("relationship_role")
    target = str(getattr(edge, "target_qualified", ""))
    tier = str(getattr(edge, "confidence_tier", "") or extra.get("confidence_tier", "")).upper()
    return role == "describes_symbol" and target.startswith("<unresolved:") and tier == "LOW"


def _documentation_result(edge: Any, *, endpoint: str, inverse_label: str | None = None) -> dict:
    role = _cross_artifact_role(edge)
    confidence_tier = str(edge.confidence_tier or "").upper()
    evidence_type = "authored" if role in {"implements_contract", "implemented_by"} else "extracted"
    if role not in {"implements_contract", "implemented_by"} and confidence_tier not in {
        "EXTRACTED",
        "HIGH",
    }:
        evidence_type = "heuristic_reachable"
    result = {
        "source": edge.source_qualified,
        "target": edge.target_qualified,
        "matched_endpoint": endpoint,
        "relationship_role": role,
        "evidence_type": evidence_type,
        "file": edge.file_path,
        "line": edge.line,
        "confidence": edge.confidence,
        "confidence_tier": edge.confidence_tier,
    }
    if inverse_label:
        result["inverse_label"] = inverse_label
    return result


def _result_evidence_type(result: dict[str, Any]) -> str:
    if result.get("evidence_type"):
        return str(result["evidence_type"])
    kind = str(result.get("kind", ""))
    file_path = str(result.get("file_path") or result.get("file") or "")
    if kind.startswith("Doc") or file_path.lower().endswith((".md", ".markdown", ".mdx")):
        return "authored"
    return "extracted"


def _exactness_action(query: str, exact_count: int, result_count: int) -> dict[str, Any]:
    if exact_count == 1:
        return {
            "tool": "query_graph_tool",
            "suggestion": "inspect callers_of/callees_of for the exact qualified match",
        }
    if exact_count > 1:
        return {
            "tool": "semantic_search_nodes_tool",
            "suggestion": f"choose one qualified name before querying relationships for '{query}'",
        }
    if result_count:
        return {
            "tool": "query_graph_tool",
            "suggestion": "confirm the best candidate with file_summary or callers_of",
        }
    return {
        "tool": "semantic_search_nodes_tool",
        "suggestion": "broaden the query or verify the graph is up to date",
    }


def _query_graph_guidance(
    *,
    pattern: str,
    target: str,
    result_count: int,
    exact_count: int,
) -> list[dict[str, Any]]:
    if result_count:
        return [
            make_guidance_item(
                claim=(
                    f"Graph query '{pattern}' returned {result_count} related node(s) "
                    f"for '{target}'."
                ),
                evidence={
                    "type": "computed",
                    "pattern": pattern,
                    "target": target,
                    "result_count": result_count,
                    "exact_match_count": exact_count,
                },
                confidence="medium",
                missingness=[
                    {
                        "reason_code": "relationship_query_not_runtime_proof",
                        "severity": "low",
                        "claim_effect": ("graph edges are static extraction, not runtime traces"),
                    }
                ],
                action=f'query_graph_tool pattern="{pattern}" -- drill into a relationship',
                reason_codes=["graph_relationship_query"],
                counts={"result_count": result_count},
            )
        ]
    return [
        make_guidance_item(
            claim=f"No graph relationships matched '{pattern}' for '{target}'.",
            evidence={
                "type": "computed",
                "pattern": pattern,
                "target": target,
                "result_count": 0,
            },
            confidence="low",
            missingness=[
                {
                    "reason_code": "not_found_in_current_graph",
                    "severity": "medium",
                    "claim_effect": (
                        "absence is graph-limited, not proof the relationship does not exist"
                    ),
                }
            ],
            action="semantic_search_nodes_tool -- verify the target and refresh the graph",
            reason_codes=["zero_result"],
            counts={"result_count": 0},
        )
    ]


def _semantic_search_guidance(
    *,
    query: str,
    result_count: int,
    search_mode: str,
    embedding_health: dict[str, Any],
) -> list[dict[str, Any]]:
    missingness_items: list[dict[str, Any]] = []
    if embedding_health and not embedding_health.get("available", True):
        missingness_items.append(
            {
                "reason_code": "missing_embeddings",
                "severity": "medium",
                "claim_effect": "semantic ranking may be keyword-only",
            }
        )
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
        truncated = result["truncated"]
        total_impacted = result["total_impacted"]

        summary_parts = [
            f"Blast radius for {len(changed_files)} changed file(s):",
            f"  - {len(changed_dicts)} nodes directly changed",
            f"  - {len(impacted_dicts)} nodes impacted (within {max_depth} hops)",
            f"  - {len(result['impacted_files'])} additional files affected",
        ]
        if truncated:
            summary_parts.append(
                f"  - Results truncated: showing {len(impacted_dicts)}"
                f" of {total_impacted} impacted nodes"
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
                "impacted_file_count": len(result["impacted_files"]),
                "key_entities": key_entities,
                "truncated": truncated,
                "answerability": answerability,
                "missingness": missingness,
            }

        payload = {
            "status": "ok",
            "summary": "\n".join(summary_parts),
            "changed_files": changed_files,
            "changed_nodes": changed_dicts,
            "impacted_nodes": impacted_dicts,
            "impacted_files": result["impacted_files"],
            "edges": edge_dicts,
            "truncated": truncated,
            "total_impacted": total_impacted,
            "answerability": answerability,
            "missingness": missingness,
        }
        apply_output_budget(
            payload,
            budget_tokens=8000,
            list_priorities=[
                "changed_files",
                "impacted_files",
                "changed_nodes",
                "impacted_nodes",
                "edges",
            ],
        )
        return payload
    except Exception as exc:
        return handle_tool_runtime_error(exc, logger=logger, context="get_impact_radius")
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
) -> dict[str, Any]:
    """Run a predefined graph query.

    Args:
        pattern: Query pattern. One of: callers_of, callees_of, imports_of,
                 importers_of, docs_for, implementations_of, children_of,
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
        if pattern not in _QUERY_PATTERNS:
            return {
                "status": "error",
                "error": (
                    f"Unknown pattern '{pattern}'. Available: {list(_QUERY_PATTERNS.keys())}"
                ),
            }

        results: list[dict] = []
        edges_out: list[dict] = []

        # For callers_of, skip common builtins early (bare names only)
        # "Who calls .map()?" returns hundreds of useless hits.
        # Qualified names (e.g. "utils.py::map") bypass this filter.
        if pattern == "callers_of" and target in _BUILTIN_CALL_NAMES and "::" not in target:
            return {
                "status": "ok",
                "pattern": pattern,
                "target": target,
                "description": _QUERY_PATTERNS[pattern],
                "summary": (f"'{target}' is a common builtin — callers_of skipped to avoid noise."),
                "results": [],
                "edges": [],
                "answerability": answerability,
                "missingness": missingness,
            }

        # Resolve target - try as-is, then as absolute path, then search
        node = store.get_node(target)
        if not node:
            abs_target = str(root / target)
            node = store.get_node(abs_target)
        if not node:
            # Search by name
            candidates = store.search_nodes(target, limit=5)
            if len(candidates) == 1:
                node = candidates[0]
                target = node.qualified_name
            elif len(candidates) > 1:
                return {
                    "status": "ambiguous",
                    "summary": (f"Multiple matches for '{target}'. Please use a qualified name."),
                    "candidates": [node_to_dict(c) for c in candidates],
                    "answerability": answerability,
                    "missingness": [
                        *missingness,
                        {
                            "reason_code": "ambiguous_target",
                            "severity": "medium",
                            "claim_effect": "relationship query was not run for a unique node",
                        },
                    ],
                }

        if not node and pattern != "file_summary":
            guidance = _query_graph_guidance(
                pattern=pattern,
                target=target,
                result_count=0,
                exact_count=0,
            )
            return {
                "status": "not_found",
                "summary": f"No node found matching '{target}' in the current graph.",
                "result_count": 0,
                "results": [],
                "zero_result_reason": "target_not_found_in_graph",
                "next_action": _exactness_action(target, 0, 0),
                "answerability": answerability,
                "missingness": [
                    *missingness,
                    {
                        "reason_code": "target_not_found_in_graph",
                        "severity": "medium",
                        "claim_effect": (
                            "absence is graph-limited, not proof the symbol does not exist"
                        ),
                    },
                ],
                "guidance": guidance,
                "_hints": guidance_actions_to_hints(guidance),
            }

        qn = node.qualified_name if node else target

        if pattern == "callers_of":
            call_edges = [e for e in store.get_edges_by_target(qn) if e.kind == "CALLS"]
            results.extend(
                _node_dicts_for_edges(store, call_edges, qualified_attr="source_qualified")
            )
            edges_out.extend(edge_to_dict(e) for e in call_edges)
            # Fallback: CALLS edges store unqualified target names
            # (e.g. "generateTestCode") while qn is fully qualified
            # (e.g. "file.ts::generateTestCode"). Search by plain name too.
            if not results and node:
                fallback_edges = store.search_edges_by_target_name(node.name)
                results.extend(
                    _node_dicts_for_edges(
                        store,
                        fallback_edges,
                        qualified_attr="source_qualified",
                    )
                )
                edges_out.extend(edge_to_dict(e) for e in fallback_edges)

        elif pattern == "callees_of":
            call_edges = [e for e in store.get_edges_by_source(qn) if e.kind == "CALLS"]
            results.extend(
                _node_dicts_for_edges(store, call_edges, qualified_attr="target_qualified")
            )
            edges_out.extend(edge_to_dict(e) for e in call_edges)

        elif pattern == "imports_of":
            for e in store.get_edges_by_source(qn):
                if e.kind == "IMPORTS_FROM":
                    results.append({"import_target": e.target_qualified})
                    edges_out.append(edge_to_dict(e))

        elif pattern == "importers_of":
            # Find edges where target matches this file.
            # Use resolve() to canonicalize the path, matching how
            # _resolve_module_to_file stores edge targets.
            abs_target = str((root / target).resolve()) if node is None else node.file_path
            for e in store.get_edges_by_target(abs_target):
                if e.kind == "IMPORTS_FROM":
                    results.append(
                        {
                            "importer": e.source_qualified,
                            "file": e.file_path,
                        }
                    )
                    edges_out.append(edge_to_dict(e))

        elif pattern == "docs_for":
            for e in store.get_edges_by_source(qn):
                if _is_low_confidence_unresolved_markdown_code_span(e):
                    continue
                role = _cross_artifact_role(e)
                if role in _ARTIFACT_TO_DOC_ROLES:
                    results.append(
                        _documentation_result(
                            e,
                            endpoint=e.target_qualified,
                            inverse_label=_ARTIFACT_TO_DOC_ROLES[role],
                        )
                    )
                    edges_out.append(edge_to_dict(e))
            for e in store.get_edges_by_target(qn):
                if _is_low_confidence_unresolved_markdown_code_span(e):
                    continue
                role = _cross_artifact_role(e)
                if role in _DOC_TO_ARTIFACT_ROLES:
                    results.append(
                        _documentation_result(
                            e,
                            endpoint=e.source_qualified,
                            inverse_label=_DOC_TO_ARTIFACT_ROLES[role],
                        )
                    )
                    edges_out.append(edge_to_dict(e))

        elif pattern == "implementations_of":
            for e in store.get_edges_by_source(qn):
                if _is_low_confidence_unresolved_markdown_code_span(e):
                    continue
                role = _cross_artifact_role(e)
                if role == "implemented_by":
                    results.append(_documentation_result(e, endpoint=e.target_qualified))
                    edges_out.append(edge_to_dict(e))
            for e in store.get_edges_by_target(qn):
                if _is_low_confidence_unresolved_markdown_code_span(e):
                    continue
                role = _cross_artifact_role(e)
                if role == "implements_contract":
                    results.append(
                        _documentation_result(
                            e,
                            endpoint=e.source_qualified,
                            inverse_label="implemented_by",
                        )
                    )
                    edges_out.append(edge_to_dict(e))

        elif pattern == "children_of":
            child_edges = [e for e in store.get_edges_by_source(qn) if e.kind == "CONTAINS"]
            results.extend(
                _node_dicts_for_edges(store, child_edges, qualified_attr="target_qualified")
            )

        elif pattern == "tests_for":
            if node is not None:
                results.extend(infer_tests_for_node(store, node))
                test_edges = [e for e in store.get_edges_by_source(qn) if e.kind == "TESTED_BY"]
                edges_out.extend(edge_to_dict(e) for e in test_edges)

        elif pattern == "inheritors_of":
            inherit_edges = [
                e for e in store.get_edges_by_target(qn) if e.kind in ("INHERITS", "IMPLEMENTS")
            ]
            results.extend(
                _node_dicts_for_edges(store, inherit_edges, qualified_attr="source_qualified")
            )
            edges_out.extend(edge_to_dict(e) for e in inherit_edges)
            # Fallback: INHERITS/IMPLEMENTS edges store unqualified base names
            # (e.g. "Animal") while qn is fully qualified
            # (e.g. "sample.dart::Animal"). Search by plain name too. See: #87
            if not results and node:
                fallback_edges = []
                for kind in ("INHERITS", "IMPLEMENTS"):
                    fallback_edges.extend(store.search_edges_by_target_name(node.name, kind=kind))
                results.extend(
                    _node_dicts_for_edges(
                        store,
                        fallback_edges,
                        qualified_attr="source_qualified",
                    )
                )
                edges_out.extend(edge_to_dict(e) for e in fallback_edges)

        elif pattern == "file_summary":
            abs_path = str(root / target)
            file_nodes = store.get_nodes_by_file(abs_path)
            for n in file_nodes:
                results.append(node_to_dict(n))

        summary = f"Found {len(results)} result(s) for {pattern}('{target}')"

        if detail_level == "minimal":
            minimal_results = [
                {
                    k: r[k]
                    for k in ("name", "kind", "file_path", "confidence", "coverage_source")
                    + (
                        "source",
                        "target",
                        "matched_endpoint",
                        "relationship_role",
                        "inverse_label",
                        "evidence_type",
                        "file",
                    )
                    if k in r
                }
                for r in results[:5]
            ]
            for item in minimal_results:
                item["evidence_type"] = _result_evidence_type(item)
            guidance = _query_graph_guidance(
                pattern=pattern,
                target=target,
                result_count=len(results),
                exact_count=1 if node else 0,
            )
            return {
                "status": "ok",
                "pattern": pattern,
                "target": target,
                "description": _QUERY_PATTERNS[pattern],
                "summary": summary,
                "result_count": len(results),
                "confidence": "medium" if results else "low",
                "zero_result_reason": None if results else "not_found_in_current_graph",
                "next_action": _exactness_action(target, 1 if node else 0, len(results)),
                "answerability": answerability,
                "missingness": missingness,
                "results": minimal_results,
                "guidance": guidance,
                "_hints": guidance_actions_to_hints(guidance),
            }

        guidance = _query_graph_guidance(
            pattern=pattern,
            target=target,
            result_count=len(results),
            exact_count=1 if node else 0,
        )
        payload = {
            "status": "ok",
            "pattern": pattern,
            "target": target,
            "description": _QUERY_PATTERNS[pattern],
            "summary": summary,
            "result_count": len(results),
            "confidence": "medium" if results else "low",
            "zero_result_reason": None if results else "not_found_in_current_graph",
            "next_action": _exactness_action(target, 1 if node else 0, len(results)),
            "answerability": answerability,
            "missingness": missingness,
            "results": results,
            "edges": edges_out,
            "guidance": guidance,
            "_hints": guidance_actions_to_hints(guidance),
        }
        apply_output_budget(payload, budget_tokens=8000, list_priorities=["results", "edges"])
        return payload
    except Exception as exc:
        return handle_tool_runtime_error(exc, logger=logger, context="query_graph")
    finally:
        if store is not None:
            store.close()


# ---------------------------------------------------------------------------
# Tool 5: semantic_search_nodes
# ---------------------------------------------------------------------------


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
        if embedding_health and not embedding_health.get("available", True):
            missingness.append(
                {
                    "reason_code": "missing_embeddings",
                    "severity": "medium",
                    "claim_effect": "semantic ranking may be keyword-only",
                }
            )

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
        next_action = _exactness_action(query, len(exact_matches), len(results))
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
                    "evidence_type": _result_evidence_type(r),
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
        return handle_tool_runtime_error(exc, logger=logger, context="semantic_search_nodes")
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
                'flow_tool mode="list" -- inspect critical execution paths',
                "semantic_search_nodes_tool -- search for specific entities",
            ],
        )
    except Exception as exc:
        return handle_tool_runtime_error(exc, logger=logger, context="list_graph_stats")
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
        return handle_tool_runtime_error(exc, logger=logger, context="find_large_functions")
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
) -> tuple[dict[str, int], list[TraversalEntry], bool]:
    """Depth-first traversal that hydrates only nodes it actually visits."""
    visited: dict[str, int] = {}
    traversal: list[TraversalEntry] = []
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
        if current_qn in visited or cur_depth > depth:
            continue

        node = _get_node(current_qn)
        if not node:
            visited[current_qn] = cur_depth
            continue

        visited[current_qn] = cur_depth
        entry = make_entry(node, cur_depth)
        approx_tokens += _estimate_traversal_entry_tokens(entry)
        if approx_tokens > token_budget:
            budget_exceeded = True
            break
        traversal.append(entry)

        if cur_depth + 1 > depth:
            continue
        neighbors = [nb for nb in _get_neighbors(current_qn) if nb not in visited]
        for nb in reversed(neighbors):
            stack.append((nb, cur_depth + 1))

    return visited, traversal, budget_exceeded


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
            visited, traversal, budget_exceeded = _traverse_dfs_lazy(
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

        reachability = seal_reachability_info(
            {
                "state": "truncated" if budget_exceeded else "complete",
                "truncated": budget_exceeded,
                "max_depth": depth,
                "nodes_visited": len(traversal),
            }
        )
        return make_response(
            "ok",
            f"Traversed {len(traversal)} node(s) from '{start_qn}' up to depth {depth}."
            + (" Output was truncated to fit the token budget." if budget_exceeded else ""),
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
        return handle_tool_runtime_error(exc, logger=logger, context="traverse_graph")
    finally:
        if store is not None:
            store.close()
