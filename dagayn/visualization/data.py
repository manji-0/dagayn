"""Graph data export utilities."""

from __future__ import annotations

import logging
import sqlite3
from dataclasses import asdict

from ..graph import GraphStore, edge_to_dict, node_to_dict

logger = logging.getLogger(__name__)


def _build_name_index(nodes: list[dict], seen_qn: set[str]) -> dict[str, list[str]]:
    """Build a mapping from short/module-style names to qualified names.

    Returns ``{short_name: [qualified_name, ...]}``.
    """
    index: dict[str, list[str]] = {}

    def _add(key: str, qn: str) -> None:
        index.setdefault(key, []).append(qn)

    for n in nodes:
        qn = n["qualified_name"]
        _add(n["name"], qn)
        # Index by "file::name" suffix (e.g. "cli.py::main")
        if "::" in qn:
            _add(qn.rsplit("/", 1)[-1], qn)
        # Index by module-style path (e.g. "merit.cli" or "merit.cli.main")
        fp = n.get("file_path", "")
        if fp:
            mod = fp.replace("/", ".").replace(".py", "")
            if n["kind"] == "File":
                _add(mod, qn)
                # Index by every path suffix so C/C++ bare includes resolve.
                # e.g. "/abs/libs/trading/Foo.hpp" is also indexed as
                # "Foo.hpp", "trading/Foo.hpp", "libs/trading/Foo.hpp", …
                parts = fp.replace("\\", "/").split("/")
                for i in range(len(parts)):
                    suffix = "/".join(parts[i:])
                    if suffix:
                        _add(suffix, qn)
            else:
                _add(mod + "." + n["name"], qn)
    return index


def _resolve_target(
    target: str,
    source: str,
    seen_qn: set[str],
    name_index: dict[str, list[str]],
) -> str | None:
    """Try to resolve an unqualified edge target to a full qualified name.

    Returns the resolved qualified name, or None if unresolvable.
    """
    # Already fully qualified
    if target in seen_qn:
        return target

    candidates = name_index.get(target)
    if not candidates:
        return None

    if len(candidates) == 1:
        return candidates[0]

    # Disambiguate: prefer node in the same file as the source
    src_file = source.split("::")[0] if "::" in source else source
    same_file = [c for c in candidates if c.startswith(src_file)]
    if len(same_file) == 1:
        return same_file[0]

    # Prefer node in the same top-level directory
    src_parts = src_file.rsplit("/", 1)[0] if "/" in src_file else ""
    same_dir = [c for c in candidates if c.startswith(src_parts)]
    if len(same_dir) == 1:
        return same_dir[0]

    # Ambiguous — pick first match rather than dropping the edge
    return candidates[0]


def export_graph_data(store: GraphStore) -> dict:
    """Export all graph nodes and edges as a JSON-serializable dict.

    Returns ``{"nodes": [...], "edges": [...], "stats": {...},
    "flows": [...], "communities": [...]}``.
    """
    nodes = []
    seen_qn: set[str] = set()

    # Preload community_id mapping from DB (column may not exist in old schemas)
    community_map = store.get_all_community_ids()

    for gnode in store.get_all_nodes(exclude_files=False):
        if gnode.qualified_name in seen_qn:
            continue
        seen_qn.add(gnode.qualified_name)
        d = node_to_dict(gnode)
        d["params"] = gnode.params
        d["return_type"] = gnode.return_type
        d["community_id"] = community_map.get(gnode.qualified_name)
        nodes.append(d)

    name_index = _build_name_index(nodes, seen_qn)

    all_edges = [edge_to_dict(e) for e in store.get_all_edges()]

    # Resolve short/unqualified edge targets to full qualified names,
    # then drop edges that still can't be resolved (external/stdlib calls).
    edges = []
    for e in all_edges:
        src = _resolve_target(e["source"], e["source"], seen_qn, name_index)
        tgt = _resolve_target(e["target"], e["source"], seen_qn, name_index)
        if src and tgt:
            e["source"] = src
            e["target"] = tgt
            edges.append(e)

    stats = store.get_stats()

    # Include flows (graceful fallback if table doesn't exist)
    try:
        from dagayn.flows import get_flows

        flows = get_flows(store, limit=100)
    except (ImportError, sqlite3.OperationalError) as exc:
        logger.debug("flows unavailable for export: %s", exc)
        flows = []

    # Include communities (graceful fallback if table doesn't exist)
    try:
        from dagayn.communities import get_communities

        communities = get_communities(store)
    except (ImportError, sqlite3.OperationalError) as exc:
        logger.debug("communities unavailable for export: %s", exc)
        communities = []

    return {
        "nodes": nodes,
        "edges": edges,
        "stats": asdict(stats),
        "flows": flows,
        "communities": communities,
    }
