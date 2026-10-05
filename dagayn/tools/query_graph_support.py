"""Result helpers ``semantic_search_nodes`` shares with the Rust ``query_graph``."""

from __future__ import annotations

from typing import Any


def result_evidence_type(result: dict[str, Any]) -> str:
    if result.get("evidence_type"):
        return str(result["evidence_type"])
    kind = str(result.get("kind", ""))
    file_path = str(result.get("file_path") or result.get("file") or "")
    if kind.startswith("Doc") or file_path.lower().endswith((".md", ".markdown", ".mdx")):
        return "authored"
    return "extracted"


def exactness_action(query: str, exact_count: int, result_count: int) -> dict[str, Any]:
    if exact_count == 1:
        return {
            "tool": "query_graph_tool",
            "suggestion": (
                'fetch live source with pattern="source_of", then callers_of/callees_of'
            ),
        }
    if exact_count > 1:
        return {
            "tool": "semantic_search_nodes_tool",
            "suggestion": f"choose one qualified name before querying relationships for '{query}'",
        }
    if result_count:
        return {
            "tool": "query_graph_tool",
            "suggestion": (
                'fetch live source with pattern="source_of" for the chosen qualified_name'
            ),
        }
    return {
        "tool": "semantic_search_nodes_tool",
        "suggestion": "broaden the query or verify the graph is up to date",
    }
