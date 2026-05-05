"""Heuristic test-coverage helpers used by graph query and review tools."""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any

from .graph import GraphNode, node_to_dict

_TEST_FILE_PARTS = ("/tests/", "/test/", "/__tests__/")
_TEST_FILE_SUFFIXES = (
    "_test.py",
    "_tests.py",
    ".test.js",
    ".test.ts",
    ".test.tsx",
    ".spec.js",
    ".spec.ts",
    ".spec.tsx",
    "_test.rs",
    "_tests.rs",
)
_NON_TEST_HELPER_NAMES = {
    "setup",
    "teardown",
    "setup_method",
    "teardown_method",
    "setup_class",
    "teardown_class",
    "setup_module",
    "teardown_module",
    "setUp".casefold(),
    "tearDown".casefold(),
}


def is_test_file_path(file_path: str) -> bool:
    """Return whether *file_path* looks like a test artifact path."""
    normalized = file_path.replace("\\", "/")
    name = normalized.rsplit("/", 1)[-1]
    return (
        any(part in normalized for part in _TEST_FILE_PARTS)
        or name.startswith("test_")
        or name in {"tests.rs", "test.rs"}
        or name.endswith(_TEST_FILE_SUFFIXES)
    )


def _identifier_tokens(value: str) -> list[str]:
    tokens = re.findall(r"[A-Za-z0-9]+", value.casefold())
    return [token for token in tokens if token and token not in {"test", "tests"}]


def _squashed_identifier(value: str) -> str:
    return "".join(_identifier_tokens(value))


def _is_test_like_node(node: GraphNode) -> bool:
    name = node.name.casefold()
    qn = node.qualified_name.casefold()
    if node.kind == "Function" and not node.is_test and name in _NON_TEST_HELPER_NAMES:
        return False
    if node.kind == "Function" and not node.is_test:
        return (
            name.startswith(("test_", "test"))
            or "::test" in qn
            or ".test." in qn
            or ".spec." in qn
        )
    return (
        bool(node.is_test)
        or node.kind == "Test"
        or name.startswith(("test_", "test"))
        or ".test." in qn
        or "::test" in qn
        or is_test_file_path(node.file_path)
    )


def _load_source_lines(store: Any, file_path: str) -> list[str]:
    try:
        path = store.resolve_file_path(file_path)
    except (AttributeError, TypeError):
        path = Path(file_path)
    try:
        return path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []


def _span_text(store: Any, node: GraphNode) -> str:
    lines = _load_source_lines(store, node.file_path)
    if not lines or node.line_start <= 0 or node.line_end < node.line_start:
        return ""
    return "\n".join(lines[node.line_start - 1 : min(node.line_end, len(lines))]).casefold()


def _coverage_record(
    node: GraphNode,
    *,
    confidence: str,
    evidence: list[str],
    source: str,
) -> dict[str, Any]:
    return {
        **node_to_dict(node),
        "confidence": confidence,
        "evidence": evidence,
        "coverage_source": source,
    }


def _candidate_score(
    store: Any,
    target: GraphNode,
    candidate: GraphNode,
    target_tokens: list[str],
    target_squashed: str,
) -> tuple[int, str, list[str]]:
    evidence: list[str] = []
    candidate_identity = f"{candidate.qualified_name} {candidate.name}".casefold()
    candidate_squashed = _squashed_identifier(candidate_identity)

    if target_squashed and target_squashed in candidate_squashed:
        evidence.append("test node name references target symbol")
        return 80, "medium", evidence

    if target_tokens and all(token in candidate_identity for token in target_tokens):
        evidence.append("test node qualified name contains target tokens")
        return 70, "medium", evidence

    span = _span_text(store, candidate)
    if span and target.name.casefold() in span:
        evidence.append("test source references target symbol")
        return 65, "medium", evidence

    target_file_stem = Path(target.file_path).stem.casefold()
    if target_file_stem and target_file_stem in candidate_identity:
        evidence.append("test node name references target file stem")
        return 35, "low", evidence

    return 0, "low", evidence


def infer_tests_for_node(
    store: Any,
    target: GraphNode,
    *,
    limit: int = 25,
    minimum_confidence: str = "medium",
) -> list[dict[str, Any]]:
    """Infer tests for *target* from graph edges, names, and local test source.

    Direct ``TESTED_BY`` edges are treated as high-confidence facts. Naming and
    source-reference matches are marked as heuristic evidence so callers can
    distinguish strong coverage from useful leads.
    """
    min_rank = {"low": 0, "medium": 1, "high": 2}.get(minimum_confidence, 0)
    confidence_rank = {"low": 0, "medium": 1, "high": 2}
    results: dict[str, tuple[int, dict[str, Any]]] = {}

    direct_edges = [
        edge
        for edge in store.get_edges_by_target(target.qualified_name)
        if edge.kind == "TESTED_BY"
    ]
    direct_sources = [edge.source_qualified for edge in direct_edges]
    direct_nodes = store.get_nodes_by_qualified_names(direct_sources)
    for edge in direct_edges:
        test_node = direct_nodes.get(edge.source_qualified)
        if test_node is None:
            continue
        record = _coverage_record(
            test_node,
            confidence="high",
            evidence=["TESTED_BY edge"],
            source="graph_edge",
        )
        results[test_node.qualified_name] = (100, record)

    target_tokens = _identifier_tokens(target.name)
    target_squashed = _squashed_identifier(target.name)
    candidates = store.get_nodes_by_kind(["Test", "Function", "Class"])
    for candidate in candidates:
        if candidate.qualified_name == target.qualified_name:
            continue
        if not _is_test_like_node(candidate):
            continue
        score, confidence, evidence = _candidate_score(
            store,
            target,
            candidate,
            target_tokens,
            target_squashed,
        )
        if score <= 0 or confidence_rank[confidence] < min_rank:
            continue
        current = results.get(candidate.qualified_name)
        if current is not None and current[0] >= score:
            continue
        record = _coverage_record(
            candidate,
            confidence=confidence,
            evidence=evidence,
            source="heuristic",
        )
        results[candidate.qualified_name] = (score, record)

    return [
        record
        for _, record in sorted(
            results.values(),
            key=lambda item: (-item[0], item[1]["qualified_name"]),
        )[:limit]
    ]


def has_coverage_evidence(
    store: Any,
    target: GraphNode,
    *,
    minimum_confidence: str = "medium",
    caller_depth: int = 2,
    _seen: set[str] | None = None,
) -> bool:
    """Return whether *target* has direct or credible heuristic test evidence."""
    if infer_tests_for_node(
        store,
        target,
        limit=1,
        minimum_confidence=minimum_confidence,
    ):
        return True
    if caller_depth <= 0 or not target.name.startswith("_"):
        return False

    seen = set() if _seen is None else set(_seen)
    if target.qualified_name in seen:
        return False
    seen.add(target.qualified_name)

    caller_edges = [
        edge
        for edge in store.get_edges_by_target(target.qualified_name)
        if edge.kind == "CALLS"
    ]
    caller_nodes = store.get_nodes_by_qualified_names(
        [edge.source_qualified for edge in caller_edges]
    )
    for caller in caller_nodes.values():
        if has_coverage_evidence(
            store,
            caller,
            minimum_confidence=minimum_confidence,
            caller_depth=caller_depth - 1,
            _seen=seen,
        ):
            return True
    return False
