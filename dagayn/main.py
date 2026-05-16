"""MCP server entry point for Dagayn.

Run as: dagayn serve
Communicates via stdio (standard MCP transport), or use
``dagayn serve --http`` for Streamable HTTP on localhost (port 5555
by default).
"""

from __future__ import annotations

import asyncio
import os
import sys
from importlib import import_module
from typing import Any, Literal, Optional

from .prompts import (
    architecture_map_prompt,
    debug_issue_prompt,
    onboard_developer_prompt,
    pre_merge_check_prompt,
    review_changes_prompt,
)
from .tool_profiles import (
    DEFAULT_TOOL_PROFILE,
    TOOL_PROFILE_ENV_VARS,
    TOOL_PROFILES,
)


class _FallbackComponent:
    """Small component record used when FastMCP cannot import."""

    def __init__(self, name: str, fn: Any) -> None:
        self.name = name
        self.fn = fn


class _FallbackProvider:
    def __init__(self) -> None:
        self._components: dict[str, _FallbackComponent] = {}


class _FallbackFastMCP:
    """Enough FastMCP surface for tests when FastMCP's deps are incompatible."""

    def __init__(self, *_args: Any, **_kwargs: Any) -> None:
        self._local_provider = _FallbackProvider()

    def tool(self, *_args: Any, **_kwargs: Any) -> Any:
        def decorator(fn: Any) -> Any:
            self._local_provider._components[f"tool:{fn.__name__}"] = _FallbackComponent(
                fn.__name__,
                fn,
            )
            return fn

        return decorator

    def prompt(self, *_args: Any, **_kwargs: Any) -> Any:
        def decorator(fn: Any) -> Any:
            self._local_provider._components[f"prompt:{fn.__name__}"] = _FallbackComponent(
                fn.__name__,
                fn,
            )
            return fn

        return decorator

    async def list_tools(self) -> list[_FallbackComponent]:
        return [
            component
            for key, component in self._local_provider._components.items()
            if key.startswith("tool:")
        ]

    def remove_tool(self, name: str) -> None:
        self._local_provider._components.pop(f"tool:{name}", None)

    def run(self, **_kwargs: Any) -> None:
        raise RuntimeError(
            "FastMCP could not be imported; install compatible FastMCP dependencies "
            "to run the MCP server."
        ) from _FASTMCP_IMPORT_ERROR


try:
    from fastmcp import FastMCP as _ImportedFastMCP

    FastMCP: Any = _ImportedFastMCP
    _FASTMCP_IMPORT_ERROR: BaseException | None = None
except (ImportError, TypeError) as exc:
    FastMCP = _FallbackFastMCP
    _FASTMCP_IMPORT_ERROR = exc

# NOTE: Thread-safe for stdio MCP (single-threaded). If adding HTTP/SSE
# transport with concurrent requests, replace with contextvars.ContextVar.
_default_repo_root: str | None = None
_default_embedding_provider: str | None = None
_default_embedding_model: str | None = None


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


def _infer_remote_embedding_provider_from_env() -> str | None:
    """Infer a remote embedding provider from the current server environment.

    ``dagayn install --mode remote`` asks users to launch the MCP server from
    an environment that contains exactly one remote provider's credentials.
    When that is true, make MCP search use that provider automatically.  If
    multiple remote providers are configured, stay unset so clients can choose
    explicitly rather than guessing which external API should receive queries.
    """
    candidates: list[str] = []
    if all(
        os.environ.get(name)
        for name in ("CRG_OPENAI_API_KEY", "CRG_OPENAI_BASE_URL", "CRG_OPENAI_MODEL")
    ):
        candidates.append("openai")
    if os.environ.get("GOOGLE_API_KEY"):
        candidates.append("google")
    if os.environ.get("MINIMAX_API_KEY"):
        candidates.append("minimax")
    return candidates[0] if len(candidates) == 1 else None


def _resolve_embedding_provider(provider: Optional[str]) -> Optional[str]:
    """Resolve the provider for search, preserving explicit client choice."""
    return provider if provider else _default_embedding_provider


def _resolve_embedding_model(model: Optional[str]) -> Optional[str]:
    """Resolve the embedding model for search, preserving explicit client choice."""
    return model if model else _default_embedding_model


def _tool(name: str) -> Any:
    """Resolve a tool implementation lazily to keep package imports acyclic."""
    return getattr(import_module("dagayn.tools"), name)


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
    local_embedding: Optional[str] = None,
    local_embedding_port: int = 18080,
    local_embedding_bin: str = "llama-server",
    keep_local_embedding_server: bool = False,
    local_embedding_timeout: int = 300,
    local_embedding_request_timeout: int = 60,
    local_embedding_batch_size: int = 1,
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
        local_embedding: Optional local Qwen embedding preset: "low" or "high".
        local_embedding_port: localhost port for the managed llama-server.
        local_embedding_bin: llama-server executable name or path.
        keep_local_embedding_server: Leave a dagayn-started server running.
        local_embedding_timeout: Seconds to wait for llama-server readiness.
        local_embedding_request_timeout: Seconds to wait for each embedding
            HTTP request after llama-server is ready.
        local_embedding_batch_size: Texts to send in each local embedding
            HTTP request.
    """
    return await asyncio.to_thread(
        _tool("build_or_update_graph"),
        full_rebuild=full_rebuild,
        repo_root=_resolve_repo_root(repo_root),
        base=base,
        postprocess=postprocess,
        recurse_submodules=recurse_submodules,
        local_embedding=local_embedding,
        local_embedding_port=local_embedding_port,
        local_embedding_bin=local_embedding_bin,
        keep_local_embedding_server=keep_local_embedding_server,
        local_embedding_timeout=local_embedding_timeout,
        local_embedding_request_timeout=local_embedding_request_timeout,
        local_embedding_batch_size=local_embedding_batch_size,
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
        _tool("run_postprocess"),
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
    return _tool("get_minimal_context")(
        task=task,
        changed_files=changed_files,
        repo_root=_resolve_repo_root(repo_root),
        base=base,
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
    - docs_for: Find docs linked to a code/Terraform/artifact node
    - implementations_of: Find implementation artifacts linked to a document node
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
    return _tool("query_graph")(
        pattern=pattern,
        target=target,
        repo_root=_resolve_repo_root(repo_root),
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
    return _tool("semantic_search_nodes")(
        query=query,
        kind=kind,
        limit=limit,
        repo_root=_resolve_repo_root(repo_root),
        model=_resolve_embedding_model(model),
        provider=_resolve_embedding_provider(provider),
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
        _tool("embed_graph"),
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
    return _tool("list_graph_stats")(repo_root=_resolve_repo_root(repo_root))


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
    return _tool("get_docs_section")(
        section_name=section_name,
        repo_root=repo_root,
        max_chars=max_chars,
    )


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
    return _tool("find_large_functions")(
        min_lines=min_lines,
        kind=kind,
        file_path_pattern=file_path_pattern,
        limit=limit,
        repo_root=_resolve_repo_root(repo_root),
    )


@mcp.tool()
def architecture_analysis_tool(
    mode: Literal[
        "overview",
        "communities",
        "community",
        "hubs",
        "bridges",
        "knowledge_gaps",
        "surprising_connections",
        "adp_violations",
        "sdp_metrics",
        "sdp_violations",
        "sap_metrics",
        "sap_violations",
    ] = "overview",
    detail_level: Literal["minimal", "standard", "verbose"] = "minimal",
    top_n: int = 10,
    sort_by: Literal["size", "cohesion", "name"] = "size",
    min_size: int = 0,
    community_name: Optional[str] = None,
    community_id: Optional[int] = None,
    include_members: bool = False,
    granularity: Literal["file", "package"] = "package",
    scope_kind: Literal["file", "package", "directory"] = "package",
    unit_filter: Optional[list[str]] = None,
    min_cycle_size: int = 2,
    max_cycle_length: int = 10,
    min_delta: float = 0.1,
    min_distance: float = 0.5,
    repo_root: Optional[str] = None,
) -> dict:
    """Run architecture analysis through a single mode-based dispatcher."""
    return _tool("architecture_analysis_func")(
        mode=mode,
        detail_level=detail_level,
        top_n=top_n,
        sort_by=sort_by,
        min_size=min_size,
        community_name=community_name,
        community_id=community_id,
        include_members=include_members,
        granularity=granularity,
        scope_kind=scope_kind,
        unit_filter=unit_filter,
        min_cycle_size=min_cycle_size,
        max_cycle_length=max_cycle_length,
        min_delta=min_delta,
        min_distance=min_distance,
        repo_root=_resolve_repo_root(repo_root),
    )


@mcp.tool()
async def review_tool(
    mode: Literal["changes", "context", "affected_flows", "impact"] = "changes",
    base: str = "HEAD~1",
    changed_files: Optional[list[str]] = None,
    include_source: Optional[bool] = None,
    max_depth: int = 2,
    max_nodes: int = 50,
    max_lines_per_file: int = 200,
    repo_root: Optional[str] = None,
    detail_level: Literal["minimal", "standard"] = "standard",
) -> dict:
    """Run review analysis through a single mode-based dispatcher."""
    return await asyncio.to_thread(
        _tool("review_func"),
        mode=mode,
        base=base,
        changed_files=changed_files,
        include_source=include_source,
        max_depth=max_depth,
        max_nodes=max_nodes,
        max_lines_per_file=max_lines_per_file,
        repo_root=_resolve_repo_root(repo_root),
        detail_level=detail_level,
    )


@mcp.tool()
def flow_tool(
    mode: Literal["list", "get"] = "list",
    sort_by: Literal["criticality", "depth", "node_count", "file_count", "name"] = "criticality",
    limit: int = 50,
    kind: Optional[str] = None,
    detail_level: Literal["minimal", "standard"] = "standard",
    flow_id: Optional[int] = None,
    flow_name: Optional[str] = None,
    include_source: bool = False,
    repo_root: Optional[str] = None,
) -> dict:
    """Run execution-flow analysis through a single mode-based dispatcher."""
    return _tool("flow_func")(
        mode=mode,
        sort_by=sort_by,
        limit=limit,
        kind=kind,
        detail_level=detail_level,
        flow_id=flow_id,
        flow_name=flow_name,
        include_source=include_source,
        repo_root=_resolve_repo_root(repo_root),
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
    - suggest: Get graph-backed refactoring suggestions, including remove,
      move, split, and document candidates.

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
    return _tool("refactor_func")(
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
    return _tool("apply_refactor_func")(
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
        _tool("generate_wiki_func"),
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
    return _tool("get_wiki_page_func")(
        community_name=community_name,
        repo_root=_resolve_repo_root(repo_root),
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
    return _tool("get_suggested_questions_func")(
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
    model: Optional[str] = None,
    provider: Optional[str] = None,
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
        model: Embedding model for the initial search. Defaults to the
            server's embedding model when configured by ``dagayn serve``.
        provider: Embedding provider for the initial search. Defaults to the
            server's embedding provider when configured by ``dagayn serve``.
    """
    return _tool("traverse_graph_func")(
        query=query,
        mode=mode,
        depth=depth,
        token_budget=token_budget,
        repo_root=_resolve_repo_root(repo_root) or "",
        model=_resolve_embedding_model(model),
        provider=_resolve_embedding_provider(provider),
    )


@mcp.tool()
def list_repos_tool() -> dict:
    """List all registered repositories in the multi-repo registry.

    Returns the list of repos registered at ~/.dagayn/registry.json.
    Use the CLI 'register' command to add repos.
    """
    return _tool("list_repos_func")()


@mcp.tool()
def cross_repo_search_tool(
    query: str,
    kind: Optional[str] = None,
    limit: int = 20,
    model: Optional[str] = None,
    provider: Optional[str] = None,
) -> dict:
    """Search for code entities across all registered repositories.

    Runs hybrid search on each registered repo's graph database and merges
    the results by score. Register repos first with the CLI 'register' command.

    Args:
        query: Search string to match against node names.
        kind: Optional filter: File, Class, Function, Type, or Test.
        limit: Maximum results per repo. Default: 20.
        model: Embedding model for hybrid search. Defaults to the server's
            embedding model when configured by ``dagayn serve``.
        provider: Embedding provider for hybrid search. Defaults to the
            server's embedding provider when configured by ``dagayn serve``.
    """
    return _tool("cross_repo_search_func")(
        query=query,
        kind=kind,
        limit=limit,
        model=_resolve_embedding_model(model),
        provider=_resolve_embedding_provider(provider),
    )


@mcp.prompt()
def review_changes(base: str = "HEAD~1") -> list[dict]:
    """Pre-commit review workflow using review_tool, affected flows, and test gaps.

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


def _parse_tool_allow_list(raw: str) -> set[str]:
    """Parse a comma-separated MCP tool allow-list."""
    return {tool.strip() for tool in raw.split(",") if tool.strip()}


def _profile_tools(profile: str) -> set[str] | None:
    """Return the allow-list for a named profile, or None for full exposure."""
    profile_name = profile.strip()
    if not profile_name:
        return None
    if profile_name not in TOOL_PROFILES:
        known = ", ".join(TOOL_PROFILES)
        raise ValueError(f"unknown tool profile {profile_name!r}; expected one of: {known}")
    profile_tools = TOOL_PROFILES[profile_name]
    if profile_tools is None:
        return None
    return set(profile_tools)


def _resolve_tool_allow_list(
    tools: str | None = None,
    tool_profile: str | None = None,
) -> set[str] | None:
    """Resolve exact tool filtering from CLI/env args and named profiles."""
    import os

    if tools is not None:
        return _parse_tool_allow_list(tools) or None

    if tool_profile is not None:
        return _profile_tools(tool_profile)

    env_tools = os.environ.get("CRG_TOOLS")
    if env_tools is not None:
        return _parse_tool_allow_list(env_tools) or None

    profile = next(
        (os.environ[name] for name in TOOL_PROFILE_ENV_VARS if os.environ.get(name)),
        DEFAULT_TOOL_PROFILE,
    )
    return _profile_tools(profile)


def _apply_tool_filter(
    tools: str | None = None,
    tool_profile: str | None = None,
) -> None:
    """Remove tools not listed in the allow-list.

    Accepts either a comma-separated string of tool names to keep or a named
    profile.  Every registered MCP tool outside the resolved allow-list is
    removed via ``FastMCP.remove_tool()``.

    The allow-list can be supplied in these ways (first match wins):

    1. ``tools`` argument (from ``serve --tools ...``).
    2. ``tool_profile`` argument (from ``serve --tool-profile ...``).
    3. ``CRG_TOOLS`` environment variable.
    4. ``DAGAYN_TOOL_PROFILE`` or ``CRG_TOOL_PROFILE`` environment variable.
    5. The built-in ``default`` profile.

    Use the ``full`` profile to expose all tools, matching the legacy behavior.

    This is useful for token-constrained environments: dagayn exposes many
    tools when unfiltered.  Filtering to a workflow profile keeps the first
    choice small while preserving exact allow-lists for advanced users.

    Example::

        # via CLI
        dagayn serve --tool-profile review
        dagayn serve --tools query_graph_tool,semantic_search_nodes_tool

        # via env var
        CRG_TOOLS=query_graph_tool,semantic_search_nodes_tool
        DAGAYN_TOOL_PROFILE=architecture
    """

    allowed = _resolve_tool_allow_list(tools=tools, tool_profile=tool_profile)
    if not allowed:
        return
    registered = _registered_tool_names()
    for name in registered:
        if name not in allowed:
            mcp.remove_tool(name)


def main(
    repo_root: str | None = None,
    tools: str | None = None,
    tool_profile: str | None = None,
    embedding_provider: str | None = None,
    embedding_model: str | None = None,
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
            Falls back to ``CRG_TOOLS`` env var and overrides tool profiles.
        tool_profile: Named workflow profile. Defaults to ``default`` unless
            ``DAGAYN_TOOL_PROFILE`` or ``CRG_TOOL_PROFILE`` is set. Use
            ``full`` for legacy all-tools behavior.
        embedding_provider: Default embedding provider for MCP search when a
            client omits the provider argument.
        embedding_model: Default embedding model for MCP search when a client
            omits the model argument.
        transport: ``"stdio"`` (default) or ``"streamable-http"`` for local HTTP.
        host: Bind address when using HTTP (required for HTTP; set by CLI).
        port: Port when using HTTP (required for HTTP; set by CLI).
    """
    global _default_embedding_model, _default_embedding_provider, _default_repo_root
    _default_repo_root = repo_root
    _default_embedding_provider = embedding_provider or _infer_remote_embedding_provider_from_env()
    _default_embedding_model = embedding_model
    _apply_tool_filter(tools=tools, tool_profile=tool_profile)
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
