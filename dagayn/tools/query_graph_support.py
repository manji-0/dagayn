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
