"""Tools 2, 3, 5, 6, 9: query / search / stats helpers."""

from __future__ import annotations

import logging
from collections.abc import Mapping
from pathlib import Path
from typing import Any

from ..contracts.state_types import (
    MissingnessRecord,
    TraversalEntry,
    TraversalMode,
    seal_missingness_item,
    seal_reachability_info,
)
from ..graph import _sanitize_name
from ..hints import generate_hints, get_session
from ..search import embedding_health_available, hybrid_search
from ._common import (
    ToolStoreScope,
    _db_path_for_repo,
    _error_response,
    _get_store,
    graph_answerability_summary,
    guidance_actions_to_hints,
    handle_tool_runtime_error,
    is_sqlite_corrupt_error,
    make_guidance_item,
    make_response,
    missingness_from_answerability,
    recover_corrupt_graph,
    summary_at_verbose_only,
)
from ._native import native_tool
from .query_graph_support import exactness_action, result_evidence_type

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
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
                            "scores rank leads; fetch source_of for the chosen "
                            "qualified_name, then callers_of if needed"
                        ),
                    }
                ],
                action=(
                    'query_graph_tool pattern="source_of" -- fetch the chosen node\'s live span'
                ),
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


# ---------------------------------------------------------------------------
# Tool 3: query_graph
# ---------------------------------------------------------------------------


def query_graph(
    pattern: str,
    target: str,
    repo_root: str | None = None,
    detail_level: str = "standard",
    depth: int = 1,
    *,
    _corrupt_retried: bool = False,
) -> dict[str, Any]:
    """Run a predefined graph query.

    Args:
        pattern: Query pattern. One of: callers_of, callees_of, imports_of,
                 importers_of, docs_for, implementations_of, bridges_from, children_of,
                 tests_for, inheritors_of, file_summary, source_of.
        target: The node name, qualified name, or file path to query about.
        repo_root: Repository root path. Auto-detected if omitted.
        detail_level: "standard" (default): one row per related node, with
                      edge lines and confidence folded into the row.
                      "minimal": the same rows with fewer fields and no
                      guidance. "full": one row per edge plus ``edges``,
                      full ``answerability``, and ``_hints``.
        depth: Hops to follow for callers_of and importers_of (1 to 6). Rows
               past hop 1 carry ``depth`` and ``via``; ``reachability`` says
               whether the walk was complete or hit the depth limit.

    Returns:
        Matching nodes and edges for the query.
    """
    store = None
    try:
        # Resolves the repository and creates, migrates, or waits for the
        # graph; the Rust tool reads it.
        store, _root = _get_store(repo_root)
        return native_tool(
            "query_graph_tool",
            pattern=pattern,
            target=target,
            repo_root=repo_root,
            detail_level=detail_level,
            depth=depth,
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
                depth,
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


#: The most calls a ``next`` list names (``dagayn_tools::next::MAX_NEXT``).
_MAX_NEXT = 3


def _read_hits(results: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """``source_of`` for the first hits, in rank order, as
    ``dagayn_tools::next::read_hits`` builds them: a hit is a lead until its
    live span is read (docs/plans/AGENT-WORKFLOW-TARGET.md#target-contract)."""
    calls: list[dict[str, Any]] = []
    for hit in results:
        target = hit.get("qualified_name")
        if not isinstance(target, str):
            continue
        kind = hit.get("kind") if isinstance(hit.get("kind"), str) else "node"
        read = "file_summary" if kind == "File" else "source_of"
        calls.append(
            {
                # Keys in the order serde_json writes them.
                "args": {"pattern": read, "target": target},
                "tool": "query_graph_tool",
                "why": f"read the {kind} this search ranked",
            }
        )
        if len(calls) == _MAX_NEXT:
            break
    return calls


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
    with ToolStoreScope(
        logger=logger, context="semantic_search_nodes", repo_root=repo_root
    ) as scope:
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
        store, root = scope.track(_get_store(repo_root))
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

        minimal = detail_level == "minimal"
        if minimal:
            results = [
                {
                    **{
                        k: r[k]
                        for k in (
                            "name",
                            "kind",
                            "file_path",
                            "qualified_name",
                            "line_start",
                            "line_end",
                            "score",
                        )
                        if k in r
                    },
                    "evidence_type": result_evidence_type(r),
                }
                for r in results[:5]
            ]
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
            "next": _read_hits(results),
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
        if not hints["next_steps"]:
            # Minimal mode feeds the hint engine only the status and summary.
            hint_input = {"status": "ok", "summary": summary} if minimal else result
            hints = generate_hints("semantic_search_nodes", hint_input, get_session())
        result["_hints"] = hints
        return summary_at_verbose_only(result, detail_level)
    return scope.error


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
    with ToolStoreScope(logger=logger, context="list_graph_stats", repo_root=repo_root) as scope:
        # Resolves the repository and creates, migrates, or waits for the
        # graph; the Rust tool reads it.
        scope.track(_get_store(repo_root))
        return native_tool("list_graph_stats_tool", repo_root=repo_root)
    return scope.error


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
    with ToolStoreScope(
        logger=logger, context="find_large_functions", repo_root=repo_root
    ) as scope:
        scope.track(_get_store(repo_root))
        return native_tool(
            "find_large_functions_tool",
            min_lines=min_lines,
            kind=kind,
            file_path_pattern=file_path_pattern,
            limit=limit,
            repo_root=repo_root,
        )
    return scope.error


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
    with ToolStoreScope(logger=logger, context="traverse_graph", repo_root=repo_root) as scope:
        store, root = scope.track(_get_store(repo_root))
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
    return scope.error
