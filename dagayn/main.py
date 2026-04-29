"""MCP server entry point for Dagayn.

Run as: dagayn serve
Communicates via stdio (standard MCP transport), or use
``dagayn serve --http`` for Streamable HTTP on localhost (port 5555
by default).
"""

from __future__ import annotations

import asyncio
import sys
from typing import Any, Literal, Optional

from fastmcp import FastMCP

from .prompts import (
    architecture_map_prompt,
    debug_issue_prompt,
    onboard_developer_prompt,
    pre_merge_check_prompt,
    review_changes_prompt,
)
from .tools import (
    apply_refactor_func,
    build_or_update_graph,
    compute_sap_metrics_func,
    compute_sdp_metrics_func,
    cross_repo_search_func,
    detect_adp_violations_func,
    detect_changes_func,
    detect_sap_violations_func,
    detect_sdp_violations_func,
    embed_graph,
    find_large_functions,
    generate_wiki_func,
    get_affected_flows_func,
    get_architecture_overview_func,
    get_bridge_nodes_func,
    get_community_func,
    get_docs_section,
    get_flow,
    get_hub_nodes_func,
    get_impact_radius,
    get_knowledge_gaps_func,
    get_minimal_context,
    get_review_context,
    get_suggested_questions_func,
    get_surprising_connections_func,
    get_wiki_page_func,
    list_communities_func,
    list_flows,
    list_graph_stats,
    list_repos_func,
    query_graph,
    refactor_func,
    run_postprocess,
    semantic_search_nodes,
    traverse_graph_func,
)

# NOTE: Thread-safe for stdio MCP (single-threaded). If adding HTTP/SSE
# transport with concurrent requests, replace with contextvars.ContextVar.
_default_repo_root: str | None = None


def _resolve_repo_root(repo_root: Optional[str]) -> Optional[str]:
    """Resolve repo_root for a tool call.

    Order of precedence:
    1. Explicit ``repo_root`` passed by the MCP client (highest).
    2. ``--repo`` CLI flag passed to ``dagayn serve``
       (captured in ``_default_repo_root``).
    3. None — the underlying impl will fall back to the server's cwd.

    Previously, only ``get_docs_section_tool`` consulted ``_default_repo_root``,
    so ``serve --repo <X>`` had no effect for the other 21 tools. See: #222
    follow-up.
    """
    return repo_root if repo_root else _default_repo_root


mcp = FastMCP(
    "dagayn",
    instructions=(
        "Persistent incremental knowledge graph for token-efficient, "
        "context-aware code reviews. Parses your codebase with Tree-sitter, "
        "builds a structural graph, and provides smart impact analysis."
    ),
)


@mcp.tool()
async def build_or_update_graph_tool(
    full_rebuild: bool = False,
    repo_root: Optional[str] = None,
    base: str = "HEAD~1",
    postprocess: str = "full",
    recurse_submodules: Optional[bool] = None,
) -> dict:
    """Build or incrementally update the code knowledge graph.

    Call this first to initialize the graph, or after making changes.
    By default performs an incremental update (only changed files).
    Set full_rebuild=True to re-parse every file.

    Runs the blocking full_build / incremental_update work in a thread
    via ``asyncio.to_thread`` so the stdio event loop stays responsive.
    Without this wrapper, long builds deadlocked on Windows because
    ``ProcessPoolExecutor`` (used by parallel parsing) interacted badly
    with the sync handler blocking the only event-loop thread. See:
    #46, #136.

    Args:
        full_rebuild: If True, re-parse all files. Default: False (incremental).
        repo_root: Repository root path. Auto-detected from current directory if omitted.
        base: Git ref to diff against for incremental updates. Default: HEAD~1.
        postprocess: Post-processing level: "full" (default), "minimal" (signatures+FTS only),
                     or "none" (skip all post-processing). Use "minimal" for faster builds.
        recurse_submodules: If True, include files from git submodules.
            When None (default), falls back to CRG_RECURSE_SUBMODULES env var.
    """
    return await asyncio.to_thread(
        build_or_update_graph,
        full_rebuild=full_rebuild,
        repo_root=_resolve_repo_root(repo_root),
        base=base,
        postprocess=postprocess,
        recurse_submodules=recurse_submodules,
    )


@mcp.tool()
async def run_postprocess_tool(
    flows: bool = True,
    communities: bool = True,
    fts: bool = True,
    repo_root: Optional[str] = None,
) -> dict:
    """Run post-processing on existing graph (flows, communities, FTS index).

    Use after building with postprocess="none" or "minimal", or to re-run
    expensive steps independently. Signatures are always computed.

    Offloaded to a thread via ``asyncio.to_thread`` so community
    detection on large graphs doesn't block the MCP event loop. See:
    #46, #136.

    Args:
        flows: Run flow detection. Default: True.
        communities: Run community detection. Default: True.
        fts: Rebuild FTS index. Default: True.
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return await asyncio.to_thread(
        run_postprocess,
        flows=flows,
        communities=communities,
        fts=fts,
        repo_root=_resolve_repo_root(repo_root),
    )


@mcp.tool()
def get_minimal_context_tool(
    task: str = "",
    changed_files: Optional[list[str]] = None,
    repo_root: Optional[str] = None,
    base: str = "HEAD~1",
) -> dict:
    """Get ultra-compact context for any task (~100 tokens). Always call this first.

    Returns graph stats, risk score, top communities/flows, and suggested
    next tools in a single compact response. Use this as the entry point
    before any other graph tool to minimize token usage.

    Args:
        task: What you are doing (e.g. "review PR #42", "debug login timeout").
        changed_files: Explicit list of changed files. Auto-detected if omitted.
        repo_root: Repository root path. Auto-detected if omitted.
        base: Git ref for diff comparison. Default: HEAD~1.
    """
    return get_minimal_context(
        task=task,
        changed_files=changed_files,
        repo_root=_resolve_repo_root(repo_root),
        base=base,
    )


@mcp.tool()
def get_impact_radius_tool(
    changed_files: Optional[list[str]] = None,
    max_depth: int = 2,
    max_nodes: int = 50,
    repo_root: Optional[str] = None,
    base: str = "HEAD~1",
    detail_level: str = "standard",
) -> dict:
    """Analyze the blast radius of changed files in the codebase.

    Shows which functions, classes, and files are impacted by changes.
    Auto-detects changed files from git if not specified.

    Args:
        changed_files: List of changed file paths (relative to repo root). Auto-detected if omitted.
        max_depth: Number of hops to traverse in the dependency graph. Default: 2.
        max_nodes: Maximum impacted nodes to return in standard mode. Default: 50.
            When the result is truncated, truncated=True and total_impacted show the full count.
        repo_root: Repository root path. Auto-detected if omitted.
        base: Git ref for auto-detecting changes. Default: HEAD~1.
        detail_level: "standard" for full output, "minimal" for compact summary. Default: standard.
    """
    return get_impact_radius(
        changed_files=changed_files,
        max_depth=max_depth,
        max_results=max_nodes,
        repo_root=_resolve_repo_root(repo_root),
        base=base,
        detail_level=detail_level,
    )


@mcp.tool()
def query_graph_tool(
    pattern: str,
    target: str,
    repo_root: Optional[str] = None,
    detail_level: str = "standard",
) -> dict:
    """Run a predefined graph query to explore code relationships.

    Available patterns:
    - callers_of: Find functions that call the target
    - callees_of: Find functions called by the target
    - imports_of: Find what the target imports
    - importers_of: Find files that import the target
    - children_of: Find nodes contained in a file or class
    - tests_for: Find tests for the target
    - inheritors_of: Find classes inheriting from the target
    - file_summary: Get all nodes in a file

    Args:
        pattern: Query pattern name (see above).
        target: Node name, qualified name, or file path to query.
        repo_root: Repository root path. Auto-detected if omitted.
        detail_level: "standard" for full output, "minimal" for compact summary. Default: standard.
    """
    return query_graph(
        pattern=pattern,
        target=target,
        repo_root=_resolve_repo_root(repo_root),
        detail_level=detail_level,
    )


@mcp.tool()
def get_review_context_tool(
    changed_files: Optional[list[str]] = None,
    max_depth: int = 2,
    include_source: bool = True,
    max_lines_per_file: int = 200,
    repo_root: Optional[str] = None,
    base: str = "HEAD~1",
    detail_level: str = "standard",
) -> dict:
    """Generate a focused, token-efficient review context for code changes.

    Combines impact analysis with source snippets and review guidance.
    Use this for comprehensive code reviews.

    Args:
        changed_files: Files to review. Auto-detected from git diff if omitted.
        max_depth: Impact radius depth. Default: 2.
        include_source: Include source code snippets. Default: True.
        max_lines_per_file: Max source lines per file. Default: 200.
        repo_root: Repository root path. Auto-detected if omitted.
        base: Git ref for change detection. Default: HEAD~1.
        detail_level: "standard" for full output, "minimal" for
            token-efficient summary. Default: standard.
    """
    return get_review_context(
        changed_files=changed_files,
        max_depth=max_depth,
        include_source=include_source,
        max_lines_per_file=max_lines_per_file,
        repo_root=_resolve_repo_root(repo_root),
        base=base,
        detail_level=detail_level,
    )


@mcp.tool()
def semantic_search_nodes_tool(
    query: str,
    kind: Optional[str] = None,
    limit: int = 20,
    repo_root: Optional[str] = None,
    model: Optional[str] = None,
    provider: Optional[str] = None,
    detail_level: str = "standard",
) -> dict:
    """Search for code entities by name, keyword, or semantic similarity.

    Uses vector embeddings for semantic search when available (run embed_graph_tool
    first, with a provider of your choice: "local" needs sentence-transformers,
    "openai" / "google" / "minimax" need their respective env vars). Falls back
    to FTS5 / keyword matching when no matching embeddings exist for the given
    provider.

    Args:
        query: Search string to match against node names.
        kind: Optional filter: File, Class, Function, Type, or Test.
        limit: Maximum results. Default: 20.
        repo_root: Repository root path. Auto-detected if omitted.
        model: Embedding model for query vectors. Must match the model used
               during embed_graph. Falls back to CRG_EMBEDDING_MODEL env var
               (local) or CRG_OPENAI_MODEL (openai).
        provider: Embedding provider: "local" (default), "openai", "google",
                  or "minimax". Must match the provider used during embed_graph.
        detail_level: "standard" for full output, "minimal" for compact summary. Default: standard.
    """
    return semantic_search_nodes(
        query=query,
        kind=kind,
        limit=limit,
        repo_root=_resolve_repo_root(repo_root),
        model=model,
        provider=provider,
        detail_level=detail_level,
    )


@mcp.tool()
async def embed_graph_tool(
    repo_root: Optional[str] = None,
    model: Optional[str] = None,
    provider: Optional[str] = None,
) -> dict:
    """Compute vector embeddings for all graph nodes to enable semantic search.

    Requires: pip install "dagayn[embeddings] @ git+https://github.com/manji-0/dagayn.git"
    (local provider only; cloud providers use stdlib urllib).
    Default provider: local. Default model: all-MiniLM-L6-v2.
    Override provider via `provider` param, model via `model` param or
    CRG_EMBEDDING_MODEL / CRG_OPENAI_MODEL env vars.
    Changing the model or provider re-embeds all nodes automatically.

    After running this, semantic_search_nodes_tool will use vector similarity
    instead of keyword matching for much better results.

    Runs the blocking sentence-transformers / Gemini / HTTP inference in a
    thread via ``asyncio.to_thread`` so the stdio event loop stays
    responsive — without this wrapper, embedding a large graph would
    silently hang the MCP server on Windows. See: #46, #136.

    Args:
        repo_root: Repository root path. Auto-detected if omitted.
        model: Embedding model. For local: HuggingFace ID/path; for openai:
               model ID (e.g. "text-embedding-3-small"); for google: Gemini
               model ID. Falls back to CRG_EMBEDDING_MODEL / CRG_OPENAI_MODEL
               env vars as appropriate.
        provider: "local" (default), "openai", "google", or "minimax".
                  "openai" requires CRG_OPENAI_BASE_URL + CRG_OPENAI_API_KEY +
                  CRG_OPENAI_MODEL env vars and accepts any OpenAI-compatible
                  endpoint (real OpenAI, Azure, new-api, LiteLLM, vLLM, etc.).
    """
    return await asyncio.to_thread(
        embed_graph,
        repo_root=_resolve_repo_root(repo_root),
        model=model,
        provider=provider,
    )


@mcp.tool()
def list_graph_stats_tool(
    repo_root: Optional[str] = None,
) -> dict:
    """Get aggregate statistics about the code knowledge graph.

    Shows total nodes, edges, languages, files, and last update time.
    Useful for checking if the graph is built and up to date.

    Args:
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return list_graph_stats(repo_root=_resolve_repo_root(repo_root))


@mcp.tool()
def get_docs_section_tool(
    section_name: str,
    repo_root: Optional[str] = None,
    max_chars: int = 4000,
) -> dict:
    """Get a specific section from the LLM-optimized documentation reference.

    Returns only the requested section content for minimal token usage.
    Use this before answering any user question about the plugin.

    Available sections: usage, review-delta, review-pr, commands, legal,
    watch, embeddings, languages, troubleshooting.

    Args:
        section_name: The section to retrieve (e.g. "review-delta", "usage").
        repo_root: Repository root path. Auto-detected if omitted.
        max_chars: Maximum characters to return. Default: 4000.
    """
    return get_docs_section(section_name=section_name, repo_root=repo_root, max_chars=max_chars)


@mcp.tool()
def find_large_functions_tool(
    min_lines: int = 50,
    kind: Optional[str] = None,
    file_path_pattern: Optional[str] = None,
    limit: int = 50,
    repo_root: Optional[str] = None,
) -> dict:
    """Find functions, classes, or files exceeding a line-count threshold.

    Useful for decomposition audits, code quality checks, and enforcing
    size limits during code review. Results are ordered by line count.

    Args:
        min_lines: Minimum line count to flag. Default: 50.
        kind: Optional filter: Function, Class, File, or Test.
        file_path_pattern: Filter by file path substring (e.g. "components/").
        limit: Maximum results. Default: 50.
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return find_large_functions(
        min_lines=min_lines,
        kind=kind,
        file_path_pattern=file_path_pattern,
        limit=limit,
        repo_root=_resolve_repo_root(repo_root),
    )


@mcp.tool()
def list_flows_tool(
    sort_by: str = "criticality",
    limit: int = 50,
    kind: Optional[str] = None,
    detail_level: str = "standard",
    repo_root: Optional[str] = None,
) -> dict:
    """List execution flows in the codebase, sorted by criticality.

    Each flow represents a call chain starting from an entry point
    (HTTP handler, CLI command, test function, etc.). Use this to
    understand the main execution paths through the codebase.

    Args:
        sort_by: Sort column: criticality, depth, node_count, file_count, or name.
        limit: Maximum flows to return. Default: 50.
        kind: Optional filter by entry point kind (e.g. "Test", "Function").
        detail_level: "standard" (default) returns full flow data; "minimal"
                      returns only name, criticality, and node_count per flow.
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return list_flows(
        repo_root=_resolve_repo_root(repo_root),
        sort_by=sort_by,
        limit=limit,
        kind=kind,
        detail_level=detail_level,
    )


@mcp.tool()
def get_flow_tool(
    flow_id: Optional[int] = None,
    flow_name: Optional[str] = None,
    include_source: bool = False,
    repo_root: Optional[str] = None,
) -> dict:
    """Get detailed information about a single execution flow.

    Returns the full call path with each step's function name, file, and
    line numbers. Optionally includes source code snippets for each step.

    Provide either flow_id (from list_flows_tool) or flow_name to search by name.

    Args:
        flow_id: Database ID of the flow.
        flow_name: Name to search for (partial match). Ignored if flow_id given.
        include_source: Include source code snippets for each step. Default: False.
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return get_flow(
        flow_id=flow_id,
        flow_name=flow_name,
        include_source=include_source,
        repo_root=_resolve_repo_root(repo_root),
    )


@mcp.tool()
def get_affected_flows_tool(
    changed_files: Optional[list[str]] = None,
    base: str = "HEAD~1",
    repo_root: Optional[str] = None,
) -> dict:
    """Find execution flows affected by changed files.

    Identifies which execution flows pass through nodes in the changed files.
    Useful during code review to understand which user-facing or critical paths
    are impacted by a change. Auto-detects changed files from git if not specified.

    Args:
        changed_files: List of changed file paths (relative to repo root). Auto-detected if omitted.
        base: Git ref for auto-detecting changes. Default: HEAD~1.
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return get_affected_flows_func(
        changed_files=changed_files,
        base=base,
        repo_root=_resolve_repo_root(repo_root),
    )


@mcp.tool()
def list_communities_tool(
    sort_by: str = "size",
    min_size: int = 0,
    detail_level: str = "standard",
    repo_root: Optional[str] = None,
) -> dict:
    """List detected code communities in the codebase.

    Each community represents a cluster of related code entities (functions,
    classes) detected via the Leiden algorithm or file-based grouping.
    Use this to understand the high-level structure of the codebase.

    Args:
        sort_by: Sort column: size, cohesion, or name.
        min_size: Minimum community size to include. Default: 0.
        detail_level: "standard" (default) returns full community data;
                      "minimal" returns only name, size, and cohesion
                      per community.
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return list_communities_func(
        repo_root=_resolve_repo_root(repo_root),
        sort_by=sort_by,
        min_size=min_size,
        detail_level=detail_level,
    )


@mcp.tool()
def get_community_tool(
    community_name: Optional[str] = None,
    community_id: Optional[int] = None,
    include_members: bool = False,
    repo_root: Optional[str] = None,
) -> dict:
    """Get detailed information about a single code community.

    Returns community metadata including size, cohesion, dominant language,
    and member list. Optionally includes full node details for each member.

    Provide either community_id (from list_communities_tool) or community_name
    to search by name.

    Args:
        community_name: Name to search for (partial match). Ignored if community_id given.
        community_id: Database ID of the community.
        include_members: Include full member node details. Default: False.
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return get_community_func(
        community_name=community_name,
        community_id=community_id,
        include_members=include_members,
        repo_root=_resolve_repo_root(repo_root),
    )


@mcp.tool()
def get_architecture_overview_tool(
    repo_root: Optional[str] = None,
    detail_level: str = "standard",
    top_n: int = 20,
) -> dict:
    """Generate an architecture overview based on community structure.

    Builds a high-level view of the codebase architecture by analyzing
    community boundaries and cross-community coupling. Includes warnings
    for high coupling between communities.

    detail_level controls output verbosity:
      "minimal"  — name/size/cohesion per community, top-5 coupling pairs, warnings only
      "standard" — full community metadata (no member lists), top-N coupling pairs (default)
      "verbose"  — includes member lists and raw per-edge cross_community_edges list

    Args:
        repo_root: Repository root path. Auto-detected if omitted.
        detail_level: Output verbosity: "minimal", "standard" (default), or "verbose".
        top_n: Maximum coupled pairs to include in standard mode. Default 20.
    """
    return get_architecture_overview_func(
        repo_root=_resolve_repo_root(repo_root),
        detail_level=detail_level,
        top_n=top_n,
    )


@mcp.tool()
async def detect_changes_tool(
    base: str = "HEAD~1",
    changed_files: Optional[list[str]] = None,
    include_source: bool = False,
    max_depth: int = 2,
    repo_root: Optional[str] = None,
    detail_level: str = "standard",
) -> dict:
    """Detect changes and produce risk-scored, priority-ordered review guidance.

    Primary tool for code review. Maps git diffs to affected functions,
    flows, communities, and test coverage gaps. Returns risk scores and
    prioritized review items. Replaces get_review_context for change-aware reviews.

    Offloaded to a thread via ``asyncio.to_thread`` — runs `git diff`
    subprocesses and BFS traversals that can take several seconds on
    large repos. See: #46, #136.

    Args:
        base: Git ref to diff against. Default: HEAD~1.
        changed_files: List of changed file paths (relative to repo root). Auto-detected if omitted.
        include_source: Include source code snippets for changed functions. Default: False.
        max_depth: Impact radius depth for BFS traversal. Default: 2.
        repo_root: Repository root path. Auto-detected if omitted.
        detail_level: "standard" for full output, "minimal" for
            token-efficient summary. Default: standard.
    """
    return await asyncio.to_thread(
        detect_changes_func,
        base=base,
        changed_files=changed_files,
        include_source=include_source,
        max_depth=max_depth,
        repo_root=_resolve_repo_root(repo_root),
        detail_level=detail_level,
    )


@mcp.tool()
def refactor_tool(
    mode: str = "rename",
    old_name: Optional[str] = None,
    new_name: Optional[str] = None,
    kind: Optional[str] = None,
    file_pattern: Optional[str] = None,
    limit: int = 50,
    repo_root: Optional[str] = None,
) -> dict:
    """Graph-powered refactoring operations.

    Unified entry point for rename previews, dead code detection, and
    refactoring suggestions.

    Modes:
    - rename: Preview renaming a symbol. Returns an edit list and a refactor_id
      to pass to apply_refactor_tool. Requires old_name and new_name.
    - dead_code: Find unreferenced functions/classes (no callers, tests, or
      importers, and not entry points).
    - suggest: Get community-driven refactoring suggestions (move misplaced
      functions, remove dead code).

    Args:
        mode: Operation mode: "rename", "dead_code", or "suggest".
        old_name: (rename) Current symbol name to rename.
        new_name: (rename) Desired new name for the symbol.
        kind: (dead_code) Optional filter: Function or Class.
        file_pattern: (dead_code) Filter by file path substring.
        limit: (dead_code, suggest) Maximum results to return. Default: 50.
            When truncated, total shows the full count.
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return refactor_func(
        mode=mode,
        old_name=old_name,
        new_name=new_name,
        kind=kind,
        file_pattern=file_pattern,
        limit=limit,
        repo_root=_resolve_repo_root(repo_root),
    )


@mcp.tool()
def apply_refactor_tool(
    refactor_id: str,
    repo_root: Optional[str] = None,
    dry_run: bool = False,
) -> dict:
    """Apply a previously previewed refactoring to source files.

    Takes a refactor_id from a prior refactor_tool(mode="rename") call and
    applies the exact string replacements to the target files. Previews
    expire after 10 minutes.

    Security: All edit paths are validated to be within the repo root.
    Only exact string replacements are performed (no regex, no eval).

    Args:
        refactor_id: The refactor ID from refactor_tool's response.
        repo_root: Repository root path. Auto-detected if omitted.
        dry_run: If True, return a unified diff of what would change
            without touching any files. The refactor_id remains valid so
            the same preview can be applied in a follow-up call without
            dry_run. Use this for a human-in-the-loop review before
            committing changes to disk. See: #176
    """
    return apply_refactor_func(
        refactor_id=refactor_id,
        repo_root=_resolve_repo_root(repo_root),
        dry_run=dry_run,
    )


@mcp.tool()
async def generate_wiki_tool(
    repo_root: Optional[str] = None,
    force: bool = False,
) -> dict:
    """Generate a markdown wiki from the code community structure.

    Creates a wiki page for each detected community and an index page.
    Pages are written to .dagayn/wiki/ inside the repository.
    Only regenerates pages whose content has changed unless force=True.

    Offloaded to a thread via ``asyncio.to_thread`` — on large graphs
    the page-generation loop touches every community and issues many
    SQLite reads, which would block the MCP event loop. See: #46, #136.

    Args:
        repo_root: Repository root path. Auto-detected if omitted.
        force: If True, regenerate all pages even if content unchanged. Default: False.
    """
    return await asyncio.to_thread(
        generate_wiki_func,
        repo_root=_resolve_repo_root(repo_root),
        force=force,
    )


@mcp.tool()
def get_wiki_page_tool(
    community_name: str,
    repo_root: Optional[str] = None,
) -> dict:
    """Retrieve a specific wiki page by community name.

    Returns the markdown content of the wiki page for the given community.
    The wiki must have been generated first via generate_wiki_tool.

    Args:
        community_name: Community name to look up.
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return get_wiki_page_func(
        community_name=community_name,
        repo_root=_resolve_repo_root(repo_root),
    )


@mcp.tool()
def get_hub_nodes_tool(
    top_n: int = 10,
    repo_root: Optional[str] = None,
) -> dict:
    """Find the most connected nodes in the codebase (architectural hotspots).

    Hub nodes have the highest total degree (in + out edges). Changes to
    them have disproportionate blast radius. Excludes File nodes.

    Args:
        top_n: Number of top hubs to return. Default: 10.
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return get_hub_nodes_func(
        repo_root=_resolve_repo_root(repo_root),
        top_n=top_n,
    )


@mcp.tool()
def get_bridge_nodes_tool(
    top_n: int = 10,
    repo_root: Optional[str] = None,
) -> dict:
    """Find architectural chokepoints via betweenness centrality.

    Bridge nodes sit on shortest paths between many node pairs.
    If they break, multiple code regions lose connectivity.
    Uses sampling approximation for graphs > 5000 nodes.

    Args:
        top_n: Number of top bridges to return. Default: 10.
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return get_bridge_nodes_func(
        repo_root=_resolve_repo_root(repo_root),
        top_n=top_n,
    )


@mcp.tool()
def get_knowledge_gaps_tool(
    repo_root: Optional[str] = None,
) -> dict:
    """Identify structural weaknesses in the codebase graph.

    Finds isolated nodes (disconnected), thin communities (< 3 members),
    untested hotspots (high-degree nodes without test coverage), and
    single-file communities.

    Args:
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return get_knowledge_gaps_func(
        repo_root=_resolve_repo_root(repo_root),
    )


@mcp.tool()
def get_surprising_connections_tool(
    top_n: int = 15,
    repo_root: Optional[str] = None,
) -> dict:
    """Find unexpected architectural coupling via composite surprise scoring.

    Scores edges by: cross-community (+0.3), cross-language (+0.2),
    peripheral-to-hub (+0.2), cross-test-boundary (+0.15), and
    unusual edge kinds (+0.15).

    Args:
        top_n: Number of top surprises to return. Default: 15.
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return get_surprising_connections_func(
        repo_root=_resolve_repo_root(repo_root),
        top_n=top_n,
    )


@mcp.tool()
def get_suggested_questions_tool(
    top_n: int = 15,
    repo_root: Optional[str] = None,
) -> dict:
    """Auto-generate review questions from graph analysis.

    Produces prioritized questions about: bridge nodes needing tests,
    untested hub nodes, surprising cross-community coupling, thin
    communities, and untested hotspots.

    Args:
        top_n: Maximum questions to return, high-priority first. Default: 15.
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return get_suggested_questions_func(
        repo_root=_resolve_repo_root(repo_root),
        top_n=top_n,
    )


@mcp.tool()
def traverse_graph_tool(
    query: str,
    mode: str = "bfs",
    depth: int = 3,
    token_budget: int = 2000,
    repo_root: Optional[str] = None,
) -> dict:
    """BFS/DFS traversal from best-matching node with token budget.

    Free-form graph exploration: finds the node best matching your
    query, then traverses outward via BFS or DFS up to the given
    depth, collecting connected nodes within the token budget.

    Args:
        query: Search string to find the starting node.
        mode: Traversal mode: "bfs" (breadth-first) or "dfs"
            (depth-first). Default: bfs.
        depth: Max traversal depth (1-6). Default: 3.
        token_budget: Approximate token limit for results.
            Default: 2000.
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return traverse_graph_func(
        query=query,
        mode=mode,
        depth=depth,
        token_budget=token_budget,
        repo_root=_resolve_repo_root(repo_root) or "",
    )


@mcp.tool()
def detect_adp_violations_tool(
    granularity: Literal["file", "package"] = "package",
    min_cycle_size: int = 2,
    max_cycle_length: int = 10,
    top_n: int = 30,
    repo_root: Optional[str] = None,
) -> dict:
    """Find cyclic dependencies that violate the Acyclic Dependencies Principle.

    Scans IMPORTS_FROM and DEPENDS_ON edges for directed cycles. Each
    violation lists the nodes in the cycle, its length, and a severity
    score. Use granularity="file" for per-file cycles or "package" for
    directory-level aggregation (default).

    Args:
        granularity: "package" (directory-level) or "file". Default: package.
        min_cycle_size: Minimum nodes in a cycle to report. Default: 2.
        max_cycle_length: Upper bound on cycle length searched. Default: 10.
        top_n: Maximum violations to return, ordered by severity. Default: 30.
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return detect_adp_violations_func(
        repo_root=_resolve_repo_root(repo_root),
        granularity=granularity,
        min_cycle_size=min_cycle_size,
        max_cycle_length=max_cycle_length,
        top_n=top_n,
    )


@mcp.tool()
def compute_sdp_metrics_tool(
    granularity: Literal["file", "package"] = "package",
    top_n: int = 30,
    repo_root: Optional[str] = None,
) -> dict:
    """Compute Stable Dependencies Principle instability scores per module.

    Instability I = Ce / (Ca + Ce), where Ca = afferent couplings (how
    many modules depend on this one) and Ce = efferent couplings (how many
    modules this one depends on). I = 0 is maximally stable, I = 1 is
    maximally unstable.

    Args:
        granularity: "package" (directory-level) or "file". Default: package.
        top_n: Return the N most unstable entries. Default: 30.
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return compute_sdp_metrics_func(
        repo_root=_resolve_repo_root(repo_root),
        granularity=granularity,
        top_n=top_n,
    )


@mcp.tool()
def detect_sdp_violations_tool(
    granularity: Literal["file", "package"] = "package",
    min_delta: float = 0.1,
    repo_root: Optional[str] = None,
) -> dict:
    """Find SDP violations: stable modules that depend on unstable ones.

    Reports edges A -> B where I(A) < I(B) - min_delta. A stable module
    depending on an unstable one increases coupling risk — changes to the
    unstable module force changes in the stable one.

    Args:
        granularity: "package" (directory-level) or "file". Default: package.
        min_delta: Minimum instability gap to flag. Default: 0.1.
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return detect_sdp_violations_func(
        repo_root=_resolve_repo_root(repo_root),
        granularity=granularity,
        min_delta=min_delta,
    )


@mcp.tool()
def compute_sap_metrics_tool(
    scope_kind: Literal["file", "package", "directory"] = "package",
    unit_filter: Optional[list[str]] = None,
    top_n: int = 30,
    repo_root: Optional[str] = None,
) -> dict:
    """Compute SAP abstractness, instability, and distance scores per scope.

    Measures A = Na/Nt (abstractness), I = Ce/(Ca+Ce) (instability), and
    D = |A+I-1| (distance from the main sequence) for each scope. Scopes
    with high D are either in the Zone of Pain (concrete + stable) or the
    Zone of Uselessness (abstract + unstable).

    Dependency edges: IMPORTS_FROM + DEPENDS_ON + INHERITS + IMPLEMENTS (fixed).

    Args:
        scope_kind: "package" (directory-level, default), "file", or "directory".
        unit_filter: Optional list of scope_key prefix strings to restrict output.
        top_n: Return the N entries with highest distance. Default: 30.
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return compute_sap_metrics_func(
        repo_root=_resolve_repo_root(repo_root),
        scope_kind=scope_kind,
        unit_filter=unit_filter,
        top_n=top_n,
    )


@mcp.tool()
def detect_sap_violations_tool(
    scope_kind: Literal["file", "package", "directory"] = "package",
    min_distance: float = 0.5,
    repo_root: Optional[str] = None,
) -> dict:
    """Find scopes that violate the Stable Abstractions Principle.

    Reports scopes where D = |A+I-1| > min_distance. High-D scopes are
    architectural risks: Zone of Pain scopes are hard to change because
    everything depends on them; Zone of Uselessness scopes are abstract
    but have no dependents.

    Dependency edges: IMPORTS_FROM + DEPENDS_ON + INHERITS + IMPLEMENTS (fixed).

    Args:
        scope_kind: "package" (default), "file", or "directory".
        min_distance: Minimum D to flag (exclusive). Default: 0.5.
        repo_root: Repository root path. Auto-detected if omitted.
    """
    return detect_sap_violations_func(
        repo_root=_resolve_repo_root(repo_root),
        scope_kind=scope_kind,
        min_distance=min_distance,
    )


@mcp.tool()
def list_repos_tool() -> dict:
    """List all registered repositories in the multi-repo registry.

    Returns the list of repos registered at ~/.dagayn/registry.json.
    Use the CLI 'register' command to add repos.
    """
    return list_repos_func()


@mcp.tool()
def cross_repo_search_tool(
    query: str,
    kind: Optional[str] = None,
    limit: int = 20,
) -> dict:
    """Search for code entities across all registered repositories.

    Runs hybrid search on each registered repo's graph database and merges
    the results by score. Register repos first with the CLI 'register' command.

    Args:
        query: Search string to match against node names.
        kind: Optional filter: File, Class, Function, Type, or Test.
        limit: Maximum results per repo. Default: 20.
    """
    return cross_repo_search_func(query=query, kind=kind, limit=limit)


@mcp.prompt()
def review_changes(base: str = "HEAD~1") -> list[dict]:
    """Pre-commit review workflow using detect_changes, affected_flows, and test gaps.

    Produces a structured code review with risk levels and actionable findings.

    Args:
        base: Git ref to diff against. Default: HEAD~1.
    """
    return review_changes_prompt(base=base)


@mcp.prompt()
def architecture_map() -> list[dict]:
    """Architecture documentation using communities, flows, and Mermaid diagrams.

    Generates a comprehensive architecture map with module summaries and coupling warnings.
    """
    return architecture_map_prompt()


@mcp.prompt()
def debug_issue(description: str = "") -> list[dict]:
    """Guided debugging using search, flow tracing, and recent changes.

    Systematic debugging workflow that traces execution paths and identifies root causes.

    Args:
        description: Description of the issue to debug.
    """
    return debug_issue_prompt(description=description)


@mcp.prompt()
def onboard_developer() -> list[dict]:
    """New developer orientation using stats, architecture, and critical flows.

    Creates an onboarding guide covering codebase structure, key modules, and patterns.
    """
    return onboard_developer_prompt()


@mcp.prompt()
def pre_merge_check(base: str = "HEAD~1") -> list[dict]:
    """PR readiness check with risk scoring, test gaps, and dead code detection.

    Produces a merge readiness report with risk assessment and recommendations.

    Args:
        base: Git ref to diff against. Default: HEAD~1.
    """
    return pre_merge_check_prompt(base=base)


def _tool_components() -> dict[str, Any]:
    """Return FastMCP's local component registry for registered tools/prompts."""
    provider = getattr(mcp, "_local_provider", None)
    components = getattr(provider, "_components", None)
    if not isinstance(components, dict):
        raise RuntimeError("FastMCP local provider registry is unavailable")
    return components


def _registered_tool_names() -> list[str]:
    """List registered MCP tool names without depending on removed internals."""
    names: list[str] = []
    for key, component in _tool_components().items():
        if not key.startswith("tool:"):
            continue
        name = getattr(component, "name", None)
        if isinstance(name, str):
            names.append(name)
    return names


def _snapshot_components() -> dict[str, Any]:
    """Copy the current FastMCP local component registry."""
    return dict(_tool_components())


def _restore_components(snapshot: dict[str, Any]) -> None:
    """Restore the FastMCP local component registry from a snapshot."""
    components = _tool_components()
    components.clear()
    components.update(snapshot)


def _apply_tool_filter(tools: str | None = None) -> None:
    """Remove tools not listed in the allow-list.

    Accepts a comma-separated string of tool names to keep.  When set,
    every registered MCP tool whose name is **not** in the list is
    removed via ``FastMCP.remove_tool()``.

    The allow-list can be supplied in two ways (first match wins):

    1. ``tools`` argument (from ``serve --tools ...``).
    2. ``CRG_TOOLS`` environment variable.

    When neither is set, all tools remain available.

    This is useful for token-constrained environments: CRG exposes 28+
    tools by default (~8k description tokens per LLM turn).  Filtering
    to a working set of 5-10 tools can reduce overhead by 70-85%.

    Example::

        # via CLI
        dagayn serve --tools query_graph_tool,semantic_search_nodes_tool

        # via env var
        CRG_TOOLS=query_graph_tool,semantic_search_nodes_tool
    """
    import os

    raw = tools or os.environ.get("CRG_TOOLS")
    if not raw:
        return
    allowed = {t.strip() for t in raw.split(",") if t.strip()}
    if not allowed:
        return
    registered = _registered_tool_names()
    for name in registered:
        if name not in allowed:
            mcp.remove_tool(name)


def main(
    repo_root: str | None = None,
    tools: str | None = None,
    *,
    transport: str = "stdio",
    host: str | None = None,
    port: int | None = None,
) -> None:
    """Run the MCP server (stdio or HTTP).

    On Windows, Python 3.8+ defaults to ``ProactorEventLoop``, which
    interacts poorly with ``concurrent.futures.ProcessPoolExecutor``
    (used by ``full_build``) over a stdio MCP transport — the combination
    produces silent hangs on ``build_or_update_graph_tool`` and
    ``embed_graph_tool``. Switching to ``WindowsSelectorEventLoopPolicy``
    before fastmcp starts its loop avoids the deadlock.
    See: #46, #136

    Args:
        repo_root: Default repository root for all tool calls.
        tools: Comma-separated list of tool names to expose.
            Falls back to ``CRG_TOOLS`` env var.  When unset, all
            tools are available.
        transport: ``"stdio"`` (default) or ``"streamable-http"`` for local HTTP.
        host: Bind address when using HTTP (required for HTTP; set by CLI).
        port: Port when using HTTP (required for HTTP; set by CLI).
    """
    global _default_repo_root
    _default_repo_root = repo_root
    _apply_tool_filter(tools)
    if sys.platform == "win32":
        import asyncio

        asyncio.set_event_loop_policy(asyncio.WindowsSelectorEventLoopPolicy())
    if transport == "stdio":
        # Stdio MCP must keep stdout strictly JSON-RPC. FastMCP's banner/update
        # notices corrupt the handshake stream on clients like Codex CLI.
        mcp.run(transport="stdio", show_banner=False)
    elif transport == "streamable-http":
        if host is None or port is None:
            raise ValueError("streamable-http transport requires host and port")
        mcp.run(transport="streamable-http", host=host, port=port)
    else:
        raise ValueError(f"unsupported transport: {transport!r}")


if __name__ == "__main__":
    main()
