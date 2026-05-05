"""Named MCP tool profiles for the dagayn server."""

from __future__ import annotations

from collections.abc import Mapping

DEFAULT_TOOL_PROFILE = "default"
FULL_TOOL_PROFILE = "full"
TOOL_PROFILE_ENV_VARS = ("DAGAYN_TOOL_PROFILE", "CRG_TOOL_PROFILE")

GRAPH_LIFECYCLE_TOOLS = frozenset(
    {
        "build_or_update_graph_tool",
        "run_postprocess_tool",
        "list_graph_stats_tool",
        "get_docs_section_tool",
    }
)

DEFAULT_WORKFLOW_TOOLS = frozenset(
    {
        "get_minimal_context_tool",
        "detect_changes_tool",
        "get_review_context_tool",
        "get_architecture_overview_tool",
        "refactor_tool",
        "query_graph_tool",
        "semantic_search_nodes_tool",
    }
)

DEFAULT_TOOL_PROFILE_TOOLS = GRAPH_LIFECYCLE_TOOLS | DEFAULT_WORKFLOW_TOOLS

REVIEW_TOOL_PROFILE_TOOLS = DEFAULT_TOOL_PROFILE_TOOLS | frozenset(
    {
        "get_impact_radius_tool",
        "get_affected_flows_tool",
        "list_flows_tool",
        "get_flow_tool",
        "get_suggested_questions_tool",
    }
)

ARCHITECTURE_TOOL_PROFILE_TOOLS = DEFAULT_TOOL_PROFILE_TOOLS | frozenset(
    {
        "list_flows_tool",
        "get_flow_tool",
        "list_communities_tool",
        "get_community_tool",
        "get_hub_nodes_tool",
        "get_bridge_nodes_tool",
        "get_knowledge_gaps_tool",
        "get_surprising_connections_tool",
        "detect_adp_violations_tool",
        "compute_sdp_metrics_tool",
        "detect_sdp_violations_tool",
        "compute_sap_metrics_tool",
        "detect_sap_violations_tool",
    }
)

REFACTOR_TOOL_PROFILE_TOOLS = DEFAULT_TOOL_PROFILE_TOOLS | frozenset(
    {
        "find_large_functions_tool",
        "get_impact_radius_tool",
        "apply_refactor_tool",
    }
)

TOOL_PROFILES: Mapping[str, frozenset[str] | None] = {
    DEFAULT_TOOL_PROFILE: DEFAULT_TOOL_PROFILE_TOOLS,
    "review": REVIEW_TOOL_PROFILE_TOOLS,
    "architecture": ARCHITECTURE_TOOL_PROFILE_TOOLS,
    "refactor": REFACTOR_TOOL_PROFILE_TOOLS,
    FULL_TOOL_PROFILE: None,
}

TOOL_PROFILE_NAMES = tuple(TOOL_PROFILES)
