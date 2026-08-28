"""Shared helpers for ``query_graph`` pattern execution."""

from __future__ import annotations

from pathlib import Path
from typing import Any, Callable, cast

from ..bare_name_resolution import (
    SymbolVisibility,
    build_import_targets,
    build_symbol_visibility,
    is_plausible_bare_edge,
    looks_like_file_target,
    node_file_from_qualified,
)
from ..cross_artifact import is_low_confidence_unresolved_markdown_code_span
from ..graph import node_to_dict
from ._common import make_guidance_item

QUERY_PATTERNS = {
    "callers_of": "Find all functions that call a given function",
    "callees_of": "Find all functions called by a given function",
    "imports_of": "Find all imports of a given file or module",
    "importers_of": "Find all files that import a given file or module",
    "docs_for": "Find documentation linked to a code, Terraform, or artifact node",
    "implementations_of": "Find implementation artifacts linked to a document node",
    "bridges_from": (
        "Find high-confidence CROSS_ARTIFACT bridges from a node "
        "(Terraform maps_entrypoint / invokes_binary, and similar)"
    ),
    "children_of": "Find all nodes contained in a file or class",
    "tests_for": "Find all tests for a given function or class",
    "inheritors_of": "Find all classes that inherit from a given class",
    "file_summary": "Get a summary of all nodes in a file",
}

_DOC_TO_ARTIFACT_ROLES = {
    "implemented_by": "implements_contract",
    "describes_symbol": "described_by",
    "discusses_artifact": "discussed_by",
    "raises_issue_for": "has_issue_note",
}

_ARTIFACT_TO_DOC_ROLES = {
    "implements_contract": "implemented_by",
    "explained_by": "explains",
    "has_runbook": "runbook_for",
    "problem_described_by": "describes_problem_in",
    "discussed_by": "discusses",
}

_INFRA_TO_CODE_BRIDGE_ROLES = {
    "maps_entrypoint": "entrypoint_for",
    "invokes_binary": "invoked_by",
}


def merge_unresolved_targets(
    accumulated: list[str],
    new_targets: list[str],
) -> list[str]:
    seen = set(accumulated)
    for target in new_targets:
        if target not in seen:
            accumulated.append(target)
            seen.add(target)
    return accumulated


def node_dicts_for_edges(
    store: Any,
    edges: list[Any],
    *,
    qualified_attr: str,
) -> tuple[list[dict[str, Any]], list[str]]:
    """Batch-resolve edge endpoints to node dicts while preserving edge order."""
    if not edges:
        return [], []

    qualified_names = [getattr(edge, qualified_attr) for edge in edges]
    nodes_by_qn = store.get_nodes_by_qualified_names(qualified_names)
    results: list[dict[str, Any]] = []
    unresolved_targets: list[str] = []
    seen_unresolved: set[str] = set()
    for edge in edges:
        qn = getattr(edge, qualified_attr)
        node = nodes_by_qn.get(qn)
        if node is not None:
            results.append(node_to_dict(node))
        elif qn not in seen_unresolved:
            unresolved_targets.append(qn)
            seen_unresolved.add(qn)
    return results, unresolved_targets


def is_unresolved_import_target(store: Any, target: str, root: Path) -> bool:
    if target.startswith("<unresolved:"):
        return True
    abs_target = (
        str((root / target).resolve())
        if not Path(target).is_absolute()
        else str(Path(target).resolve())
    )
    return len(store.get_nodes_by_file(abs_target)) == 0


def query_zero_result_fields(
    *,
    results: list[dict],
    unresolved_targets: list[str],
) -> dict[str, Any]:
    if results:
        return {
            "confidence": "medium",
            "zero_result_reason": None,
        }
    if unresolved_targets:
        return {
            "confidence": "medium",
            "zero_result_reason": "unresolved_endpoints_only",
        }
    return {
        "confidence": "low",
        "zero_result_reason": "not_found_in_current_graph",
    }


def cross_artifact_role(edge: Any) -> str | None:
    if edge.kind != "CROSS_ARTIFACT":
        return None
    extra = edge.extra if isinstance(edge.extra, dict) else {}
    role = extra.get("relationship_role")
    return role if isinstance(role, str) else None


def is_low_confidence_markdown_code_span(edge: Any) -> bool:
    return is_low_confidence_unresolved_markdown_code_span(edge)


def documentation_result(edge: Any, *, endpoint: str, inverse_label: str | None = None) -> dict:
    role = cross_artifact_role(edge)
    confidence_tier = str(edge.confidence_tier or "").upper()
    evidence_type = "authored" if role in {"implements_contract", "implemented_by"} else "extracted"
    if role not in {"implements_contract", "implemented_by"} and confidence_tier not in {
        "EXTRACTED",
        "HIGH",
    }:
        evidence_type = "heuristic_reachable"
    result = {
        "source": edge.source_qualified,
        "target": edge.target_qualified,
        "matched_endpoint": endpoint,
        "relationship_role": role,
        "evidence_type": evidence_type,
        "file": edge.file_path,
        "line": edge.line,
        "confidence": edge.confidence,
        "confidence_tier": edge.confidence_tier,
    }
    if inverse_label:
        result["inverse_label"] = inverse_label
    return result


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
            "suggestion": "inspect callers_of/callees_of for the exact qualified match",
        }
    if exact_count > 1:
        return {
            "tool": "semantic_search_nodes_tool",
            "suggestion": f"choose one qualified name before querying relationships for '{query}'",
        }
    if result_count:
        return {
            "tool": "query_graph_tool",
            "suggestion": "confirm the best candidate with file_summary or callers_of",
        }
    return {
        "tool": "semantic_search_nodes_tool",
        "suggestion": "broaden the query or verify the graph is up to date",
    }


def filter_bare_name_fallback_edges(
    store: Any,
    edges: list[Any],
    target_node: Any,
) -> list[Any]:
    """Keep bare-name fallback edges only when import context supports them.

    A globally unique symbol name is unambiguous on its own, so it needs no
    import evidence. Languages whose imports name modules rather than files
    (C# ``using``, Java ``import``) never produce file-to-file IMPORTS_FROM
    edges, and would otherwise lose every cross-file caller. Dead-code
    analysis already applies the same unique-name rule.
    """
    if not edges or target_node is None:
        return edges

    if _bare_name_is_unique(store, getattr(target_node, "name", "")):
        return edges

    native = getattr(store, "import_targets_by_file", None)
    if callable(native):
        native_map = cast(Callable[[], dict[str, list[str]]], native)()
        import_targets = {file_path: set(targets) for file_path, targets in native_map.items()}
        visibility = _native_symbol_visibility(store)
    elif hasattr(store, "_conn"):
        import_targets = build_import_targets(store._conn)
        visibility = build_symbol_visibility(store._conn)
    else:
        return edges

    target_file = target_node.file_path
    target_qualified = getattr(target_node, "qualified_name", "") or ""
    return [
        edge
        for edge in edges
        if is_plausible_bare_edge(
            node_file_from_qualified(edge.source_qualified, edge.file_path),
            target_file,
            import_targets,
            visibility,
            target_qualified,
        )
    ]


def _native_symbol_visibility(store: Any) -> SymbolVisibility | None:
    """Read the visibility index from the Rust backend, if it exposes one."""
    reader = getattr(store, "symbol_visibility_by_file", None)
    if not callable(reader):
        return None
    declared, imported, class_files = cast(
        Callable[
            [],
            tuple[dict[str, list[str]], dict[str, list[str]], dict[str, list[str]]],
        ],
        reader,
    )()
    return SymbolVisibility(
        declared={file_path: set(values) for file_path, values in declared.items()},
        imported={file_path: set(values) for file_path, values in imported.items()},
        class_files={name: set(values) for name, values in class_files.items()},
    )


def _bare_name_is_unique(store: Any, name: str) -> bool:
    """True when exactly one Function/Class in the graph carries *name*."""
    if not name:
        return False
    counter = getattr(store, "count_nodes_by_name", None)
    if not callable(counter):
        return False
    counts = cast(Callable[..., dict[str, int]], counter)(["Function", "Class"])
    return counts.get(name, 0) == 1


def annotate_bare_name_edges(edges_out: list[dict[str, Any]]) -> None:
    for edge in edges_out:
        if "::" not in str(edge.get("target", "")):
            edge["match"] = "bare_name"
            edge["confidence_tier"] = "MEDIUM"
            edge["confidence"] = 0.6


def file_path_candidates(root: Path, target: str) -> list[str]:
    """Return absolute file paths to try for file_summary lookups."""
    candidates = [str(root / target)]
    resolved = (root / target).resolve()
    candidates.append(str(resolved))
    if target.startswith("/"):
        candidates.append(target)
    seen: set[str] = set()
    ordered: list[str] = []
    for path in candidates:
        if path not in seen:
            seen.add(path)
            ordered.append(path)
    return ordered


def file_is_indexed(store: Any, root: Path, target: str) -> bool:
    for abs_path in file_path_candidates(root, target):
        if store.get_nodes_by_file(abs_path):
            return True
        if store.get_node(abs_path) is not None:
            return True
    return False


def query_graph_guidance(
    *,
    pattern: str,
    target: str,
    result_count: int,
    exact_count: int,
) -> list[dict[str, Any]]:
    if result_count:
        return [
            make_guidance_item(
                claim=(
                    f"Graph query '{pattern}' returned {result_count} related node(s) "
                    f"for '{target}'."
                ),
                evidence={
                    "type": "computed",
                    "pattern": pattern,
                    "target": target,
                    "result_count": result_count,
                    "exact_match_count": exact_count,
                },
                confidence="medium",
                missingness=[
                    {
                        "reason_code": "relationship_query_not_runtime_proof",
                        "severity": "low",
                        "claim_effect": ("graph edges are static extraction, not runtime traces"),
                    }
                ],
                action=f'query_graph_tool pattern="{pattern}" -- drill into a relationship',
                reason_codes=["graph_relationship_query"],
                counts={"result_count": result_count},
            )
        ]
    return [
        make_guidance_item(
            claim=f"No graph relationships matched '{pattern}' for '{target}'.",
            evidence={
                "type": "computed",
                "pattern": pattern,
                "target": target,
                "result_count": 0,
            },
            confidence="low",
            missingness=[
                {
                    "reason_code": "not_found_in_current_graph",
                    "severity": "medium",
                    "claim_effect": (
                        "absence is graph-limited, not proof the relationship does not exist"
                    ),
                }
            ],
            action="semantic_search_nodes_tool -- verify the target and refresh the graph",
            reason_codes=["zero_result"],
            counts={"result_count": 0},
        )
    ]


def looks_like_query_file_target(target: str) -> bool:
    return looks_like_file_target(target)
