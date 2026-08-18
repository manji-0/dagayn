"""Tools 10, 11: list_flows, get_flow."""

from __future__ import annotations

import logging
from pathlib import Path

from ..flows import FlowRecord, get_flow_by_id, get_flows
from ..hints import generate_hints, get_session
from ._common import (
    ToolPayload,
    _get_store,
    apply_output_budget,
    graph_answerability_summary,
    guidance_actions_to_hints,
    handle_tool_runtime_error,
    make_guidance_item,
    missingness_from_answerability,
)

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Tool 10: list_flows  [EXPLORE]
# ---------------------------------------------------------------------------


def list_flows(
    repo_root: str | None = None,
    sort_by: str = "criticality",
    limit: int = 50,
    kind: str | None = None,
    detail_level: str = "standard",
) -> ToolPayload:
    """List reachable-set flows in the codebase, sorted by criticality.

    [EXPLORE] Retrieves stored flows from the knowledge graph. Each flow is the
    CALLS reachable set from an entry point (e.g. HTTP handler, CLI command),
    not an ordered execution path. ``path`` / ``steps`` are BFS visit order.
    Truncation is disclosed via ``truncated`` / ``truncation_reason``.

    Args:
        repo_root: Repository root path. Auto-detected if omitted.
        sort_by: Sort column: criticality, depth, node_count, file_count,
                 or name.
        limit: Maximum flows to return (default: 50).
        kind: Optional filter by entry point kind (e.g. "Test", "Function").
        detail_level: "standard" (default) returns full flow data;
                      "minimal" returns only name, criticality, and
                      node_count per flow.

    Returns:
        List of flows with criticality scores.
    """
    store = None
    try:
        store, root = _get_store(repo_root)
        answerability = graph_answerability_summary(store)
        fetch_limit = limit if not kind else limit * 10  # fetch more when filtering
        flows = get_flows(store, sort_by=sort_by, limit=fetch_limit)

        if kind:
            entry_ids = [f["entry_point_id"] for f in flows if f.get("entry_point_id") is not None]
            entry_nodes = store.get_nodes_by_ids(entry_ids)
            filtered = []
            for f in flows:
                ep_id = f.get("entry_point_id")
                if ep_id is not None:
                    node = entry_nodes.get(ep_id)
                    if node is not None and node.kind == kind:
                        filtered.append(f)
            flows = filtered[:limit]

        if detail_level == "minimal":
            flows = [
                {
                    "name": f["name"],
                    "criticality": f["criticality"],
                    "node_count": f["node_count"],
                    "kind": f.get("kind") or "reachable_set",
                    "truncated": bool(f.get("truncated")),
                }
                for f in flows
            ]

        truncated_count = sum(1 for f in flows if f.get("truncated"))
        result: dict[str, object] = {
            "status": "ok",
            "summary": (
                f"Found {len(flows)} reachable-set flow(s)"
                + (f" ({truncated_count} truncated)" if truncated_count else "")
            ),
            "flows": flows,
            "flow_coverage": {
                "source": "stored_flow_extraction",
                "returned_flow_count": len(flows),
                "limit": limit,
                "kind_filter": kind,
                "truncated_count": truncated_count,
                "coverage_guarantee": False,
            },
            "answerability": answerability,
            "missingness": [
                *missingness_from_answerability(answerability),
                {
                    "reason_code": "flow_criticality_is_ranking_signal",
                    "severity": "low",
                    "claim_effect": "flow ranking is not a coverage guarantee",
                },
                *(
                    [
                        {
                            "reason_code": "truncated_flow",
                            "severity": "medium",
                            "claim_effect": (
                                "one or more reachable sets were capped; "
                                "omitted callees are not absent from the program"
                            ),
                        }
                    ]
                    if truncated_count
                    else []
                ),
            ],
        }
        flow_guidance = [
            make_guidance_item(
                claim=(
                    f"Returned {len(flows)} ranked reachable-set flow(s) "
                    "from stored flow extraction."
                    if flows
                    else "No flows matched the current filters."
                ),
                evidence={
                    "type": "computed",
                    "returned_flow_count": len(flows),
                    "limit": limit,
                    "sort_by": sort_by,
                    "kind_filter": kind,
                },
                confidence="medium" if flows else "low",
                missingness=[
                    {
                        "reason_code": "flow_criticality_is_ranking_signal",
                        "severity": "low",
                        "claim_effect": "flow ranking is not a coverage guarantee",
                    }
                ],
                action=(
                    'flow_tool mode="get" -- inspect a specific reachable set'
                    if flows
                    else 'flow_tool mode="list" -- broaden kind filter or increase limit'
                ),
                reason_codes=["stored_flow_extraction"],
                counts={"returned_flow_count": len(flows)},
            )
        ]
        result["guidance"] = flow_guidance
        hints = guidance_actions_to_hints(flow_guidance)
        result["_hints"] = (
            hints if hints["next_steps"] else generate_hints("list_flows", result, get_session())
        )
        return result
    except Exception as exc:
        return handle_tool_runtime_error(exc, logger=logger, context="list_flows")
    finally:
        if store is not None:
            store.close()


# ---------------------------------------------------------------------------
# Tool 11: get_flow  [EXPLORE]
# ---------------------------------------------------------------------------


def get_flow(
    flow_id: int | None = None,
    flow_name: str | None = None,
    include_source: bool = False,
    repo_root: str | None = None,
) -> ToolPayload:
    """Get details of a single reachable-set flow.

    [EXPLORE] Retrieves membership details for a flow, including each member's
    function name, file, and line numbers in BFS visit order. That list is not
    a call sequence. Optionally includes source snippets for every member.
    Truncation is disclosed on the flow as ``truncated`` / ``truncation_reason``.

    Args:
        flow_id: Database ID of the flow (from list_flows).
        flow_name: Name to search for (partial match). Ignored if flow_id
                   given.
        include_source: If True, include source code snippets for each step.
        repo_root: Repository root path. Auto-detected if omitted.

    Returns:
        Flow details with steps, or not_found status.
    """
    store = None
    try:
        store, root = _get_store(repo_root)
        answerability = graph_answerability_summary(store)
        flow: FlowRecord | None = None

        if flow_id is not None:
            flow = get_flow_by_id(store, flow_id)
        elif flow_name is not None:
            # Search flows by name match
            all_flows = get_flows(store, sort_by="criticality", limit=500)
            for f in all_flows:
                if flow_name.lower() in f["name"].lower():
                    flow = get_flow_by_id(store, f["id"])
                    break

        if flow is None:
            return {
                "status": "not_found",
                "summary": "No flow found matching the given criteria.",
                "answerability": answerability,
                "missingness": [
                    *missingness_from_answerability(answerability),
                    {
                        "reason_code": "flow_not_found_in_current_graph",
                        "severity": "medium",
                        "claim_effect": "absence is graph-limited, not proof the flow cannot exist",
                    },
                ],
            }

        resolved_step_count = int(flow.get("resolved_step_count") or len(flow.get("steps", [])))
        missing_step_count = int(flow.get("missing_step_count") or 0)
        stored_node_count = int(flow.get("node_count") or resolved_step_count)
        is_stale_flow = missing_step_count > 0
        is_truncated = bool(flow.get("truncated"))
        truncation_reason = flow.get("truncation_reason")

        _source_max_chars = 2000
        # Optionally include source snippets for each step
        if include_source and "steps" in flow:
            for step in flow["steps"]:
                fp = Path(step["file"]) if step.get("file") else None
                if fp is not None and not fp.is_absolute():
                    fp = root / fp
                file_path = fp
                if file_path and file_path.is_file():
                    try:
                        lines = file_path.read_text(errors="replace").splitlines()
                        start = max(0, (step.get("line_start") or 1) - 1)
                        end = min(
                            len(lines),
                            step.get("line_end") or len(lines),
                        )
                        src = "\n".join(f"{i + 1}: {lines[i]}" for i in range(start, end))
                        if len(src) > _source_max_chars:
                            src = src[:_source_max_chars] + "\n... (truncated)"
                        step["source"] = src
                    except (OSError, UnicodeDecodeError):
                        step["source"] = "(could not read file)"

        if is_stale_flow:
            summary = (
                f"Flow '{flow['name']}': {resolved_step_count}/{stored_node_count} members "
                f"resolved ({missing_step_count} missing), depth {flow['depth']}, "
                f"criticality {flow['criticality']:.4f}"
            )
        else:
            summary = (
                f"Flow '{flow['name']}' (reachable_set): {stored_node_count} members, "
                f"depth {flow['depth']}, "
                f"criticality {flow['criticality']:.4f}"
            )
        if is_truncated:
            summary += f" [truncated:{truncation_reason or 'unspecified'}]"

        stale_missingness = (
            [
                {
                    "reason_code": "stale_flow",
                    "severity": "medium",
                    "claim_effect": (
                        f"{missing_step_count} stored member(s) no longer "
                        "resolve to live graph nodes"
                    ),
                }
            ]
            if is_stale_flow
            else []
        )
        truncated_missingness = (
            [
                {
                    "reason_code": "truncated_flow",
                    "severity": "medium",
                    "claim_effect": (
                        "reachable set was capped"
                        + (f" ({truncation_reason})" if truncation_reason else "")
                        + "; omitted callees are not absent from the program"
                    ),
                }
            ]
            if is_truncated
            else []
        )

        result: ToolPayload = {
            "status": "degraded" if (is_stale_flow or is_truncated) else "ok",
            "summary": summary,
            "flow": flow,
            "flow_coverage": {
                "source_included": include_source,
                "step_count": resolved_step_count,
                "stored_node_count": stored_node_count,
                "resolved_step_count": resolved_step_count,
                "missing_step_count": missing_step_count,
                "truncated": is_truncated,
                "truncation_reason": truncation_reason,
                "coverage_guarantee": False,
            },
            "answerability": answerability,
            "missingness": [
                *missingness_from_answerability(answerability),
                *stale_missingness,
                *truncated_missingness,
                {
                    "reason_code": "source_inclusion_explicit",
                    "severity": "low",
                    "claim_effect": f"source snippets included: {include_source}",
                },
            ],
        }
        flow_guidance = [
            make_guidance_item(
                claim=(
                    (
                        f"Flow '{flow['name']}' resolves {resolved_step_count} of "
                        f"{stored_node_count} stored step(s); {missing_step_count} step(s) "
                        f"are missing from the live graph."
                    )
                    if is_stale_flow
                    else (
                        f"Flow '{flow['name']}' has {stored_node_count} step(s) "
                        f"with criticality {flow['criticality']:.4f}"
                        + (
                            f", including {flow.get('bridge_step_count', 0)} bridge step(s)."
                            if int(flow.get("bridge_step_count") or 0)
                            else "."
                        )
                    )
                ),
                evidence={
                    "type": "computed",
                    "flow_id": flow.get("id"),
                    "name": flow.get("name"),
                    "node_count": stored_node_count,
                    "resolved_step_count": resolved_step_count,
                    "missing_step_count": missing_step_count,
                    "depth": flow.get("depth"),
                    "criticality": flow.get("criticality"),
                    "bridge_step_count": flow.get("bridge_step_count", 0),
                    "source_included": include_source,
                },
                confidence="medium" if not is_stale_flow else "low",
                missingness=[
                    *stale_missingness,
                    *truncated_missingness,
                    {
                        "reason_code": "flow_path_is_stored_extraction",
                        "severity": "low",
                        "claim_effect": (
                            "flow members are a BFS reachable set, not a runtime call sequence"
                        ),
                    },
                    *(
                        [
                            {
                                "reason_code": "cross_artifact_bridge_is_static_evidence",
                                "severity": "low",
                                "claim_effect": (
                                    "bridge steps mark CROSS_ARTIFACT transitions distinctly"
                                ),
                            }
                        ]
                        if int(flow.get("bridge_step_count") or 0)
                        else []
                    ),
                ],
                action=(
                    "dagayn build --local-embedding none -- refresh stored flows; "
                    'review_tool mode="impact" -- check blast radius along resolved steps'
                    if is_stale_flow
                    else (
                        'review_tool mode="impact" -- check blast radius along this flow; '
                        'query_graph_tool pattern="docs_for" -- follow bridge docs when present'
                    )
                ),
                reason_codes=["stored_flow_extraction", "reachable_set"]
                + (["stale_flow"] if is_stale_flow else [])
                + (["truncated_flow"] if is_truncated else [])
                + (
                    ["cross_artifact_bridge_step"]
                    if int(flow.get("bridge_step_count") or 0)
                    else []
                ),
            )
        ]
        result["guidance"] = flow_guidance
        if include_source:
            apply_output_budget(result["flow"], budget_tokens=8000, list_priorities=["steps"])
        hints = guidance_actions_to_hints(flow_guidance)
        result["_hints"] = (
            hints if hints["next_steps"] else generate_hints("get_flow", result, get_session())
        )
        return result
    except Exception as exc:
        return handle_tool_runtime_error(exc, logger=logger, context="get_flow")
    finally:
        if store is not None:
            store.close()
