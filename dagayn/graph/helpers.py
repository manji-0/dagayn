"""Standalone helper functions for graph data serialization."""
from __future__ import annotations

from .types import GraphEdge, GraphNode


def _sanitize_name(s: str, max_len: int = 256) -> str:
    """Strip ASCII control characters and truncate to prevent prompt injection.

    Node names extracted from source code could contain adversarial strings
    (e.g. ``IGNORE_ALL_PREVIOUS_INSTRUCTIONS``).  This function removes control
    characters (0x00-0x1F except tab and newline) and enforces a length limit so
    that names flowing through MCP tool responses cannot easily influence AI
    agent behaviour.
    """
    # Strip control chars 0x00-0x1F except \t (0x09) and \n (0x0A)
    cleaned = "".join(ch for ch in s if ch in ("\t", "\n") or ord(ch) >= 0x20)
    return cleaned[:max_len]


def node_to_dict(n: GraphNode) -> dict:
    return {
        "id": n.id,
        "kind": n.kind,
        "name": _sanitize_name(n.name),
        "qualified_name": _sanitize_name(n.qualified_name),
        "file_path": n.file_path,
        "line_start": n.line_start,
        "line_end": n.line_end,
        "language": n.language,
        "parent_name": _sanitize_name(n.parent_name) if n.parent_name else n.parent_name,
        "is_test": n.is_test,
    }


def edge_to_dict(e: GraphEdge) -> dict:
    return {
        "id": e.id,
        "kind": e.kind,
        "source": _sanitize_name(e.source_qualified),
        "target": _sanitize_name(e.target_qualified),
        "file_path": e.file_path,
        "line": e.line,
        "confidence": e.confidence,
        "confidence_tier": e.confidence_tier,
    }
