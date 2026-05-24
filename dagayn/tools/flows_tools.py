"""Tools 10, 11: list_flows, get_flow."""

from __future__ import annotations

from pathlib import Path
from typing import Any

from ..flows import get_flow_by_id, get_flows
from ..hints import generate_hints, get_session
from ._common import (
    _get_store,
    apply_output_budget,
    graph_answerability_summary,
    missingness_from_answerability,
)

# ---------------------------------------------------------------------------
# Tool 10: list_flows  [EXPLORE]
# ---------------------------------------------------------------------------


def list_flows(
    repo_root: str | None = None,
    sort_by: str = "criticality",
    limit: int = 50,
    kind: str | None = None,
    detail_level: str = "standard",
) -> dict[str, Any]:
    """List execution flows in the codebase, sorted by criticality.

    [EXPLORE] Retrieves stored execution flows from the knowledge graph.
    Each flow represents a call chain starting from an entry point
    (e.g. HTTP handler, CLI command, test function).

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
    store, root = _get_store(repo_root)
    try:
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
                }
                for f in flows
            ]

        result: dict[str, object] = {
            "status": "ok",
            "summary": f"Found {len(flows)} execution flow(s)",
            "flows": flows,
            "flow_coverage": {
                "source": "stored_flow_extraction",
                "returned_flow_count": len(flows),
                "limit": limit,
                "kind_filter": kind,
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
            ],
        }
        result["_hints"] = generate_hints("list_flows", result, get_session())
        return result
    except Exception as exc:
        return {"status": "error", "error": str(exc)}
    finally:
        store.close()


# ---------------------------------------------------------------------------
# Tool 11: get_flow  [EXPLORE]
# ---------------------------------------------------------------------------


def get_flow(
    flow_id: int | None = None,
    flow_name: str | None = None,
    include_source: bool = False,
    repo_root: str | None = None,
) -> dict[str, Any]:
    """Get details of a single execution flow.

    [EXPLORE] Retrieves full path details for a flow, including each step's
    function name, file, and line numbers.  Optionally includes source
    snippets for every step in the path.

    Args:
        flow_id: Database ID of the flow (from list_flows).
        flow_name: Name to search for (partial match). Ignored if flow_id
                   given.
        include_source: If True, include source code snippets for each step.
        repo_root: Repository root path. Auto-detected if omitted.

    Returns:
        Flow details with steps, or not_found status.
    """
    store, root = _get_store(repo_root)
    try:
        answerability = graph_answerability_summary(store)
        flow: dict | None = None

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
            }

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

        result: dict[str, Any] = {
            "status": "ok",
            "summary": (
                f"Flow '{flow['name']}': {flow['node_count']} nodes, "
                f"depth {flow['depth']}, "
                f"criticality {flow['criticality']:.4f}"
            ),
            "flow": flow,
            "flow_coverage": {
                "source_included": bool(include_source),
                "step_count": len(flow.get("steps", [])),
                "truncated": bool(flow.get("truncated", False)),
                "coverage_guarantee": False,
            },
            "answerability": answerability,
            "missingness": [
                *missingness_from_answerability(answerability),
                {
                    "reason_code": "source_inclusion_explicit",
                    "severity": "low",
                    "claim_effect": f"source snippets included: {bool(include_source)}",
                },
            ],
        }
        if include_source:
            apply_output_budget(result["flow"], budget_tokens=8000, list_priorities=["steps"])
        result["_hints"] = generate_hints("get_flow", result, get_session())
        return result
    except Exception as exc:
        return {"status": "error", "error": str(exc)}
    finally:
        store.close()
