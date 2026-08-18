"""Community- and file-level graph aggregation for visualization."""

from __future__ import annotations

from collections import Counter, defaultdict
from typing import Any

type VisualizationRecord = dict[str, Any]
type VisualizationPayload = dict[str, Any]


def _aggregate_community(data: VisualizationPayload) -> VisualizationPayload:
    """Aggregate full graph data into community-level super-nodes.

    Each community becomes a single node sized by member count.
    Edges between super-nodes represent the count of cross-community edges.
    Returns a new dict with the same schema as *data* but fewer nodes/edges.
    Also returns per-community detail data for drill-down rendering.
    """
    communities = data.get("communities") or []
    nodes = data["nodes"]
    edges = data["edges"]

    # Build mapping: qualified_name -> community_id
    qn_to_cid: dict[str, int] = {}
    for c in communities:
        for qn in c.get("members", []):
            qn_to_cid[qn] = c["id"]

    # Also use node-level community_id for nodes not in community member lists
    for n in nodes:
        if n.get("community_id") is not None and n["qualified_name"] not in qn_to_cid:
            qn_to_cid[n["qualified_name"]] = n["community_id"]

    # Assign uncategorized nodes to a synthetic community id = -1
    uncategorized_members: list[str] = []
    for n in nodes:
        if n["qualified_name"] not in qn_to_cid:
            qn_to_cid[n["qualified_name"]] = -1
            uncategorized_members.append(n["qualified_name"])

    # Build community info map (including the synthetic uncategorized one)
    cid_info: dict[int, VisualizationRecord] = {}
    for c in communities:
        cid_info[c["id"]] = c
    if uncategorized_members:
        cid_info[-1] = {
            "id": -1,
            "name": "Uncategorized",
            "size": len(uncategorized_members),
            "members": uncategorized_members,
            "dominant_language": "",
            "description": "Nodes not assigned to any community",
            "cohesion": 0,
            "level": 0,
        }

    # Build super-nodes (one per community)
    super_nodes = []
    for cid, info in cid_info.items():
        size = info.get("size", len(info.get("members", [])))
        if size == 0:
            continue
        super_nodes.append(
            {
                "qualified_name": f"__community__{cid}",
                "name": info.get("name", f"Community {cid}"),
                "kind": "Community",
                "file_path": "",
                "line_start": None,
                "line_end": None,
                "language": info.get("dominant_language", ""),
                "community_id": cid,
                "member_count": size,
                "description": info.get("description", ""),
                "id": cid,
            }
        )

    # Build super-edges: aggregate cross-community edges
    cross_edge_counts: Counter[tuple[int, int]] = Counter()
    for e in edges:
        src_cid = qn_to_cid.get(e["source"])
        tgt_cid = qn_to_cid.get(e["target"])
        if src_cid is not None and tgt_cid is not None and src_cid != tgt_cid:
            pair = (min(src_cid, tgt_cid), max(src_cid, tgt_cid))
            cross_edge_counts[pair] += 1

    super_edges = []
    for (c1, c2), count in cross_edge_counts.items():
        super_edges.append(
            {
                "source": f"__community__{c1}",
                "target": f"__community__{c2}",
                "kind": "CROSS_COMMUNITY",
                "weight": count,
            }
        )

    # Build per-community detail data for drill-down
    community_details: dict[int, VisualizationPayload] = {}
    cid_members_set: dict[int, set[str]] = defaultdict(set)
    for qn, cid in qn_to_cid.items():
        cid_members_set[cid].add(qn)

    # One pass over nodes and one over edges, bucketed by community, instead of
    # a full scan of both per community: the nested form scaled 4x per 2x input
    # (0.35 s at 33.6k nodes / 400 communities, ~100x that on a 300k-node repo).
    detail_nodes_by_cid: dict[int, list[VisualizationRecord]] = defaultdict(list)
    for node in nodes:
        cid = qn_to_cid.get(node["qualified_name"])
        if cid is not None:
            detail_nodes_by_cid[cid].append(node)
    detail_edges_by_cid: dict[int, list[VisualizationRecord]] = defaultdict(list)
    for edge in edges:
        source_cid = qn_to_cid.get(edge["source"])
        if source_cid is not None and source_cid == qn_to_cid.get(edge["target"]):
            detail_edges_by_cid[source_cid].append(edge)

    for cid in cid_members_set:
        community_details[cid] = {
            "nodes": detail_nodes_by_cid.get(cid, []),
            "edges": detail_edges_by_cid.get(cid, []),
        }

    return {
        "nodes": super_nodes,
        "edges": super_edges,
        "stats": data["stats"],
        "flows": data.get("flows", []),
        "communities": communities,
        "mode": "community",
        "community_details": {str(k): v for k, v in community_details.items()},
    }


def _aggregate_file(data: VisualizationPayload) -> VisualizationPayload:
    """Aggregate full graph data into file-level nodes.

    Each file becomes a node sized by symbol count.
    Edges between files represent aggregated cross-file dependencies.
    """
    nodes = data["nodes"]
    edges = data["edges"]

    # Count symbols per file
    file_symbol_count: Counter[str] = Counter()
    qn_to_file: dict[str, str] = {}
    file_languages: dict[str, str] = {}

    for n in nodes:
        fp = n.get("file_path", "")
        if not fp:
            continue
        qn_to_file[n["qualified_name"]] = fp
        if n["kind"] != "File":
            file_symbol_count[fp] += 1
        else:
            file_symbol_count.setdefault(fp, 0)
        if n.get("language"):
            file_languages[fp] = n["language"]

    # Recover each file's community from the majority of its symbols, in one
    # pass. The previous form rescanned every node per file (quadratic) and,
    # despite the comment, took the *first* symbol with a community rather than
    # the majority.
    community_votes: dict[str, Counter[int]] = defaultdict(Counter)
    for n in nodes:
        fp = n.get("file_path", "")
        cid = n.get("community_id")
        if fp and cid is not None:
            community_votes[fp][cid] += 1
    file_community: dict[str, int] = {
        fp: votes.most_common(1)[0][0] for fp, votes in community_votes.items() if votes
    }

    # Build file nodes
    file_nodes = []
    for fp, count in file_symbol_count.items():
        parts = fp.replace("\\", "/").split("/")
        short = parts[-1] if parts else fp
        parent = parts[-2] if len(parts) >= 2 else ""
        label = f"{parent}/{short}" if parent else short
        cid = file_community.get(fp)
        file_nodes.append(
            {
                "qualified_name": fp,
                "name": label,
                "kind": "File",
                "file_path": fp,
                "line_start": None,
                "line_end": None,
                "language": file_languages.get(fp, ""),
                "community_id": cid,
                "symbol_count": count,
            }
        )

    # Aggregate cross-file edges
    cross_file_counts: Counter[tuple[str, str]] = Counter()
    for e in edges:
        src_fp = qn_to_file.get(e["source"])
        tgt_fp = qn_to_file.get(e["target"])
        if src_fp and tgt_fp and src_fp != tgt_fp:
            pair = (src_fp, tgt_fp)
            cross_file_counts[pair] += 1

    file_edges = []
    for (f1, f2), count in cross_file_counts.items():
        file_edges.append(
            {
                "source": f1,
                "target": f2,
                "kind": "DEPENDS_ON",
                "weight": count,
            }
        )

    return {
        "nodes": file_nodes,
        "edges": file_edges,
        "stats": data["stats"],
        "flows": data.get("flows", []),
        "communities": data.get("communities", []),
        "mode": "file",
    }
