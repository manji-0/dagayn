"""Pattern dispatch for ``query_graph``."""

from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from ..contracts.state_types import seal_reachability_info
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
    is_external_package_target,
    is_low_confidence_markdown_code_span,
    is_unresolved_import_target,
    looks_like_query_file_target,
    merge_unresolved_targets,
    node_dicts_for_edges,
    query_graph_guidance,
    query_zero_result_fields,
    result_evidence_type,
)

_NAME_RESOLUTION_SEARCH_LIMIT = 200

#: Patterns whose relationship chains, so ``depth > 1`` has a meaning.
TRANSITIVE_PATTERNS = frozenset({"callers_of", "importers_of"})
MAX_QUERY_DEPTH = 6
#: Rows collected past hop 1 before the walk stops and reports truncation.
_TRANSITIVE_ROW_LIMIT = 500


@dataclass
class QueryGraphState:
    store: Any
    root: Path
    pattern: str
    original_target: str
    target: str
    depth: int = 1
    node: Any | None = None
    resolution: str = "exact"
    resolved_target: str | None = None
    results: list[dict[str, Any]] = field(default_factory=list)
    edges_out: list[dict[str, Any]] = field(default_factory=list)
    unresolved_targets: list[str] = field(default_factory=list)
    reachability: dict[str, Any] | None = None

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
    if (
        not node
        and state.pattern == "callers_of"
        and is_external_package_target(state.store, state.target)
    ):
        # `callers_of("react::useState")`: the callers of an external
        # package symbol, which has edges but no node.
        state.resolution = "external_package"
        return None
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
        # FTS ranking does not put exact name matches first, so a bare name
        # shared with many similarly named helpers would never reach the top 5.
        search_hits = state.store.search_nodes(state.target, limit=_NAME_RESOLUTION_SEARCH_LIMIT)
        exact_name_hits = [hit for hit in search_hits if hit.name == state.target]
        candidates = exact_name_hits[:5] if exact_name_hits else search_hits[:5]
        if len(exact_name_hits) == 1:
            node = exact_name_hits[0]
            state.resolved_target = node.qualified_name
            state.target = state.resolved_target
            state.resolution = "exact_name"
        elif len(candidates) == 1:
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
    state.results.extend(_caller_rows(state, call_edges))
    state.edges_out.extend(edge_to_dict(edge) for edge in call_edges)
    if not state.results and state.node is not None:
        fallback_edges = filter_bare_name_fallback_edges(
            state.store,
            state.store.search_edges_by_target_name(state.node.name),
            state.node,
        )
        state.results.extend(_caller_rows(state, fallback_edges))
        state.edges_out.extend(edge_to_dict(edge) for edge in fallback_edges)
        annotate_bare_name_edges(state.edges_out)
    if state.depth > 1:
        _expand_transitive(
            state,
            [row["qualified_name"] for row in state.results],
            edge_kind="CALLS",
            next_key=lambda edge: edge.source_qualified,
            rows_for_edges=_caller_rows,
        )


def _edge_rows(
    state: QueryGraphState, edges: list[Any], *, qualified_attr: str
) -> list[dict[str, Any]]:
    """Node rows for edge endpoints, each carrying the edge's line and tier."""
    if not edges:
        return []
    nodes_by_qn = state.store.get_nodes_by_qualified_names(
        [getattr(edge, qualified_attr) for edge in edges]
    )
    rows: list[dict[str, Any]] = []
    unresolved: list[str] = []
    for edge in edges:
        node = nodes_by_qn.get(getattr(edge, qualified_attr))
        if node is None:
            unresolved.append(getattr(edge, qualified_attr))
            continue
        row = node_to_dict(node)
        row["line"] = edge.line
        if "::" in str(edge.target_qualified):
            row["confidence_tier"] = edge.confidence_tier
        else:
            row["confidence_tier"] = "MEDIUM"
            row["match"] = "bare_name"
        rows.append(row)
    merge_unresolved_targets(state.unresolved_targets, unresolved)
    return rows


def _caller_rows(state: QueryGraphState, edges: list[Any]) -> list[dict[str, Any]]:
    return _edge_rows(state, edges, qualified_attr="source_qualified")


def _importer_rows(state: QueryGraphState, edges: list[Any]) -> list[dict[str, Any]]:
    return [
        {
            "importer": edge.source_qualified,
            "file": edge.file_path,
            "line": edge.line,
            "confidence_tier": edge.confidence_tier,
        }
        for edge in edges
    ]


def _expand_transitive(
    state: QueryGraphState,
    first_hop: list[str],
    *,
    edge_kind: str,
    next_key: Callable[[Any], str],
    rows_for_edges: Callable[[QueryGraphState, list[Any]], list[dict[str, Any]]],
) -> None:
    """Extend hop-1 rows breadth-first up to ``state.depth`` hops.

    Rows past hop 1 carry ``depth`` and ``via`` (the node they reach the
    previous hop through); each node appears once, at its shortest hop. Later
    hops follow resolved edges only, without hop 1's bare-name fallback.
    """
    for row in state.results:
        row["depth"] = 1
    seen = {state.qualified_name, *first_hop}
    frontier = list(dict.fromkeys(first_hop))
    hop = 2
    added = 0
    truncated = False
    while frontier and hop <= state.depth and not truncated:
        _, incoming = state.store.get_edges_by_endpoints(frontier)
        layer: list[Any] = []
        for via in frontier:
            for edge in incoming.get(via, []):
                key = next_key(edge)
                if edge.kind != edge_kind or key in seen:
                    continue
                seen.add(key)
                layer.append(edge)
        if added + len(layer) > _TRANSITIVE_ROW_LIMIT:
            layer = layer[: _TRANSITIVE_ROW_LIMIT - added]
            truncated = True
        rows = rows_for_edges(state, layer)
        vias = {next_key(edge): edge.target_qualified for edge in layer}
        for row in rows:
            row["depth"] = hop
            row["via"] = vias.get(row.get("qualified_name") or row.get("file", ""), "")
        state.results.extend(rows)
        state.edges_out.extend(edge_to_dict(edge) for edge in layer)
        added += len(layer)
        frontier = [next_key(edge) for edge in layer]
        hop += 1
    state.reachability = seal_reachability_info(
        {
            "state": "truncated" if truncated else "complete",
            "truncated": truncated,
            "max_depth": state.depth,
            "nodes_visited": len(seen) - 1,
            "depth_limit_reached": bool(frontier) and not truncated,
        }
    )


def _pattern_callees_of(state: QueryGraphState) -> None:
    qn = state.qualified_name
    call_edges = [edge for edge in state.store.get_edges_by_source(qn) if edge.kind == "CALLS"]
    state.results.extend(_edge_rows(state, call_edges, qualified_attr="target_qualified"))
    state.edges_out.extend(edge_to_dict(edge) for edge in call_edges)


def _pattern_imports_of(state: QueryGraphState) -> None:
    qn = state.qualified_name
    for edge in state.store.get_edges_by_source(qn):
        if edge.kind == "IMPORTS_FROM":
            state.results.append(
                {
                    "import_target": edge.target_qualified,
                    "line": edge.line,
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
            state.results.extend(_importer_rows(state, [edge]))
            state.edges_out.append(edge_to_dict(edge))
    if state.depth > 1:
        _expand_transitive(
            state,
            [row["file"] for row in state.results],
            edge_kind="IMPORTS_FROM",
            next_key=lambda edge: edge.file_path,
            rows_for_edges=_importer_rows,
        )


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
    state.results.extend(_edge_rows(state, inherit_edges, qualified_attr="source_qualified"))
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
        state.results.extend(_edge_rows(state, fallback_edges, qualified_attr="source_qualified"))
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
    "importer",
    "import_target",
    "unresolved",
    "lines",
    "line",
    "match",
    "confidence_tier",
    "depth",
    "via",
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
    if state.depth > 1:
        summary += f" within {state.depth} hops"
    transitive_payload: dict[str, Any] = (
        {"depth": state.depth, "reachability": state.reachability}
        if state.reachability is not None
        else {}
    )
    exact_count = 1 if state.resolution in {"exact", "exact_name"} and state.node is not None else 0
    resolution_payload: dict[str, Any] = {
        "resolution": state.resolution,
        "exact_match_count": exact_count,
    }
    if state.resolved_target is not None:
        resolution_payload["resolved_target"] = state.resolved_target
    if state.resolution in {"fuzzy", "exact_name"}:
        resolution_payload["original_target"] = state.original_target
    zero_result_fields = query_zero_result_fields(
        results=state.results,
        unresolved_targets=state.unresolved_targets,
        edges=state.edges_out,
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

    common = {
        "status": "ok",
        "pattern": state.pattern,
        "target": state.target,
        "unresolved_count": len(state.unresolved_targets),
        "unresolved_targets": state.unresolved_targets,
        **zero_result_fields,
        "next_action": next_action,
        **resolution_payload,
        **transitive_payload,
    }

    if detail_level == "full":
        payload = {
            **common,
            "description": QUERY_PATTERNS[state.pattern],
            "summary": summary,
            "result_count": len(state.results),
            "answerability": answerability,
            "results": state.results,
            "edges": state.edges_out,
            "guidance": guidance,
            "_hints": guidance_actions_to_hints(guidance),
        }
        return _finish_payload(payload, state, missingness, budget_tokens=8000)

    minimal = detail_level == "minimal"
    rows = [_compact_row(result, with_evidence=minimal) for result in state.results]
    if state.pattern in _MERGED_ROW_PATTERNS:
        rows = _merge_rows(rows)
    if minimal:
        rows = [{k: row[k] for k in _QUERY_MINIMAL_FIELDS if k in row} for row in rows]
    payload = {
        **common,
        "summary": summary.replace(f"Found {len(state.results)} ", f"Found {len(rows)} ", 1),
        "result_count": len(rows),
        "answerability": {
            key: answerability[key] for key in _COMPACT_ANSWERABILITY if key in answerability
        },
        "results": rows,
    }
    if not minimal:
        payload["guidance"] = guidance
    return _finish_payload(payload, state, missingness, budget_tokens=4000 if minimal else 8000)


def _finish_payload(
    payload: dict[str, Any],
    state: QueryGraphState,
    missingness: Sequence[Mapping[str, Any]],
    *,
    budget_tokens: int,
) -> dict[str, Any]:
    apply_output_budget(payload, budget_tokens=budget_tokens, list_priorities=["results", "edges"])
    truncation = payload.get("_truncation")
    payload["results_complete"] = not (isinstance(truncation, dict) and "results" in truncation)
    if state.reachability is not None:
        payload["next_action"] = _transitive_next_action(
            state.reachability, results_complete=payload["results_complete"]
        )
    return _attach_source_of_coverage(payload, state, missingness)


def _transitive_next_action(
    reachability: Mapping[str, Any], *, results_complete: bool
) -> dict[str, Any]:
    if reachability.get("truncated") or not results_complete:
        return {
            "tool": "query_graph_tool",
            "suggestion": (
                "the reachable set was cut off; lower depth or query the deepest "
                "listed nodes to see the rest"
            ),
        }
    if reachability.get("depth_limit_reached"):
        return {
            "tool": "query_graph_tool",
            "suggestion": (
                f"nodes beyond {reachability.get('max_depth')} hops may exist; "
                "raise depth (max 6) or query the deepest listed nodes"
            ),
        }
    return {
        "tool": None,
        "suggestion": (
            "the transitive set is closed over graph edges: no other node is reachable, "
            "so querying listed nodes again returns nothing new"
        ),
    }


_COMPACT_ANSWERABILITY = ("status", "score", "reason_codes")
#: Patterns whose rows are one per edge; compact levels merge rows for the
#: same endpoint and list the edge lines.
_MERGED_ROW_PATTERNS = frozenset({"callers_of", "callees_of", "inheritors_of", "importers_of"})
_COMPACT_DROPPED_FIELDS = frozenset({"id", "language"})
_COMPACT_DEFAULTS: dict[str, Any] = {"parent_name": None, "is_test": False}


def _compact_row(row: Mapping[str, Any], *, with_evidence: bool) -> dict[str, Any]:
    """Drop fields an agent can read off ``qualified_name`` or that hold defaults."""
    out = {
        key: value
        for key, value in row.items()
        if key not in _COMPACT_DROPPED_FIELDS
        and not (key in _COMPACT_DEFAULTS and value == _COMPACT_DEFAULTS[key])
    }
    if with_evidence:
        out["evidence_type"] = result_evidence_type(dict(row))
    file_path = out.get("file_path")
    if file_path and str(out.get("qualified_name") or "").startswith(str(file_path)):
        del out["file_path"]
    if "importer" in out and out["importer"] == out.get("file"):
        del out["importer"]
    return out


def _merge_rows(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    merged: dict[str, dict[str, Any]] = {}
    for row in rows:
        key = str(row.get("qualified_name") or f"{row.get('importer', '')}|{row.get('file', '')}")
        line = row.get("line")
        existing = merged.get(key)
        if existing is None:
            first = {k: v for k, v in row.items() if k != "line"}
            first["lines"] = [line] if line is not None else []
            merged[key] = first
        elif line is not None and line not in existing["lines"]:
            existing["lines"].append(line)
    return list(merged.values())
