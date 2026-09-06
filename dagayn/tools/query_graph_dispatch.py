"""Pattern dispatch for ``query_graph``."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from ..coverage import infer_tests_for_node
from ..graph import edge_to_dict, node_to_dict
from ._common import apply_output_budget, guidance_actions_to_hints
from .node_source import SOURCE_OF_MAX_CHARS, read_live_node_source
from .query_graph_support import (
    _ARTIFACT_TO_DOC_ROLES,
    _DOC_TO_ARTIFACT_ROLES,
    _INFRA_TO_CODE_BRIDGE_ROLES,
    QUERY_PATTERNS,
    annotate_bare_name_edges,
    cross_artifact_role,
    documentation_result,
    exactness_action,
    file_is_indexed,
    file_path_candidates,
    filter_bare_name_fallback_edges,
    is_low_confidence_markdown_code_span,
    is_unresolved_import_target,
    looks_like_query_file_target,
    merge_unresolved_targets,
    node_dicts_for_edges,
    query_graph_guidance,
    query_zero_result_fields,
    result_evidence_type,
)


@dataclass
class QueryGraphState:
    store: Any
    root: Path
    pattern: str
    original_target: str
    target: str
    node: Any | None = None
    resolution: str = "exact"
    resolved_target: str | None = None
    results: list[dict[str, Any]] = field(default_factory=list)
    edges_out: list[dict[str, Any]] = field(default_factory=list)
    unresolved_targets: list[str] = field(default_factory=list)

    @property
    def qualified_name(self) -> str:
        return self.node.qualified_name if self.node is not None else self.target


def resolve_query_target(
    state: QueryGraphState,
    *,
    answerability: Mapping[str, Any],
    missingness: Sequence[Mapping[str, Any]],
) -> dict[str, Any] | None:
    """Resolve the query target to a graph node.

    Returns an early response payload when resolution fails, otherwise ``None``.
    """
    node = state.store.get_node(state.target)
    if not node:
        abs_target = str(state.root / state.target)
        node = state.store.get_node(abs_target)
    if not node and state.pattern == "file_summary" and looks_like_query_file_target(state.target):
        if not file_is_indexed(state.store, state.root, state.target):
            guidance = query_graph_guidance(
                pattern=state.pattern,
                target=state.target,
                result_count=0,
                exact_count=0,
            )
            return {
                "status": "not_found",
                "pattern": state.pattern,
                "target": state.target,
                "description": QUERY_PATTERNS[state.pattern],
                "summary": (
                    f"No indexed file found matching '{state.target}' in the current graph."
                ),
                "result_count": 0,
                "results": [],
                "zero_result_reason": "target_not_found_in_graph",
                "next_action": exactness_action(state.target, 0, 0, pattern=state.pattern),
                "answerability": answerability,
                "missingness": [
                    *missingness,
                    {
                        "reason_code": "target_not_found_in_graph",
                        "severity": "medium",
                        "claim_effect": (
                            "absence is graph-limited, not proof the file does not exist"
                        ),
                    },
                ],
                "guidance": guidance,
                "_hints": guidance_actions_to_hints(guidance),
            }
    elif not node and not looks_like_query_file_target(state.target):
        candidates = state.store.search_nodes(state.target, limit=5)
        if len(candidates) == 1:
            node = candidates[0]
            state.resolved_target = node.qualified_name
            state.target = state.resolved_target
            state.resolution = "fuzzy"
        elif len(candidates) > 1:
            return {
                "status": "ambiguous",
                "pattern": state.pattern,
                "target": state.original_target,
                "summary": (
                    f"Multiple matches for '{state.original_target}'. Please use a qualified name."
                ),
                "result_count": 0,
                "results": [],
                "candidates": [node_to_dict(candidate) for candidate in candidates],
                "candidates_truncated": len(candidates) >= 5,
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

    if not node and state.pattern != "file_summary":
        guidance = query_graph_guidance(
            pattern=state.pattern,
            target=state.target,
            result_count=0,
            exact_count=0,
        )
        return {
            "status": "not_found",
            "summary": f"No node found matching '{state.target}' in the current graph.",
            "result_count": 0,
            "results": [],
            "zero_result_reason": "target_not_found_in_graph",
            "next_action": exactness_action(state.target, 0, 0, pattern=state.pattern),
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

    state.node = node
    return None


def _pattern_callers_of(state: QueryGraphState) -> None:
    qn = state.qualified_name
    call_edges = [edge for edge in state.store.get_edges_by_target(qn) if edge.kind == "CALLS"]
    caller_nodes, caller_unresolved = node_dicts_for_edges(
        state.store, call_edges, qualified_attr="source_qualified"
    )
    state.results.extend(caller_nodes)
    merge_unresolved_targets(state.unresolved_targets, caller_unresolved)
    state.edges_out.extend(edge_to_dict(edge) for edge in call_edges)
    if not state.results and state.node is not None:
        fallback_edges = filter_bare_name_fallback_edges(
            state.store,
            state.store.search_edges_by_target_name(state.node.name),
            state.node,
        )
        fallback_nodes, fallback_unresolved = node_dicts_for_edges(
            state.store,
            fallback_edges,
            qualified_attr="source_qualified",
        )
        state.results.extend(fallback_nodes)
        merge_unresolved_targets(state.unresolved_targets, fallback_unresolved)
        state.edges_out.extend(edge_to_dict(edge) for edge in fallback_edges)
        annotate_bare_name_edges(state.edges_out)


def _pattern_callees_of(state: QueryGraphState) -> None:
    qn = state.qualified_name
    call_edges = [edge for edge in state.store.get_edges_by_source(qn) if edge.kind == "CALLS"]
    callee_nodes, callee_unresolved = node_dicts_for_edges(
        state.store, call_edges, qualified_attr="target_qualified"
    )
    state.results.extend(callee_nodes)
    merge_unresolved_targets(state.unresolved_targets, callee_unresolved)
    state.edges_out.extend(edge_to_dict(edge) for edge in call_edges)


def _pattern_imports_of(state: QueryGraphState) -> None:
    qn = state.qualified_name
    for edge in state.store.get_edges_by_source(qn):
        if edge.kind == "IMPORTS_FROM":
            state.results.append(
                {
                    "import_target": edge.target_qualified,
                    "unresolved": is_unresolved_import_target(
                        state.store,
                        edge.target_qualified,
                        state.root,
                    ),
                }
            )
            state.edges_out.append(edge_to_dict(edge))
            if state.results[-1]["unresolved"]:
                merge_unresolved_targets(state.unresolved_targets, [edge.target_qualified])


def _pattern_importers_of(state: QueryGraphState) -> None:
    abs_target = (
        str((state.root / state.target).resolve()) if state.node is None else state.node.file_path
    )
    for edge in state.store.get_edges_by_target(abs_target):
        if edge.kind == "IMPORTS_FROM":
            state.results.append(
                {
                    "importer": edge.source_qualified,
                    "file": edge.file_path,
                }
            )
            state.edges_out.append(edge_to_dict(edge))


def _pattern_docs_for(state: QueryGraphState) -> None:
    qn = state.qualified_name
    for edge in state.store.get_edges_by_source(qn):
        if is_low_confidence_markdown_code_span(edge):
            continue
        role = cross_artifact_role(edge)
        if role in _ARTIFACT_TO_DOC_ROLES:
            state.results.append(
                documentation_result(
                    edge,
                    endpoint=edge.target_qualified,
                    inverse_label=_ARTIFACT_TO_DOC_ROLES[role],
                )
            )
            state.edges_out.append(edge_to_dict(edge))
    for edge in state.store.get_edges_by_target(qn):
        if is_low_confidence_markdown_code_span(edge):
            continue
        role = cross_artifact_role(edge)
        if role in _DOC_TO_ARTIFACT_ROLES:
            state.results.append(
                documentation_result(
                    edge,
                    endpoint=edge.source_qualified,
                    inverse_label=_DOC_TO_ARTIFACT_ROLES[role],
                )
            )
            state.edges_out.append(edge_to_dict(edge))


def _pattern_implementations_of(state: QueryGraphState) -> None:
    qn = state.qualified_name
    for edge in state.store.get_edges_by_source(qn):
        if is_low_confidence_markdown_code_span(edge):
            continue
        role = cross_artifact_role(edge)
        if role == "implemented_by":
            state.results.append(documentation_result(edge, endpoint=edge.target_qualified))
            state.edges_out.append(edge_to_dict(edge))
    for edge in state.store.get_edges_by_target(qn):
        if is_low_confidence_markdown_code_span(edge):
            continue
        role = cross_artifact_role(edge)
        if role == "implements_contract":
            state.results.append(
                documentation_result(
                    edge,
                    endpoint=edge.source_qualified,
                    inverse_label="implemented_by",
                )
            )
            state.edges_out.append(edge_to_dict(edge))


def _pattern_bridges_from(state: QueryGraphState) -> None:
    qn = state.qualified_name
    for edge in state.store.get_edges_by_source(qn):
        if edge.kind != "CROSS_ARTIFACT":
            continue
        if is_low_confidence_markdown_code_span(edge):
            continue
        role = cross_artifact_role(edge)
        if role not in _INFRA_TO_CODE_BRIDGE_ROLES:
            continue
        tier = str(getattr(edge, "confidence_tier", "") or "").upper()
        if not tier and isinstance(getattr(edge, "extra", None), dict):
            tier = str(edge.extra.get("confidence_tier", "")).upper()
        if tier not in {"EXACT", "HIGH", "EXTRACTED"}:
            continue
        state.results.append(
            documentation_result(
                edge,
                endpoint=edge.target_qualified,
                inverse_label=_INFRA_TO_CODE_BRIDGE_ROLES[role],
            )
        )
        state.edges_out.append(edge_to_dict(edge))


def _pattern_children_of(state: QueryGraphState) -> None:
    qn = state.qualified_name
    child_edges = [edge for edge in state.store.get_edges_by_source(qn) if edge.kind == "CONTAINS"]
    child_nodes, child_unresolved = node_dicts_for_edges(
        state.store, child_edges, qualified_attr="target_qualified"
    )
    state.results.extend(child_nodes)
    merge_unresolved_targets(state.unresolved_targets, child_unresolved)


def _pattern_tests_for(state: QueryGraphState) -> None:
    if state.node is None:
        return
    qn = state.qualified_name
    state.results.extend(dict(item) for item in infer_tests_for_node(state.store, state.node))
    test_edges = [edge for edge in state.store.get_edges_by_source(qn) if edge.kind == "TESTED_BY"]
    state.edges_out.extend(edge_to_dict(edge) for edge in test_edges)


def _pattern_inheritors_of(state: QueryGraphState) -> None:
    qn = state.qualified_name
    inherit_edges = [
        edge
        for edge in state.store.get_edges_by_target(qn)
        if edge.kind in ("INHERITS", "IMPLEMENTS")
    ]
    inheritor_nodes, inheritor_unresolved = node_dicts_for_edges(
        state.store, inherit_edges, qualified_attr="source_qualified"
    )
    state.results.extend(inheritor_nodes)
    merge_unresolved_targets(state.unresolved_targets, inheritor_unresolved)
    state.edges_out.extend(edge_to_dict(edge) for edge in inherit_edges)
    if not state.results and state.node is not None:
        fallback_edges = []
        for kind in ("INHERITS", "IMPLEMENTS"):
            fallback_edges.extend(
                filter_bare_name_fallback_edges(
                    state.store,
                    state.store.search_edges_by_target_name(state.node.name, kind=kind),
                    state.node,
                )
            )
        fallback_nodes, fallback_unresolved = node_dicts_for_edges(
            state.store,
            fallback_edges,
            qualified_attr="source_qualified",
        )
        state.results.extend(fallback_nodes)
        merge_unresolved_targets(state.unresolved_targets, fallback_unresolved)
        state.edges_out.extend(edge_to_dict(edge) for edge in fallback_edges)
        annotate_bare_name_edges(state.edges_out)


def _pattern_file_summary(state: QueryGraphState) -> None:
    file_nodes: list[Any] = []
    for abs_path in file_path_candidates(state.root, state.target):
        file_nodes = state.store.get_nodes_by_file(abs_path)
        if file_nodes:
            break
    for node in file_nodes:
        state.results.append(node_to_dict(node))


def _pattern_source_of(state: QueryGraphState) -> None:
    if state.node is None:
        return
    state.results.append(
        read_live_node_source(
            state.node,
            repo_root=state.root,
            max_chars=SOURCE_OF_MAX_CHARS,
        )
    )


_PATTERN_HANDLERS = {
    "callers_of": _pattern_callers_of,
    "callees_of": _pattern_callees_of,
    "imports_of": _pattern_imports_of,
    "importers_of": _pattern_importers_of,
    "docs_for": _pattern_docs_for,
    "implementations_of": _pattern_implementations_of,
    "bridges_from": _pattern_bridges_from,
    "children_of": _pattern_children_of,
    "tests_for": _pattern_tests_for,
    "inheritors_of": _pattern_inheritors_of,
    "file_summary": _pattern_file_summary,
    "source_of": _pattern_source_of,
}


def execute_query_pattern(state: QueryGraphState) -> None:
    handler = _PATTERN_HANDLERS[state.pattern]
    handler(state)


_QUERY_MINIMAL_FIELDS = (
    "name",
    "kind",
    "file_path",
    "qualified_name",
    "line_start",
    "line_end",
    "confidence",
    "coverage_source",
    "source",
    "target",
    "matched_endpoint",
    "relationship_role",
    "inverse_label",
    "evidence_type",
    "file",
    "truncated",
    "source_stale",
    "read_error",
    "omitted_chars",
    "omitted_lines",
    "signature",
    "span_line_start",
    "span_line_end",
)


def _source_of_missingness(item: Mapping[str, Any]) -> list[dict[str, Any]]:
    extra: list[dict[str, Any]] = []
    read_error = item.get("read_error")
    if read_error:
        extra.append(
            {
                "reason_code": "source_unreadable",
                "severity": "medium",
                "claim_effect": f"live source was not read ({read_error})",
            }
        )
    if item.get("source_stale"):
        extra.append(
            {
                "reason_code": "source_stale",
                "severity": "medium",
                "claim_effect": (
                    "worktree file_hash differs from the graph; "
                    "the stored span may not match the live body"
                ),
            }
        )
    if item.get("truncated"):
        extra.append(
            {
                "reason_code": "live_source_truncated",
                "severity": "low",
                "claim_effect": (
                    f"{item.get('omitted_chars', 0)} character(s) omitted; "
                    "Read the file for the rest"
                ),
            }
        )
    return extra


def _attach_source_of_coverage(
    payload: dict[str, Any],
    state: QueryGraphState,
    missingness: Sequence[Mapping[str, Any]],
) -> dict[str, Any]:
    if state.pattern != "source_of" or not state.results:
        payload["missingness"] = list(missingness)
        return payload
    item = state.results[0]
    extra = _source_of_missingness(item)
    payload["source_coverage"] = {
        "max_chars": item.get("max_chars"),
        "truncated": bool(item.get("truncated")),
        "source_stale": bool(item.get("source_stale")),
        "read_error": item.get("read_error"),
        "omitted_chars": item.get("omitted_chars", 0),
        "omitted_lines": item.get("omitted_lines", 0),
    }
    payload["missingness"] = [*missingness, *extra]
    if item.get("read_error") or item.get("source_stale"):
        payload["status"] = "degraded"
    return payload


def build_query_graph_response(
    state: QueryGraphState,
    *,
    detail_level: str,
    answerability: Mapping[str, Any],
    missingness: Sequence[Mapping[str, Any]],
) -> dict[str, Any]:
    summary = f"Found {len(state.results)} result(s) for {state.pattern}('{state.target}')"
    exact_count = 1 if state.resolution == "exact" and state.node is not None else 0
    resolution_payload: dict[str, Any] = {
        "resolution": state.resolution,
        "exact_match_count": exact_count,
    }
    if state.resolved_target is not None:
        resolution_payload["resolved_target"] = state.resolved_target
    if state.resolution == "fuzzy":
        resolution_payload["original_target"] = state.original_target
    zero_result_fields = query_zero_result_fields(
        results=state.results,
        unresolved_targets=state.unresolved_targets,
    )
    guidance = query_graph_guidance(
        pattern=state.pattern,
        target=state.target,
        result_count=len(state.results),
        exact_count=exact_count,
    )
    next_action = exactness_action(
        state.target,
        exact_count,
        len(state.results),
        pattern=state.pattern,
    )

    if detail_level == "minimal":
        minimal_results = [
            {k: result[k] for k in _QUERY_MINIMAL_FIELDS if k in result}
            for result in state.results[:5]
        ]
        for item in minimal_results:
            item["evidence_type"] = result_evidence_type(item)
        payload = {
            "status": "ok",
            "pattern": state.pattern,
            "target": state.target,
            "description": QUERY_PATTERNS[state.pattern],
            "summary": summary,
            "result_count": len(state.results),
            "unresolved_count": len(state.unresolved_targets),
            "unresolved_targets": state.unresolved_targets,
            **zero_result_fields,
            "next_action": next_action,
            **resolution_payload,
            "answerability": answerability,
            "results": minimal_results,
            "guidance": guidance,
            "_hints": guidance_actions_to_hints(guidance),
        }
        return _attach_source_of_coverage(payload, state, missingness)

    payload = {
        "status": "ok",
        "pattern": state.pattern,
        "target": state.target,
        "description": QUERY_PATTERNS[state.pattern],
        "summary": summary,
        "result_count": len(state.results),
        "unresolved_count": len(state.unresolved_targets),
        "unresolved_targets": state.unresolved_targets,
        **zero_result_fields,
        "next_action": next_action,
        **resolution_payload,
        "answerability": answerability,
        "results": state.results,
        "edges": state.edges_out,
        "guidance": guidance,
        "_hints": guidance_actions_to_hints(guidance),
    }
    apply_output_budget(payload, budget_tokens=8000, list_priorities=["results", "edges"])
    return _attach_source_of_coverage(payload, state, missingness)
