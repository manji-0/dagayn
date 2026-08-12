"""Review context MCP tool implementation."""

from __future__ import annotations

import logging
from typing import Any

from ..graph import edge_to_dict, node_to_dict
from ..incremental import get_changed_file_sources, get_staged_and_unstaged
from ._common import (
    _get_store,
    graph_answerability_summary,
    handle_tool_runtime_error,
    missingness_from_answerability,
    resolve_contained_path,
)
from .review_helpers import (
    _is_low_confidence_unresolved_markdown_code_span,
    _relative_qualified_name,
)

logger = logging.getLogger(__name__)


def get_review_context(
    changed_files: list[str] | None = None,
    max_depth: int = 2,
    include_source: bool = True,
    max_lines_per_file: int = 200,
    repo_root: str | None = None,
    base: str = "HEAD~1",
    detail_level: str = "standard",
) -> dict[str, Any]:
    """Generate a focused review context from changed files.

    Builds a token-optimized subgraph + source snippets for code review.

    Args:
        changed_files: Files to review (auto-detected from git diff if omitted).
        max_depth: Impact radius depth (default: 2).
        include_source: Whether to include source code snippets (default: True).
        max_lines_per_file: Max source lines per file in output (default: 200).
        repo_root: Repository root path. Auto-detected if omitted.
        base: Git ref for change detection (default: HEAD~1).
        detail_level: Output detail level.  "standard" returns full context;
            "minimal" returns summary, risk level, changed/impacted file counts,
            top 5 key entity names, test gap count, and next tool suggestions.
            Default: "standard".

    Returns:
        Structured review context with subgraph, source snippets, and
        review guidance.
    """
    store = None
    try:
        store, root = _get_store(repo_root)
        answerability = graph_answerability_summary(store)
        missingness = missingness_from_answerability(answerability)
        # Get impact radius first
        change_file_sources: dict[str, list[str]]
        if changed_files is None:
            change_file_sources = get_changed_file_sources(root, base)
            changed_files = change_file_sources["files"]
            if not changed_files:
                changed_files = get_staged_and_unstaged(root)
                change_file_sources = {"files": changed_files, "worktree": changed_files}
        else:
            change_file_sources = {"files": changed_files, "explicit": changed_files}

        if not changed_files:
            return {
                "status": "ok",
                "summary": "No changes detected. Nothing to review.",
                "context": {},
                "answerability": answerability,
                "missingness": missingness,
            }

        abs_files = [str(root / f) for f in changed_files]
        impact = store.get_impact_radius(abs_files, max_depth=max_depth)

        if detail_level == "minimal":
            impacted_count = len(impact["impacted_nodes"])
            if impacted_count > 20:
                risk = "high"
            elif impacted_count > 5:
                risk = "medium"
            else:
                risk = "low"

            key_entities = [
                _relative_qualified_name(n.qualified_name, root)
                for n in impact["changed_nodes"][:5]
            ]

            # Count test gaps among changed functions.
            changed_funcs = [
                n for n in impact["changed_nodes"] if n.kind == "Function" and not n.is_test
            ]
            test_edges = [e for e in impact["edges"] if e.kind == "TESTED_BY"]
            tested_qualified = {e.source_qualified for e in test_edges}
            test_gap_count = sum(
                1 for f in changed_funcs if f.qualified_name not in tested_qualified
            )

            summary_parts = [
                f"Review context for {len(changed_files)} changed file(s):",
                f"  - Risk: {risk}",
                f"  - {len(impact['impacted_nodes'])} impacted nodes"
                f" in {len(impact['impacted_files'])} files",
            ]

            return {
                "status": "ok",
                "summary": "\n".join(summary_parts),
                "risk": risk,
                "changed_file_count": len(changed_files),
                "change_file_sources": change_file_sources,
                "impacted_file_count": len(impact["impacted_files"]),
                "key_entities": key_entities,
                "test_gaps": test_gap_count,
                "answerability": answerability,
                "missingness": missingness,
                "next_tool_suggestions": [
                    'review_tool mode="changes"',
                    'review_tool mode="affected_flows"',
                    'review_tool mode="impact"',
                ],
            }

        # Build review context
        context: dict[str, Any] = {
            "changed_files": changed_files,
            "change_file_sources": change_file_sources,
            "impacted_files": impact["impacted_files"],
            "graph": {
                "changed_nodes": [node_to_dict(n) for n in impact["changed_nodes"]],
                "impacted_nodes": [node_to_dict(n) for n in impact["impacted_nodes"]],
                "edges": [
                    edge_to_dict(e)
                    for e in impact["edges"]
                    if not _is_low_confidence_unresolved_markdown_code_span(e)
                ],
            },
        }

        # Add source snippets for changed files
        if include_source:
            snippets = {}
            out_of_repo: list[str] = []
            for rel_path in changed_files:
                full_path = resolve_contained_path(rel_path, root)
                if full_path is None:
                    # An absolute or ``..``-escaping entry would otherwise be
                    # read and returned verbatim: ``changed_files`` is
                    # caller-controlled over MCP.
                    out_of_repo.append(rel_path)
                    continue
                if full_path.is_file():
                    try:
                        lines = full_path.read_text(errors="replace").splitlines()
                        if len(lines) > max_lines_per_file:
                            # Include only the relevant functions/classes
                            relevant_lines = _extract_relevant_lines(
                                lines,
                                impact["changed_nodes"],
                                rel_path,
                            )
                            snippets[rel_path] = relevant_lines
                        else:
                            snippets[rel_path] = "\n".join(
                                f"{i + 1}: {line}" for i, line in enumerate(lines)
                            )
                    except (OSError, UnicodeDecodeError):
                        snippets[rel_path] = "(could not read file)"
            context["source_snippets"] = snippets
            if out_of_repo:
                context["out_of_repo_files"] = out_of_repo

        # Generate review guidance
        guidance = _generate_review_guidance(impact, changed_files)
        context["review_guidance"] = guidance

        summary_parts = [
            f"Review context for {len(changed_files)} changed file(s):",
            f"  - {len(impact['changed_nodes'])} directly changed nodes",
            f"  - {len(impact['impacted_nodes'])} impacted nodes"
            f" in {len(impact['impacted_files'])} files",
            "",
            "Review guidance:",
            guidance,
        ]

        return {
            "status": "ok",
            "summary": "\n".join(summary_parts),
            "context": context,
            "answerability": answerability,
            "missingness": missingness,
        }
    except Exception as exc:
        return handle_tool_runtime_error(exc, logger=logger, context="get_review_context")
    finally:
        if store is not None:
            store.close()


def _extract_relevant_lines(lines: list[str], nodes: list, file_path: str) -> str:
    """Extract only the lines relevant to changed nodes."""
    ranges = []
    for n in nodes:
        if n.file_path == file_path:
            start = max(0, n.line_start - 3)  # 2 lines context before
            end = min(len(lines), n.line_end + 2)  # 1 line context after
            ranges.append((start, end))

    if not ranges:
        # Show first N lines as fallback
        return "\n".join(f"{i + 1}: {line}" for i, line in enumerate(lines[:50]))

    # Merge overlapping ranges
    ranges.sort()
    merged = [ranges[0]]
    for start, end in ranges[1:]:
        if start <= merged[-1][1] + 1:
            merged[-1] = (merged[-1][0], max(merged[-1][1], end))
        else:
            merged.append((start, end))

    parts: list[str] = []
    for start, end in merged:
        if parts:
            parts.append("...")
        for i in range(start, end):
            parts.append(f"{i + 1}: {lines[i]}")

    return "\n".join(parts)


def _generate_review_guidance(impact: dict, changed_files: list[str]) -> str:
    """Generate review guidance based on the impact analysis."""
    guidance_parts = []

    # Check for test coverage
    changed_funcs = [n for n in impact["changed_nodes"] if n.kind == "Function"]
    test_edges = [e for e in impact["edges"] if e.kind == "TESTED_BY"]
    tested_funcs = {e.source_qualified for e in test_edges}

    untested = [f for f in changed_funcs if f.qualified_name not in tested_funcs and not f.is_test]
    if untested:
        guidance_parts.append(
            f"- {len(untested)} changed function(s) lack test coverage: "
            + ", ".join(n.name for n in untested[:5])
        )

    # Check for wide blast radius
    if len(impact["impacted_nodes"]) > 20:
        guidance_parts.append(
            f"- Wide blast radius: {len(impact['impacted_nodes'])} "
            "nodes impacted. "
            "Review callers and dependents carefully."
        )

    # Check for inheritance changes
    inheritance_edges = [e for e in impact["edges"] if e.kind in ("INHERITS", "IMPLEMENTS")]
    if inheritance_edges:
        guidance_parts.append(
            f"- {len(inheritance_edges)} inheritance/implementation "
            "relationship(s) affected. "
            "Check for Liskov substitution violations."
        )

    # Check for cross-file impact
    impacted_file_count = len(impact["impacted_files"])
    if impacted_file_count > 3:
        guidance_parts.append(
            f"- Changes impact {impacted_file_count} other files."
            " Consider splitting into smaller PRs."
        )

    if not guidance_parts:
        guidance_parts.append("- Changes appear well-contained with minimal blast radius.")

    return "\n".join(guidance_parts)
