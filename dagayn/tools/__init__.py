"""MCP tool implementation functions for the Dagayn server."""

from __future__ import annotations

# Re-export names that external code may patch via "dagayn.tools.*"
from ..changes import parse_diff_ranges as parse_diff_ranges
from ..changes import parse_git_diff_ranges as parse_git_diff_ranges
from ..changes import parse_svn_diff_ranges as parse_svn_diff_ranges
from ..incremental import (
    get_changed_files as get_changed_files,
)
from ..incremental import (
    get_staged_and_unstaged as get_staged_and_unstaged,
)

# -- _common ----------------------------------------------------------------
from ._common import (
    _BUILTIN_CALL_NAMES,
    _get_store,
    _validate_repo_root,
)
from ._common import (
    apply_output_budget as apply_output_budget,
)
from ._common import (
    make_response as make_response,
)
from ._common import (
    projection_for_detail_level as projection_for_detail_level,
)

# -- analysis_tools ---------------------------------------------------------
from .analysis_tools import (
    get_bridge_nodes_func,
    get_hub_nodes_func,
    get_knowledge_gaps_func,
    get_suggested_questions_func,
    get_surprising_connections_func,
)

# -- architecture_analysis --------------------------------------------------
from .architecture_analysis import architecture_analysis_func

# -- architecture_tools -----------------------------------------------------
from .architecture_tools import (
    compute_sdp_metrics_func,
    detect_adp_violations_func,
    detect_sdp_violations_func,
)

# -- build ------------------------------------------------------------------
from .build import build_or_update_graph, run_postprocess

# -- community_tools --------------------------------------------------------
from .community_tools import (
    get_architecture_overview_func,
    get_community_func,
    list_communities_func,
)

# -- context ----------------------------------------------------------------
from .context import get_minimal_context

# -- docs -------------------------------------------------------------------
from .docs import embed_graph, generate_wiki_func, get_docs_section, get_wiki_page_func

# -- ensure -----------------------------------------------------------------
from .ensure import ensure_graph

# -- session_prepare --------------------------------------------------------
from .session_prepare import session_prepare

# -- sync_status ------------------------------------------------------------
from .sync_status import assess_graph_sync

# -- flow_dispatcher --------------------------------------------------------
from .flow_dispatcher import flow_func

# -- flows_tools ------------------------------------------------------------
from .flows_tools import get_flow, list_flows

# -- query ------------------------------------------------------------------
from .query import (
    find_large_functions,
    get_impact_radius,
    list_graph_stats,
    query_graph,
    semantic_search_nodes,
    traverse_graph_func,
)

# -- refactor_tools ---------------------------------------------------------
from .refactor_tools import apply_refactor_func, refactor_func

# -- registry_tools ---------------------------------------------------------
from .registry_tools import cross_repo_search_func, list_repos_func

# -- review -----------------------------------------------------------------
from .review import (
    detect_changes_func,
    get_review_context,
)

# -- review_dispatcher ------------------------------------------------------
from .review_dispatcher import review_func
from .review_flows import get_affected_flows_func

# -- sap_tools --------------------------------------------------------------
from .sap_tools import compute_sap_metrics_func, detect_sap_violations_func

__all__ = [
    # _common
    "_BUILTIN_CALL_NAMES",
    "_get_store",
    "_validate_repo_root",
    # build
    "build_or_update_graph",
    "run_postprocess",
    # ensure
    "ensure_graph",
    # session_prepare / sync_status
    "session_prepare",
    "assess_graph_sync",
    # context
    "get_minimal_context",
    # architecture_analysis
    "architecture_analysis_func",
    # community_tools
    "get_architecture_overview_func",
    "get_community_func",
    "list_communities_func",
    # docs
    "embed_graph",
    "generate_wiki_func",
    "get_docs_section",
    "get_wiki_page_func",
    # flow_dispatcher
    "flow_func",
    # flows_tools
    "get_flow",
    "list_flows",
    # query
    "find_large_functions",
    "get_impact_radius",
    "list_graph_stats",
    "query_graph",
    "semantic_search_nodes",
    "traverse_graph_func",
    # refactor_tools
    "apply_refactor_func",
    "refactor_func",
    # registry_tools
    "cross_repo_search_func",
    "list_repos_func",
    # review
    "detect_changes_func",
    "get_affected_flows_func",
    "get_review_context",
    # review_dispatcher
    "review_func",
    # analysis_tools
    "get_bridge_nodes_func",
    "get_hub_nodes_func",
    "get_knowledge_gaps_func",
    "get_suggested_questions_func",
    "get_surprising_connections_func",
    # architecture_tools
    "compute_sdp_metrics_func",
    "detect_adp_violations_func",
    "detect_sdp_violations_func",
    # sap_tools
    "compute_sap_metrics_func",
    "detect_sap_violations_func",
    # re-exported for backward compat (used in test patches)
    "get_changed_files",
    "get_staged_and_unstaged",
    "parse_git_diff_ranges",
    "parse_svn_diff_ranges",
    "parse_diff_ranges",
]
