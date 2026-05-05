"""Tools 2, 3, 5, 6, 9: query / search / stats helpers."""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Any

from ..coverage import infer_tests_for_node
from ..embeddings import EmbeddingStore
from ..graph import _sanitize_name, edge_to_dict, node_to_dict
from ..hints import generate_hints, get_session
from ..incremental import get_changed_files, get_db_path, get_staged_and_unstaged
from ..search import hybrid_search
from ._common import _BUILTIN_CALL_NAMES, _get_store, apply_output_budget, make_response

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Tool 2: get_impact_radius
# ---------------------------------------------------------------------------

_QUERY_PATTERNS = {
    "callers_of": "Find all functions that call a given function",
    "callees_of": "Find all functions called by a given function",
    "imports_of": "Find all imports of a given file or module",
    "importers_of": "Find all files that import a given file or module",
    "children_of": "Find all nodes contained in a file or class",
    "tests_for": "Find all tests for a given function or class",
    "inheritors_of": "Find all classes that inherit from a given class",
    "file_summary": "Get a summary of all nodes in a file",
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
    store, root = _get_store(repo_root)
    try:
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
            }

        # Convert to absolute paths for graph lookup
        abs_files = [str(root / f) for f in changed_files]
        result = store.get_impact_radius(abs_files, max_depth=max_depth, max_nodes=max_results)

        changed_dicts = [node_to_dict(n) for n in result["changed_nodes"]]
        impacted_dicts = [node_to_dict(n) for n in result["impacted_nodes"]]
        edge_dicts = [edge_to_dict(e) for e in result["edges"]]
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
    finally:
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
                 importers_of, children_of, tests_for, inheritors_of, file_summary.
        target: The node name, qualified name, or file path to query about.
        repo_root: Repository root path. Auto-detected if omitted.
        detail_level: "standard" (full output) or "minimal" (summary only).

    Returns:
        Matching nodes and edges for the query.
    """
    store, root = _get_store(repo_root)
    try:
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
                }

        if not node and pattern != "file_summary":
            return {
                "status": "not_found",
                "summary": f"No node found matching '{target}'.",
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

        elif pattern == "children_of":
            child_edges = [e for e in store.get_edges_by_source(qn) if e.kind == "CONTAINS"]
            results.extend(
                _node_dicts_for_edges(store, child_edges, qualified_attr="target_qualified")
            )

        elif pattern == "tests_for":
            if node is not None:
                results.extend(infer_tests_for_node(store, node))
                test_edges = [
                    e for e in store.get_edges_by_target(qn) if e.kind == "TESTED_BY"
                ]
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
                    if k in r
                }
                for r in results[:5]
            ]
            return {
                "status": "ok",
                "pattern": pattern,
                "target": target,
                "description": _QUERY_PATTERNS[pattern],
                "summary": summary,
                "result_count": len(results),
                "results": minimal_results,
            }

        return {
            "status": "ok",
            "pattern": pattern,
            "target": target,
            "description": _QUERY_PATTERNS[pattern],
            "summary": summary,
            "results": results,
            "edges": edges_out,
        }
    finally:
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
    store, root = _get_store(repo_root)
    try:
        results = hybrid_search(
            store,
            query,
            kind=kind,
            limit=limit,
            context_files=context_files,
            model=model,
            provider=provider,
        )

        search_mode = "hybrid"
        if not results:
            search_mode = "keyword"

        summary = f"Found {len(results)} node(s) matching '{query}'" + (
            f" (kind={kind})" if kind else ""
        )

        if detail_level == "minimal":
            minimal_results = [
                {k: r[k] for k in ("name", "kind", "file_path", "score") if k in r}
                for r in results[:5]
            ]
            return {
                "status": "ok",
                "query": query,
                "search_mode": search_mode,
                "summary": summary,
                "results": minimal_results,
            }

        result: dict[str, object] = {
            "status": "ok",
            "query": query,
            "search_mode": search_mode,
            "summary": summary,
            "results": results,
        }
        result["_hints"] = generate_hints("semantic_search_nodes", result, get_session())
        return result
    finally:
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
    store, root = _get_store(repo_root)
    try:
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
                "list_communities_tool -- inspect the high-level code structure",
                "list_flows_tool -- inspect critical execution paths",
                "semantic_search_nodes_tool -- search for specific entities",
            ],
        )
    finally:
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
    store, root = _get_store(repo_root)
    try:
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
    finally:
        store.close()


# -------------------------------------------------------------------
# traverse_graph: free-form BFS / DFS traversal
# -------------------------------------------------------------------


def traverse_graph_func(
    query: str,
    mode: str = "bfs",
    depth: int = 3,
    token_budget: int = 2000,
    repo_root: str | None = None,
) -> dict[str, Any]:
    """BFS/DFS traversal from best-matching node.

    Args:
        query: Search string to find the starting node.
        mode: "bfs" (breadth-first) or "dfs" (depth-first).
        depth: Max traversal depth (1-6). Default: 3.
        token_budget: Approximate token limit for results.
        repo_root: Repository root path.
    """
    store, root = _get_store(repo_root)
    try:
        results = hybrid_search(store, query, limit=1)
        if not results:
            return make_response(
                "not_found",
                f"No node matching '{query}'.",
                start_node=None,
                mode=mode,
                max_depth=max(1, min(depth, 6)),
                nodes_visited=0,
                traversal=[],
                truncated=False,
                next_tool_suggestions=[
                    "semantic_search_nodes_tool -- search more broadly for the symbol",
                    "query_graph_tool -- inspect a known qualified name directly",
                ],
            )

        start_qn = results[0]["qualified_name"]
        depth = max(1, min(depth, 6))

        # Traversal state shared by both modes.
        visited: dict[str, int] = {}
        traversal: list[dict] = []
        approx_tokens = 0
        budget_exceeded = False

        def _make_entry(node: Any, cur_depth: int) -> dict:
            return {
                "name": _sanitize_name(node.name),
                "qualified_name": node.qualified_name,
                "kind": node.kind,
                "file": node.file_path,
                "depth": cur_depth,
            }

        if mode == "dfs":
            # Pre-fetch the entire local subgraph in 3 SQL queries
            # (CTE + nodes batch + edges batch) instead of 2 queries per
            # visited node.  DFS order is preserved via the in-memory adj.
            nodes_map, adj = store.get_local_subgraph(start_qn, depth)
            stack: list[tuple[str, int]] = [(start_qn, 0)]
            while stack and not budget_exceeded:
                current_qn, cur_depth = stack.pop()
                if current_qn in visited or cur_depth > depth:
                    continue

                node = nodes_map.get(current_qn)
                if not node:
                    visited[current_qn] = cur_depth
                    continue

                visited[current_qn] = cur_depth
                entry = _make_entry(node, cur_depth)
                approx_tokens += (
                    len(entry["qualified_name"]) + len(entry["file"]) + len(entry["name"]) + 30
                ) // 4
                if approx_tokens > token_budget:
                    budget_exceeded = True
                    break
                traversal.append(entry)

                if cur_depth + 1 > depth:
                    continue
                neighbors = [nb for nb in adj.get(current_qn, []) if nb not in visited]
                # Reverse-push so the first neighbour is popped next,
                # giving stable left-to-right depth-first order.
                for nb in reversed(neighbors):
                    stack.append((nb, cur_depth + 1))
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
                    approx_tokens += (
                        len(entry["qualified_name"]) + len(entry["file"]) + len(entry["name"]) + 30
                    ) // 4
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
            next_tool_suggestions=[
                "query_graph callers_of -- focused relationship query",
                "get_impact_radius -- blast radius analysis",
            ],
        )
    finally:
        store.close()
