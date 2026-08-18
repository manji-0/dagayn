"""Review analysis helpers shared by review MCP tools."""

from __future__ import annotations

import logging
from collections import defaultdict
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any, cast

from ..bridge_types import BridgeTransitionRecord
from ..coverage import infer_tests_for_node, is_test_file_path
from ..cross_artifact import (
    bridge_transition_dict,
    is_low_confidence_bridge,
    is_reportable_bridge,
)
from ..cross_artifact import (
    cross_artifact_role as _shared_cross_artifact_role,
)
from ..cross_artifact import (
    is_low_confidence_unresolved_markdown_code_span as _shared_low_conf_code_span,
)
from ..graph.types import GraphNode, ImpactRadiusResult
from ..stability_policy import component_stability_profiles, scope_key_for_file
from ..state_types import ChangeAnalysisResult
from ._common import make_guidance_item

logger = logging.getLogger(__name__)

type ReviewValue = Any
type ReviewPayload = dict[str, ReviewValue]

_ARTIFACT_TO_DOC_ROLES = {
    "implements_contract",
    "explained_by",
    "has_runbook",
    "problem_described_by",
    "discussed_by",
}
_DOC_TO_ARTIFACT_ROLES = {
    "implemented_by",
    "describes_symbol",
    "discusses_artifact",
    "raises_issue_for",
}
_CONTRACT_DOC_ROLES = {
    "implements_contract",
    "implemented_by",
    "has_runbook",
    "explained_by",
}
_LOW_SIGNAL_DOC_FILES = {
    "AGENTS.md",
    "CHANGELOG.md",
    "CLAUDE.md",
    "GEMINI.md",
    "QODER.md",
}
SUPPLEMENTAL_TEST_DENSITY_NODE_LIMIT = 10
_SUPPLEMENTAL_TEST_DENSITY_NODE_LIMIT = SUPPLEMENTAL_TEST_DENSITY_NODE_LIMIT


def _relative_qualified_name(qualified_name: str, root: Path) -> str:
    """Convert an absolute-path qualified name into a repo-relative display form."""
    head, sep, tail = qualified_name.partition("::")
    path = Path(head)
    if path.is_absolute():
        try:
            head = str(path.relative_to(root))
        except ValueError:
            pass
    return f"{head}::{tail}" if sep else head


def _risk_level(score: float) -> str:
    if score >= 0.7:
        return "high"
    if score >= 0.35:
        return "medium"
    return "low"


def _is_markdown_path(path: str) -> bool:
    return path.lower().endswith((".md", ".markdown", ".mdx"))


def _is_low_signal_doc_path(path: str) -> bool:
    return Path(path.replace("\\", "/")).name in _LOW_SIGNAL_DOC_FILES


def _scope_key_for_file(file_path: str | None) -> str | None:
    return scope_key_for_file(file_path)


def _scope_key_for_record(record: ReviewPayload) -> str | None:
    file_path = record.get("file_path") or record.get("file")
    return _scope_key_for_file(str(file_path)) if file_path else None


def _changed_scope_keys(changed_files: list[str]) -> set[str]:
    scopes: set[str] = set()
    for file_path in changed_files:
        normalized = file_path.replace("\\", "/").lstrip("/")
        if not normalized:
            continue
        parts = normalized.split("/")
        scopes.add(parts[0] if len(parts) == 1 else "/".join(parts[:2]))
        if len(parts) >= 3:
            scopes.add("/".join(parts[:3]))
    return scopes


def _dedupe_dicts_by_key(items: list[ReviewPayload], key: str, limit: int) -> list[ReviewPayload]:
    seen: set[str] = set()
    out: list[ReviewPayload] = []
    for item in items:
        value = item.get(key)
        if not isinstance(value, str) or not value or value in seen:
            continue
        seen.add(value)
        out.append(item)
        if len(out) >= limit:
            break
    return out


def _confidence_weight(confidence: Any, confidence_tier: Any) -> float:
    try:
        value = float(confidence)
    except (TypeError, ValueError):
        value = 0.0
    tier = str(confidence_tier or "").upper()
    tier_weight = {
        "EXTRACTED": 1.0,
        "HIGH": 0.9,
        "MEDIUM": 0.65,
        "LOW": 0.35,
    }.get(tier, 0.5)
    return max(value, tier_weight)


def _doc_role_weight(role: str | None) -> float:
    if role in {"implements_contract", "implemented_by"}:
        return 0.95
    if role == "has_runbook":
        return 0.85
    if role in {"explained_by", "describes_symbol"}:
        return 0.75
    if role in {"problem_described_by", "raises_issue_for"}:
        return 0.65
    if role in {"discussed_by", "discusses_artifact"}:
        return 0.45
    return 0.25


def _cross_artifact_role(edge: Any) -> str | None:
    return _shared_cross_artifact_role(edge)


def _is_low_confidence_unresolved_markdown_code_span(edge: Any) -> bool:
    return _shared_low_conf_code_span(edge)


def _is_production_code_node(node: Any) -> bool:
    file_path = str(getattr(node, "file_path", ""))
    language = str(getattr(node, "language", ""))
    if getattr(node, "kind", None) not in {"Function", "Class"}:
        return False
    if getattr(node, "is_test", False) or is_test_file_path(file_path):
        return False
    if language == "markdown" or _is_markdown_path(file_path):
        return False
    return True


def _classify_test_gap(gap: ReviewPayload) -> str:
    file_path = str(gap.get("file", ""))
    language = str(gap.get("language", ""))
    if _is_markdown_path(file_path) or language == "markdown":
        return "documentation"
    normalized = file_path.replace("\\", "/")
    if normalized.startswith("tests/") or "/tests/" in normalized or gap.get("kind") == "Test":
        return "test_artifact"
    return "actionable"


def _rank_test_gaps(test_gaps: list[ReviewPayload], *, limit: int = 5) -> ReviewPayload:
    buckets = {"actionable": [], "documentation": [], "test_artifact": []}
    for gap in test_gaps:
        bucket = _classify_test_gap(gap)
        buckets.setdefault(bucket, []).append(gap)

    return {
        "top_actionable": buckets["actionable"][:limit],
        "counts": {name: len(items) for name, items in buckets.items()},
        "note": (
            "actionable gaps are production-code nodes without direct or credible "
            "heuristic test evidence; documentation and test artifacts are separated "
            "to reduce review noise."
        ),
    }


def _component_stability_profiles(
    store: Any, *, snapshot: Any | None = None
) -> dict[str, ReviewPayload]:
    """Return package-level stability expectations from Clean Architecture metrics."""
    return component_stability_profiles(store, snapshot=snapshot)


def _component_density_by_scope(
    store: Any,
    scopes: set[str],
    *,
    include_supplemental_tests: bool = False,
    supplemental_test_density_node_limit: int = _SUPPLEMENTAL_TEST_DENSITY_NODE_LIMIT,
) -> dict[str, ReviewPayload]:
    """Measure direct test and evidence-tiered documentation density for changed scopes."""
    if not scopes:
        return {}

    scope_nodes: dict[str, list[GraphNode]] = defaultdict(list)
    test_node_counts: dict[str, int] = defaultdict(int)
    for node in store.get_all_nodes(exclude_files=True):
        scope_key = _scope_key_for_file(str(getattr(node, "file_path", "")))
        if scope_key is None or scope_key not in scopes:
            continue
        if _is_production_code_node(node):
            scope_nodes[scope_key].append(node)
        elif getattr(node, "is_test", False) or getattr(node, "kind", "") == "Test":
            test_node_counts[scope_key] += 1

    qns = [node.qualified_name for nodes in scope_nodes.values() for node in nodes]
    outgoing_by_qn, incoming_by_qn = store.get_edges_by_endpoints(qns)
    densities: dict[str, ReviewPayload] = {}
    for scope_key, nodes in scope_nodes.items():
        nodes = sorted(nodes, key=lambda node: node.qualified_name)
        supplemental_nodes = nodes
        if include_supplemental_tests and supplemental_test_density_node_limit > 0:
            supplemental_nodes = nodes[:supplemental_test_density_node_limit]
        supplemental_qns = {node.qualified_name for node in supplemental_nodes}
        tested = 0
        heuristic_tested = 0
        transitive_tested = 0
        authored_documented = 0
        extracted_documented = 0
        heuristic_documented = 0
        for node in nodes:
            outgoing = outgoing_by_qn.get(node.qualified_name, [])
            incoming = incoming_by_qn.get(node.qualified_name, [])
            if any(edge.kind == "TESTED_BY" for edge in outgoing):
                tested += 1
            if include_supplemental_tests and node.qualified_name in supplemental_qns:
                try:
                    inferred_tests = infer_tests_for_node(
                        store,
                        node,
                        limit=1,
                        minimum_confidence="medium",
                    )
                except Exception:  # pragma: no cover - defensive for backend parity drift
                    inferred_tests = []
                if any(test.get("coverage_source") == "heuristic" for test in inferred_tests):
                    heuristic_tested += 1
                try:
                    transitive_tests = store.get_transitive_tests(node.qualified_name)
                except Exception:  # pragma: no cover - defensive for backend parity drift
                    transitive_tests = []
                if any(test.get("indirect") for test in transitive_tests):
                    transitive_tested += 1
            evidence_types: set[str] = set()
            for edge in outgoing:
                if _is_low_confidence_unresolved_markdown_code_span(edge):
                    continue
                role = _cross_artifact_role(edge)
                if role in _ARTIFACT_TO_DOC_ROLES:
                    evidence_types.add(_doc_evidence_type(role, edge.confidence_tier))
            for edge in incoming:
                if _is_low_confidence_unresolved_markdown_code_span(edge):
                    continue
                role = _cross_artifact_role(edge)
                if role in _DOC_TO_ARTIFACT_ROLES:
                    evidence_types.add(_doc_evidence_type(role, edge.confidence_tier))
            if "authored" in evidence_types:
                authored_documented += 1
            if "extracted" in evidence_types:
                extracted_documented += 1
            if "heuristic_reachable" in evidence_types:
                heuristic_documented += 1

        prod_count = len(nodes)
        supplemental_sample_count = len(supplemental_nodes) if include_supplemental_tests else 0
        supplemental_denominator = (
            supplemental_sample_count if include_supplemental_tests else prod_count
        )
        supplemental_truncated = (
            include_supplemental_tests and supplemental_sample_count < prod_count
        )
        documented = authored_documented + extracted_documented
        densities[scope_key] = {
            "production_node_count": prod_count,
            "test_node_count": test_node_counts.get(scope_key, 0),
            "tested_node_count": tested,
            "heuristic_tested_node_count": heuristic_tested,
            "transitive_tested_node_count": transitive_tested,
            "supplemental_test_density_evaluated": include_supplemental_tests,
            "supplemental_test_density_sampled_node_count": supplemental_sample_count,
            "supplemental_test_density_truncated": supplemental_truncated,
            "documented_node_count": documented,
            "authored_documented_node_count": authored_documented,
            "extracted_documented_node_count": extracted_documented,
            "heuristic_documented_node_count": heuristic_documented,
            "direct_test_density": round(tested / prod_count, 4) if prod_count else 0.0,
            "heuristic_test_density": (
                round(heuristic_tested / supplemental_denominator, 4)
                if supplemental_denominator
                else 0.0
            ),
            "transitive_test_density": (
                round(transitive_tested / supplemental_denominator, 4)
                if supplemental_denominator
                else 0.0
            ),
            "documentation_density": round(documented / prod_count, 4) if prod_count else 0.0,
            "authored_documentation_density": (
                round(authored_documented / prod_count, 4) if prod_count else 0.0
            ),
            "extracted_documentation_density": (
                round(extracted_documented / prod_count, 4) if prod_count else 0.0
            ),
            "heuristic_documentation_density": (
                round(heuristic_documented / prod_count, 4) if prod_count else 0.0
            ),
        }
    return densities


def _review_signal_quality(
    reason_codes: list[str],
    docs: list[ReviewPayload],
    test_gaps: list[ReviewPayload],
    stability_contracts: list[ReviewPayload] | None = None,
) -> ReviewPayload:
    uncertain = []
    if any(doc.get("evidence_level") == "heuristic_reachable" for doc in docs):
        uncertain.append("documentation candidates are graph-reachable markdown nodes")
    if test_gaps:
        uncertain.append(
            "test gaps use direct TESTED_BY edges plus medium-confidence naming/source heuristics"
        )
    if stability_contracts:
        uncertain.append(
            (
                "stable-component density compares current graph evidence to "
                "package-level SDP/SAP thresholds"
            )
        )
    return {
        "graph_facts": [
            code
            for code in reason_codes
            if code
            in {
                "affected_flows",
                "critical_flow_affected",
                "wide_blast_radius",
                "changed_hotspot",
                "impacted_hotspot",
                "architecture_violation_in_changed_scope",
                "stable_component_contract_gap",
                "cross_artifact_proximity",
            }
        ],
        "heuristics": [
            code
            for code in reason_codes
            if code
            in {
                "test_gaps",
                "documentation_update_candidates",
                "stable_density_gap",
                "low_confidence_cross_artifact_bridge",
            }
        ],
        "uncertain": uncertain,
    }


def _recommend_tests(
    store: Any,
    changed_functions: list[ReviewPayload],
    affected_flows: list[ReviewPayload],
    *,
    limit: int = 10,
    stability_profiles: dict[str, ReviewPayload] | None = None,
) -> list[ReviewPayload]:
    """Recommend tests that directly or indirectly cover changed code."""
    recommendations: list[ReviewPayload] = []
    stability_profiles = stability_profiles or {}

    for func in changed_functions:
        qualified_name = func.get("qualified_name")
        if not isinstance(qualified_name, str) or not qualified_name:
            continue
        scope_key = _scope_key_for_record(func)
        profile = stability_profiles.get(scope_key or "", {})
        stability_bonus = 0.1 if profile.get("stable") or profile.get("should_be_stable") else 0.0
        try:
            tests = store.get_transitive_tests(qualified_name)
        except Exception:  # pragma: no cover - defensive for backend parity drift
            tests = []
        for test in tests:
            test_qn = test.get("qualified_name")
            if not isinstance(test_qn, str):
                continue
            recommendations.append(
                {
                    "name": test.get("name", test_qn.rsplit("::", 1)[-1]),
                    "qualified_name": test_qn,
                    "file": test.get("file_path"),
                    "reason": (
                        "indirect coverage via changed dependency"
                        if test.get("indirect")
                        else "direct coverage of changed code"
                    ),
                    "source": qualified_name,
                    "scope_key": scope_key,
                    "score": round(
                        min(
                            1.0,
                            (0.82 if test.get("indirect") else 0.95) + stability_bonus,
                        ),
                        4,
                    ),
                    "evidence_level": "graph_indirect" if test.get("indirect") else "graph_direct",
                    "stability": {
                        "stable": bool(profile.get("stable")),
                        "should_be_stable": bool(profile.get("should_be_stable")),
                        "instability": profile.get("instability"),
                    },
                }
            )
        if tests:
            continue
        try:
            node = store.get_node(qualified_name)
        except Exception:  # pragma: no cover - defensive for backend parity drift
            node = None
        if node is None:
            continue
        for test in infer_tests_for_node(store, node, limit=3, minimum_confidence="medium"):
            test_qn = test.get("qualified_name")
            if not isinstance(test_qn, str):
                continue
            recommendations.append(
                {
                    "name": test.get("name", test_qn.rsplit("::", 1)[-1]),
                    "qualified_name": test_qn,
                    "file": test.get("file_path"),
                    "reason": "heuristic coverage candidate",
                    "source": qualified_name,
                    "confidence": test.get("confidence"),
                    "evidence": test.get("evidence", []),
                    "scope_key": scope_key,
                    "score": round(
                        min(
                            1.0,
                            (0.75 if test.get("confidence") == "high" else 0.65) + stability_bonus,
                        ),
                        4,
                    ),
                    "evidence_level": "heuristic",
                    "stability": {
                        "stable": bool(profile.get("stable")),
                        "should_be_stable": bool(profile.get("should_be_stable")),
                        "instability": profile.get("instability"),
                    },
                }
            )

    # Flow names are not tests by definition, but test-named flows are useful
    # review commands when the graph cannot resolve TESTED_BY edges.
    for flow in affected_flows:
        name = str(flow.get("name", ""))
        if not name.lower().startswith(("test", "it:", "describe")):
            continue
        recommendations.append(
            {
                "name": name,
                "qualified_name": name,
                "file": None,
                "reason": "affected test flow",
                "source": "affected_flows",
                "score": 0.55,
                "evidence_level": "flow",
            }
        )

    recommendations.sort(
        key=lambda item: (
            -float(item.get("score", 0.0) or 0.0),
            str(item.get("qualified_name", "")),
        )
    )
    return _dedupe_dicts_by_key(recommendations, "qualified_name", limit)


def _doc_evidence_type(role: str | None, confidence_tier: Any) -> str:
    if role in _CONTRACT_DOC_ROLES:
        return "authored"
    tier = str(confidence_tier or "").upper()
    if tier in {"EXTRACTED", "HIGH"}:
        return "extracted"
    return "heuristic_reachable"


def _doc_missingness(role: str | None, confidence_tier: Any) -> list[ReviewPayload]:
    missing: list[ReviewPayload] = []
    tier = str(confidence_tier or "").upper()
    if role not in _CONTRACT_DOC_ROLES:
        missing.append(
            {
                "reason_code": "not_contract_documentation_edge",
                "severity": "low",
                "claim_effect": "candidate may be explanatory rather than contract-bearing",
            }
        )
    if tier in {"LOW", "UNKNOWN", ""}:
        missing.append(
            {
                "reason_code": "low_confidence_documentation_edge",
                "severity": "medium",
                "claim_effect": "read the section before treating it as authored evidence",
            }
        )
    return missing


def _directive_hint_for_role(role: str | None, *, direction: str) -> str:
    if direction == "artifact_to_doc":
        return {
            "implements_contract": "# dagayn: implements <doc-section>",
            "explained_by": "# dagayn: explained-by <doc-section>",
            "has_runbook": "# dagayn: has-runbook <doc-section>",
            "problem_described_by": "# dagayn: problem-described-by <doc-section>",
            "discussed_by": "# dagayn: discussed-by <doc-section>",
        }.get(str(role), "# dagayn: discussed-by <doc-section>")
    return {
        "implemented_by": "<!-- dagayn: implemented-by <code-symbol> -->",
        "describes_symbol": "<!-- dagayn: describes-symbol <code-symbol> -->",
        "discusses_artifact": "<!-- dagayn: discusses-artifact <code-symbol> -->",
        "raises_issue_for": "<!-- dagayn: raises-issue-for <code-symbol> -->",
    }.get(str(role), "<!-- dagayn: discusses-artifact <code-symbol> -->")


def _documentation_update_candidates(
    store: Any,
    impact: ImpactRadiusResult,
    changed_functions: list[ReviewPayload],
    changed_files: list[str],
    *,
    limit: int = 10,
    stability_profiles: dict[str, ReviewPayload] | None = None,
    include_heuristic_docs: bool = False,
) -> list[ReviewPayload]:
    changed_set = set(changed_files)
    code_changed = any(not _is_markdown_path(path) for path in changed_files)
    if not code_changed:
        return []

    stability_profiles = stability_profiles or {}
    candidates: list[ReviewPayload] = []

    source_qns = [
        qn
        for qn in (func.get("qualified_name") for func in changed_functions)
        if isinstance(qn, str) and qn
    ]
    source_by_qn = {
        func.get("qualified_name"): func
        for func in changed_functions
        if isinstance(func.get("qualified_name"), str)
    }
    outgoing_by_qn, incoming_by_qn = store.get_edges_by_endpoints(source_qns)
    doc_qns: set[str] = set()
    for qn in source_qns:
        source_record = source_by_qn.get(qn, {})
        scope_key = _scope_key_for_record(source_record)
        profile = stability_profiles.get(scope_key or "", {})
        stability_bonus = 0.08 if profile.get("stable") or profile.get("should_be_stable") else 0.0
        for edge in outgoing_by_qn.get(qn, []):
            if _is_low_confidence_unresolved_markdown_code_span(edge):
                continue
            role = _cross_artifact_role(edge)
            if role not in _ARTIFACT_TO_DOC_ROLES:
                continue
            if _is_low_signal_doc_path(edge.file_path) and role not in _CONTRACT_DOC_ROLES:
                continue
            doc_qn = edge.target_qualified
            doc_qns.add(doc_qn)
            score = min(1.0, _doc_role_weight(role) + stability_bonus)
            candidates.append(
                {
                    "file": edge.file_path,
                    "section": doc_qn.rsplit("::", 1)[-1],
                    "qualified_name": doc_qn,
                    "reason": "documentation edge from changed code",
                    "source": qn,
                    "relationship_role": role,
                    "confidence": edge.confidence,
                    "confidence_tier": edge.confidence_tier,
                    "score": round(score, 4),
                    "evidence_level": "cross_artifact",
                    "evidence_type": _doc_evidence_type(role, edge.confidence_tier),
                    "missingness": _doc_missingness(role, edge.confidence_tier),
                    "documentation_action": (
                        "Read this section and update the contract directive if behavior changed."
                    ),
                    "directive_hint": _directive_hint_for_role(
                        role,
                        direction="artifact_to_doc",
                    ),
                    "scope_key": scope_key,
                    "stable_contract": role in _CONTRACT_DOC_ROLES,
                }
            )
        for edge in incoming_by_qn.get(qn, []):
            if _is_low_confidence_unresolved_markdown_code_span(edge):
                continue
            role = _cross_artifact_role(edge)
            if role not in _DOC_TO_ARTIFACT_ROLES:
                continue
            if _is_low_signal_doc_path(edge.file_path) and role not in _CONTRACT_DOC_ROLES:
                continue
            doc_qn = edge.source_qualified
            doc_qns.add(doc_qn)
            score = min(
                1.0,
                _doc_role_weight(role)
                + 0.08 * _confidence_weight(edge.confidence, edge.confidence_tier)
                + stability_bonus,
            )
            candidates.append(
                {
                    "file": edge.file_path,
                    "section": doc_qn.rsplit("::", 1)[-1],
                    "qualified_name": doc_qn,
                    "reason": "documentation edge to changed code",
                    "source": qn,
                    "relationship_role": role,
                    "confidence": edge.confidence,
                    "confidence_tier": edge.confidence_tier,
                    "score": round(score, 4),
                    "evidence_level": "cross_artifact",
                    "evidence_type": _doc_evidence_type(role, edge.confidence_tier),
                    "missingness": _doc_missingness(role, edge.confidence_tier),
                    "documentation_action": (
                        "Read this section and update the contract directive if behavior changed."
                    ),
                    "directive_hint": _directive_hint_for_role(
                        role,
                        direction="doc_to_artifact",
                    ),
                    "scope_key": scope_key,
                    "stable_contract": role in _CONTRACT_DOC_ROLES,
                }
            )

    if not include_heuristic_docs:
        candidates.sort(
            key=lambda item: (
                -float(item.get("score", 0.0) or 0.0),
                str(item.get("qualified_name", "")),
            )
        )
        return _dedupe_dicts_by_key(candidates, "qualified_name", limit)

    for node in impact.get("impacted_nodes", []):
        file_path = getattr(node, "file_path", "")
        if not isinstance(file_path, str) or not _is_markdown_path(file_path):
            continue
        if _is_low_signal_doc_path(file_path):
            continue
        if file_path in changed_set:
            continue
        qn = getattr(node, "qualified_name", None)
        if isinstance(qn, str) and qn in doc_qns:
            continue
        candidates.append(
            {
                "file": file_path,
                "section": getattr(node, "name", None),
                "qualified_name": qn,
                "reason": "markdown node reached from changed code/doc graph",
                "score": 0.25,
                "evidence_level": "heuristic_reachable",
                "evidence_type": "heuristic_reachable",
                "missingness": [
                    {
                        "reason_code": "heuristic_documentation_reachability",
                        "severity": "medium",
                        "claim_effect": "candidate is reachable but not an authored contract edge",
                    }
                ],
                "documentation_action": (
                    "Read this section before deciding whether docs need updates."
                ),
                "directive_hint": _directive_hint_for_role(None, direction="doc_to_artifact"),
            }
        )

    candidates.sort(
        key=lambda item: (
            -float(item.get("score", 0.0) or 0.0),
            str(item.get("qualified_name", "")),
        )
    )
    return _dedupe_dicts_by_key(candidates, "qualified_name", limit)


def _stability_contracts(
    changed_functions: list[ReviewPayload],
    recommended_tests: list[ReviewPayload],
    docs: list[ReviewPayload],
    test_gaps: list[ReviewPayload],
    stability_profiles: dict[str, ReviewPayload],
    component_density: dict[str, ReviewPayload],
    *,
    limit: int = 10,
) -> list[ReviewPayload]:
    tests_by_source: dict[str, list[ReviewPayload]] = defaultdict(list)
    for item in recommended_tests:
        source = item.get("source")
        if isinstance(source, str):
            tests_by_source[source].append(item)

    docs_by_source: dict[str, list[ReviewPayload]] = defaultdict(list)
    for item in docs:
        source = item.get("source")
        if isinstance(source, str):
            docs_by_source[source].append(item)

    gap_qns = {str(gap.get("qualified_name")) for gap in test_gaps if gap.get("qualified_name")}
    changed_by_scope: dict[str, list[ReviewPayload]] = defaultdict(list)
    for func in changed_functions:
        if func.get("kind") not in {"Function", "Class"}:
            continue
        file_path = str(func.get("file_path") or func.get("file") or "")
        if func.get("is_test") or is_test_file_path(file_path) or _is_markdown_path(file_path):
            continue
        scope_key = _scope_key_for_record(func)
        if scope_key:
            changed_by_scope[scope_key].append(func)

    contracts: list[ReviewPayload] = []
    for scope_key, funcs in changed_by_scope.items():
        profile = stability_profiles.get(scope_key)
        if not profile:
            continue
        if not profile.get("stable") and not profile.get("should_be_stable"):
            continue
        density = component_density.get(scope_key, {})
        changed_qns = [
            func.get("qualified_name")
            for func in funcs
            if isinstance(func.get("qualified_name"), str)
        ]
        changed_with_tests = [qn for qn in changed_qns if tests_by_source.get(qn)]
        changed_with_docs = [qn for qn in changed_qns if docs_by_source.get(qn)]
        missing_tests = [qn for qn in changed_qns if qn not in changed_with_tests and qn in gap_qns]
        missing_docs = [qn for qn in changed_qns if qn not in changed_with_docs]

        # A scope with no density entry was never measured. Defaulting the
        # observed values to 0.0 published an unmeasured scope as one with no
        # coverage, and unconditionally appended the low-density reason code.
        density_measured = bool(density)
        supplemental_evaluated = bool(density.get("supplemental_test_density_evaluated"))
        observed_test_density = (
            float(density.get("direct_test_density", 0.0) or 0.0) if density_measured else None
        )
        # The supplemental pass only runs at detail_level="verbose"; without it
        # these are 0/prod_count = 0.0, indistinguishable from a measured zero.
        observed_heuristic_test_density = (
            float(density.get("heuristic_test_density", 0.0) or 0.0)
            if supplemental_evaluated
            else None
        )
        observed_transitive_test_density = (
            float(density.get("transitive_test_density", 0.0) or 0.0)
            if supplemental_evaluated
            else None
        )
        observed_doc_density = (
            float(density.get("documentation_density", 0.0) or 0.0) if density_measured else None
        )
        expected_test_density = float(profile.get("expected_test_density", 0.5) or 0.5)
        expected_doc_density = float(profile.get("expected_doc_density", 0.25) or 0.25)
        reason_codes = list(profile.get("reason_codes", []))
        if observed_test_density is not None and observed_test_density < expected_test_density:
            reason_codes.append("stable_component_low_test_density")
        elif observed_test_density is None:
            reason_codes.append("stable_component_test_density_unmeasured")
        if (
            observed_doc_density is not None and observed_doc_density < expected_doc_density
        ) or missing_docs:
            reason_codes.append("stable_component_missing_documentation")

        status = (
            "warn" if any(code.startswith("stable_component_") for code in reason_codes) else "ok"
        )
        contracts.append(
            {
                "scope_key": scope_key,
                "status": status,
                "instability": profile.get("instability"),
                "ca": profile.get("ca"),
                "ce": profile.get("ce"),
                "stable": profile.get("stable"),
                "should_be_stable": profile.get("should_be_stable"),
                "expected_test_density": expected_test_density,
                "observed_direct_test_density": observed_test_density,
                "observed_heuristic_test_density": observed_heuristic_test_density,
                "observed_transitive_test_density": observed_transitive_test_density,
                "supplemental_test_density_evaluated": supplemental_evaluated,
                "expected_doc_density": expected_doc_density,
                "observed_documentation_density": observed_doc_density,
                "changed_production_node_count": len(changed_qns),
                "changed_nodes_with_recommended_tests": len(changed_with_tests),
                "changed_nodes_with_docs": len(changed_with_docs),
                "missing_changed_tests": missing_tests[:5],
                "missing_changed_docs": missing_docs[:5],
                "reason_codes": reason_codes,
            }
        )

    contracts.sort(
        key=lambda item: (
            0 if item.get("status") == "warn" else 1,
            float(item.get("instability", 1.0) or 1.0),
            str(item.get("scope_key", "")),
        )
    )
    return contracts[:limit]


def _hotspot_proximity(
    store: Any,
    impact: ImpactRadiusResult,
    *,
    top_n: int = 25,
    limit: int = 5,
    snapshot: Any | None = None,
) -> ReviewPayload:
    try:
        from ..analysis import find_bridge_nodes, find_hub_nodes

        hubs = find_hub_nodes(
            store,
            top_n=top_n,
            artifact_scope="code",
            include_tests=False,
            snapshot=snapshot,
        )
        bridges = find_bridge_nodes(
            store,
            top_n=top_n,
            artifact_scope="code",
            include_tests=False,
            snapshot=snapshot,
        )
    except Exception:  # pragma: no cover - defensive for backend parity drift
        hubs = []
        bridges = []

    changed_qns = {
        getattr(node, "qualified_name", "")
        for node in impact.get("changed_nodes", [])
        if getattr(node, "qualified_name", "")
    }
    impacted_qns = {
        getattr(node, "qualified_name", "")
        for node in impact.get("impacted_nodes", [])
        if getattr(node, "qualified_name", "")
    }

    def _matches(items: Sequence[Mapping[str, object]], qns: set[str]) -> list[ReviewPayload]:
        return [cast(ReviewPayload, item) for item in items if item.get("qualified_name") in qns][
            :limit
        ]

    return {
        "changed_hubs": _matches(hubs, changed_qns),
        "changed_bridges": _matches(bridges, changed_qns),
        "impacted_hubs": _matches(hubs, impacted_qns),
        "impacted_bridges": _matches(bridges, impacted_qns),
        "method": {
            "hub": f"top {top_n} by total degree",
            "bridge": f"top {top_n} by betweenness centrality",
        },
    }


DOC_FOLLOW_UPS = (
    'query_graph_tool pattern="docs_for"',
    'query_graph_tool pattern="implementations_of"',
)


def _cross_artifact_proximity(
    store: Any,
    impact: ImpactRadiusResult,
    changed_functions: list[ReviewPayload],
    *,
    limit: int = 8,
) -> ReviewPayload:
    """Surface reportable CROSS_ARTIFACT bridges near the change as review leads."""
    seed_qns = {
        func.get("qualified_name")
        for func in changed_functions
        if isinstance(func.get("qualified_name"), str)
    }
    seed_qns.update(
        str(getattr(node, "qualified_name", ""))
        for node in impact.get("changed_nodes", [])
        if getattr(node, "qualified_name", "")
    )
    seed_qns.discard("")
    if not seed_qns:
        return {
            "reportable_bridges": [],
            "low_confidence_bridges": [],
            "follow_ups": list(DOC_FOLLOW_UPS),
            "counts": {"reportable": 0, "low_confidence": 0},
        }

    outgoing, incoming = store.get_edges_by_endpoints(list(seed_qns))
    reportable: list[BridgeTransitionRecord] = []
    low_confidence: list[BridgeTransitionRecord] = []
    seen: set[tuple[str, str]] = set()
    for edge_map in (outgoing, incoming):
        for edges in edge_map.values():
            for edge in edges:
                if getattr(edge, "kind", None) != "CROSS_ARTIFACT":
                    continue
                key = (edge.source_qualified, edge.target_qualified)
                if key in seen:
                    continue
                seen.add(key)
                meta = bridge_transition_dict(edge)
                if is_reportable_bridge(edge):
                    reportable.append(meta)
                elif is_low_confidence_bridge(edge):
                    low_confidence.append(meta)

    reportable.sort(
        key=lambda item: (
            -float(item.get("confidence") or 0.0),
            str(item.get("relationship_role") or ""),
            str(item.get("source") or ""),
        )
    )
    low_confidence.sort(
        key=lambda item: (
            str(item.get("relationship_role") or ""),
            str(item.get("source") or ""),
        )
    )
    return {
        "reportable_bridges": reportable[:limit],
        "low_confidence_bridges": low_confidence[:limit],
        "follow_ups": [
            *DOC_FOLLOW_UPS,
            "follow CROSS_ARTIFACT bridge edges from changed nodes",
        ],
        "counts": {
            "reportable": len(reportable),
            "low_confidence": len(low_confidence),
        },
    }


def _architecture_delta_summary(
    store: Any,
    changed_files: list[str],
    *,
    limit: int = 5,
    snapshot: Any | None = None,
) -> ReviewPayload:
    """Summarize current architecture risks in scopes touched by the change."""
    scopes = _changed_scope_keys(changed_files)
    if not scopes:
        return {
            "mode": "current_graph_changed_scope",
            "changed_scopes": [],
            "related_violations": {},
            "counts": {},
            "note": "No changed scopes were available for architecture filtering.",
        }

    try:
        from ..architecture import find_adp_violations, find_sdp_violations
        from ..sap import find_sap_violations

        adp = find_adp_violations(store, granularity="package", snapshot=snapshot)
        sdp = find_sdp_violations(store, granularity="package", snapshot=snapshot)
        sap = find_sap_violations(store, scope_kind="package", snapshot=snapshot)
    except Exception:  # pragma: no cover - defensive for backend parity drift
        adp = []
        sdp = []
        sap = []

    def _touches_scope(value: str) -> bool:
        normalized = value.replace("\\", "/")
        return any(normalized == scope or normalized.startswith(f"{scope}/") for scope in scopes)

    related_adp = [
        violation
        for violation in adp
        if any(_touches_scope(str(node)) for node in violation.get("nodes", []))
    ][:limit]
    related_sdp = [
        violation
        for violation in sdp
        if _touches_scope(str(violation.get("source", "")))
        or _touches_scope(str(violation.get("target", "")))
    ][:limit]
    related_sap = [
        violation
        for violation in sap
        if _touches_scope(str(violation.get("scope_key", "")))
        or _touches_scope(str(violation.get("display_name", "")))
    ][:limit]

    return {
        "mode": "current_graph_changed_scope",
        "baseline_comparison": {
            "available": False,
            "reason": (
                "review currently compares changed scopes against the current graph; "
                "a separate base graph is not materialized for every review call."
            ),
        },
        "changed_scopes": sorted(scopes),
        "related_violations": {
            "adp": related_adp,
            "sdp": related_sdp,
            "sap": related_sap,
        },
        "counts": {
            "adp": len(related_adp),
            "sdp": len(related_sdp),
            "sap": len(related_sap),
        },
        "note": (
            "This summarizes current graph violations that touch changed scopes; "
            "it does not build a separate baseline graph."
        ),
    }


def _review_guidance_items(
    *,
    risk: str,
    risk_score: float,
    reason_codes: list[str],
    recommended_tests: list[ReviewPayload],
    docs: list[ReviewPayload],
    test_gap_ranking: ReviewPayload,
    stability_contracts: list[ReviewPayload],
    affected_flow_rankings: list[ReviewPayload],
    hotspots: ReviewPayload,
    architecture_delta: ReviewPayload,
    signal_quality: ReviewPayload,
    cross_artifact_proximity: ReviewPayload | None = None,
) -> list[ReviewPayload]:
    guidance: list[ReviewPayload] = []
    cross_artifact_proximity = cross_artifact_proximity or {}
    actionable_gap_count = int(test_gap_ranking.get("counts", {}).get("actionable", 0) or 0)
    if actionable_gap_count or recommended_tests:
        guidance.append(
            make_guidance_item(
                claim=(
                    f"{actionable_gap_count} production change(s) need focused test attention."
                    if actionable_gap_count
                    else "Graph-linked tests are available for the changed code."
                ),
                evidence=[
                    {
                        "type": (
                            "computed"
                            if item.get("evidence_level", "").startswith("graph")
                            else "evaluated"
                        ),
                        "source": item.get("source"),
                        "target": item.get("qualified_name"),
                        "score": item.get("score"),
                        "evidence_level": item.get("evidence_level"),
                    }
                    for item in recommended_tests[:5]
                ],
                confidence="medium" if actionable_gap_count else "high",
                missingness=[
                    {
                        "reason_code": "heuristic_test_gap_detection",
                        "severity": "low",
                        "claim_effect": "test recommendations may miss naming-only coverage",
                    }
                ]
                if actionable_gap_count
                else [],
                action=(
                    'review_tool mode="context" -- inspect changed nodes and run focused tests'
                ),
                reason_codes=["test_gaps"] if actionable_gap_count else ["recommended_tests"],
                counts={
                    "actionable_test_gap_count": actionable_gap_count,
                    "recommended_test_count": len(recommended_tests),
                },
            )
        )

    if docs:
        top_doc = docs[0]
        guidance.append(
            make_guidance_item(
                claim="Documentation or contract evidence is connected to this change.",
                evidence=[
                    {
                        "type": item.get("evidence_type", "computed"),
                        "file": item.get("file"),
                        "section": item.get("section"),
                        "relationship_role": item.get("relationship_role"),
                        "evidence_level": item.get("evidence_level"),
                        "score": item.get("score"),
                    }
                    for item in docs[:5]
                ],
                confidence="high" if top_doc.get("evidence_type") == "authored" else "low",
                missingness=[missing for item in docs for missing in item.get("missingness", [])][
                    :5
                ],
                action=(
                    'query_graph_tool pattern="docs_for" -- inspect linked contract docs; '
                    'also try pattern="implementations_of" for inverse follow-ups'
                ),
                reason_codes=["documentation_update_candidates"],
                counts={"documentation_candidate_count": len(docs)},
            )
        )

    reportable_bridges = list(cross_artifact_proximity.get("reportable_bridges") or [])
    low_conf_bridges = list(cross_artifact_proximity.get("low_confidence_bridges") or [])
    if reportable_bridges:
        guidance.append(
            make_guidance_item(
                claim="Cross-artifact bridges are near this change and should be followed.",
                evidence={
                    "type": "extracted",
                    "bridges": reportable_bridges[:5],
                    "follow_ups": cross_artifact_proximity.get("follow_ups", []),
                },
                confidence="high",
                missingness=[
                    {
                        "reason_code": "cross_artifact_bridge_is_static_evidence",
                        "severity": "low",
                        "claim_effect": (
                            "bridge proximity is structural; confirm with docs_for / "
                            "implementations_of or source reads"
                        ),
                    }
                ],
                action=(
                    'query_graph_tool pattern="docs_for" -- follow docs; '
                    'pattern="implementations_of" -- follow implementations; '
                    "inspect CROSS_ARTIFACT neighbors"
                ),
                reason_codes=["cross_artifact_proximity"],
                counts={
                    "reportable_bridge_count": len(reportable_bridges),
                    "low_confidence_bridge_count": len(low_conf_bridges),
                },
            )
        )
    elif low_conf_bridges:
        guidance.append(
            make_guidance_item(
                claim="Low-confidence cross-artifact bridges are caveats near this change.",
                evidence={
                    "type": "extracted",
                    "bridges": low_conf_bridges[:5],
                },
                confidence="low",
                missingness=[
                    {
                        "reason_code": "low_confidence_cross_artifact_bridge",
                        "severity": "medium",
                        "claim_effect": (
                            "do not treat the other side as confirmed impact without verification"
                        ),
                        "bridge": item,
                    }
                    for item in low_conf_bridges[:3]
                ],
                action=(
                    'query_graph_tool pattern="docs_for" -- verify before treating as hard impact'
                ),
                reason_codes=["low_confidence_cross_artifact_bridge"],
                counts={"low_confidence_bridge_count": len(low_conf_bridges)},
            )
        )

    warn_contracts = [item for item in stability_contracts if item.get("status") == "warn"]
    if warn_contracts:
        guidance.append(
            make_guidance_item(
                claim="A stable or should-be-stable component has a quality-policy gap.",
                evidence=[
                    {
                        "type": "computed",
                        "scope_key": item.get("scope_key"),
                        "instability": item.get("instability"),
                        "expected_test_density": item.get("expected_test_density"),
                        "observed_direct_test_density": item.get("observed_direct_test_density"),
                        "observed_heuristic_test_density": item.get(
                            "observed_heuristic_test_density"
                        ),
                        "observed_transitive_test_density": item.get(
                            "observed_transitive_test_density"
                        ),
                        "expected_doc_density": item.get("expected_doc_density"),
                        "observed_documentation_density": item.get(
                            "observed_documentation_density"
                        ),
                    }
                    for item in warn_contracts[:3]
                ],
                confidence="medium",
                missingness=[
                    {
                        "reason_code": "stability_policy_uses_current_graph",
                        "severity": "low",
                        "claim_effect": "policy is calibrated by current SDP/SAP graph metrics",
                    }
                ],
                action=(
                    'architecture_analysis_tool mode="overview" -- inspect stable component policy'
                ),
                reason_codes=["stable_component_contract_gap"],
                counts={"stable_contract_warning_count": len(warn_contracts)},
            )
        )

    if affected_flow_rankings:
        guidance.append(
            make_guidance_item(
                claim="Changed code intersects ranked execution flows.",
                evidence=[
                    {
                        "type": "computed",
                        "name": flow.get("name"),
                        "criticality": flow.get("criticality"),
                        "node_count": flow.get("node_count"),
                        "file_count": flow.get("file_count"),
                    }
                    for flow in affected_flow_rankings[:3]
                ],
                confidence="medium",
                missingness=[
                    {
                        "reason_code": "flow_rank_is_not_coverage",
                        "severity": "low",
                        "claim_effect": "criticality ranks review leads, not sufficient coverage",
                    }
                ],
                action='review_tool mode="affected_flows" -- inspect affected flow paths',
                reason_codes=["affected_flows"],
                counts={"affected_flow_count": len(affected_flow_rankings)},
            )
        )

    architecture_counts = architecture_delta.get("counts", {})
    if any(architecture_counts.values()):
        guidance.append(
            make_guidance_item(
                claim="Current architecture violations touch changed scopes.",
                evidence={
                    "type": "computed",
                    "changed_scopes": architecture_delta.get("changed_scopes", []),
                    "counts": architecture_counts,
                    "baseline_comparison": architecture_delta.get("baseline_comparison"),
                },
                confidence="medium",
                missingness=[
                    {
                        "reason_code": "current_graph_not_baseline_delta",
                        "severity": "low",
                        "claim_effect": (
                            "violation is scoped to current graph, not a new-introduced proof"
                        ),
                    }
                ],
                action='architecture_analysis_tool mode="overview" -- inspect scoped risks',
                reason_codes=["architecture_violation_in_changed_scope"],
                counts=dict(architecture_counts),
            )
        )

    hotspot_count = sum(
        len(hotspots.get(key, []))
        for key in ("changed_hubs", "changed_bridges", "impacted_hubs", "impacted_bridges")
    )
    if hotspot_count:
        guidance.append(
            make_guidance_item(
                claim="The change is near graph hub or bridge nodes.",
                evidence={
                    "type": "computed",
                    "method": hotspots.get("method"),
                    "changed_hubs": hotspots.get("changed_hubs", []),
                    "changed_bridges": hotspots.get("changed_bridges", []),
                    "impacted_hubs": hotspots.get("impacted_hubs", []),
                    "impacted_bridges": hotspots.get("impacted_bridges", []),
                },
                confidence="medium",
                action='review_tool mode="impact" -- inspect blast radius around hotspots',
                reason_codes=["changed_hotspot", "impacted_hotspot"],
                counts={"hotspot_match_count": hotspot_count},
            )
        )

    if risk in {"medium", "high"} and not guidance:
        guidance.append(
            make_guidance_item(
                claim=f"Review priority is {risk} by graph impact score.",
                evidence={
                    "type": "computed",
                    "metric": "review_priority_score",
                    "legacy_metric": "risk_score",
                    "value": risk_score,
                },
                confidence="medium",
                action='review_tool mode="context" -- inspect changed nodes before merging',
                reason_codes=reason_codes,
                counts={"graph_fact_count": len(signal_quality.get("graph_facts", []))},
            )
        )
    return guidance


def _change_analysis_summary(
    store: Any,
    analysis: ChangeAnalysisResult,
    impact: ImpactRadiusResult,
    changed_files: list[str],
    *,
    detail_level: str = "standard",
) -> ReviewPayload:
    risk_score = analysis.risk_score or 0.0
    affected_flows = cast(list[ReviewPayload], list(analysis.affected_flows))
    test_gaps = cast(list[ReviewPayload], list(analysis.test_gaps))
    changed_functions = cast(list[ReviewPayload], list(analysis.changed_functions))
    risk = _risk_level(risk_score)

    # One shared snapshot for every downstream sub-analysis (stability
    # profiles, hotspot proximity, architecture delta). Each helper otherwise
    # re-reads the full edge table on its own (~0.3 s per call).
    from ..analysis import build_graph_snapshot

    snapshot = build_graph_snapshot(store)
    stability_profiles = _component_stability_profiles(store, snapshot=snapshot)
    changed_scopes = {
        scope_key
        for scope_key in (_scope_key_for_record(func) for func in changed_functions)
        if scope_key
    }
    component_density = _component_density_by_scope(
        store,
        changed_scopes,
        include_supplemental_tests=detail_level == "verbose",
    )
    recommended_tests = _recommend_tests(
        store,
        changed_functions,
        affected_flows,
        stability_profiles=stability_profiles,
    )
    docs = _documentation_update_candidates(
        store,
        impact,
        changed_functions,
        changed_files,
        stability_profiles=stability_profiles,
        include_heuristic_docs=detail_level == "verbose",
    )
    hotspots = _hotspot_proximity(store, impact, snapshot=snapshot)
    cross_artifact = _cross_artifact_proximity(store, impact, changed_functions)
    architecture_delta = _architecture_delta_summary(store, changed_files, snapshot=snapshot)
    test_gap_ranking = _rank_test_gaps(test_gaps)
    stability_contracts = _stability_contracts(
        changed_functions,
        recommended_tests,
        docs,
        test_gaps,
        stability_profiles,
        component_density,
    )

    reason_codes: list[str] = []
    attribution = analysis.attribution or {}
    for code in attribution.get("reason_codes", []):
        if code not in reason_codes:
            reason_codes.append(code)
    if (
        analysis.diff_parse_status == "base_unresolved"
        and "diff_base_unreachable" not in reason_codes
    ):
        reason_codes.append("diff_base_unreachable")
    if analysis.unmapped_changed_files and "unmapped_changed_files" not in reason_codes:
        reason_codes.append("unmapped_changed_files")
    if risk == "high":
        reason_codes.append("high_risk_score")
    elif risk == "medium":
        reason_codes.append("medium_risk_score")
    if len(changed_functions) >= 10:
        reason_codes.append("many_changed_graph_nodes")
    if affected_flows:
        reason_codes.append("affected_flows")
    if any(float(flow.get("criticality", 0.0) or 0.0) >= 0.5 for flow in affected_flows):
        reason_codes.append("critical_flow_affected")
    if test_gaps:
        reason_codes.append("test_gaps")
    if len(impact.get("impacted_nodes", [])) > 20:
        reason_codes.append("wide_blast_radius")
    if docs:
        reason_codes.append("documentation_update_candidates")
    if cross_artifact.get("counts", {}).get("reportable"):
        reason_codes.append("cross_artifact_proximity")
    if cross_artifact.get("counts", {}).get("low_confidence"):
        reason_codes.append("low_confidence_cross_artifact_bridge")
    if hotspots["changed_hubs"] or hotspots["changed_bridges"]:
        reason_codes.append("changed_hotspot")
    elif hotspots["impacted_hubs"] or hotspots["impacted_bridges"]:
        reason_codes.append("impacted_hotspot")
    if any(architecture_delta["counts"].values()):
        reason_codes.append("architecture_violation_in_changed_scope")
    if any(contract.get("status") == "warn" for contract in stability_contracts):
        reason_codes.append("stable_component_contract_gap")
    if any(
        "stable_component_low_test_density" in contract.get("reason_codes", [])
        for contract in stability_contracts
    ):
        reason_codes.append("stable_density_gap")

    affected_flow_rankings = [
        {
            "name": flow.get("name"),
            "criticality": flow.get("criticality", 0.0),
            "node_count": flow.get("node_count"),
            "file_count": flow.get("file_count"),
        }
        for flow in sorted(
            affected_flows,
            key=lambda item: float(item.get("criticality", 0.0) or 0.0),
            reverse=True,
        )[:10]
    ]

    signal_quality = _review_signal_quality(
        reason_codes,
        docs,
        test_gaps,
        stability_contracts,
    )
    guidance = _review_guidance_items(
        risk=risk,
        risk_score=risk_score,
        reason_codes=reason_codes,
        recommended_tests=recommended_tests,
        docs=docs,
        test_gap_ranking=test_gap_ranking,
        stability_contracts=stability_contracts,
        affected_flow_rankings=affected_flow_rankings,
        hotspots=hotspots,
        architecture_delta=architecture_delta,
        signal_quality=signal_quality,
        cross_artifact_proximity=cross_artifact,
    )

    return {
        "risk_level": risk,
        "risk_score": risk_score,
        "review_priority_score": risk_score,
        "score_semantics": {
            "risk_score": "legacy alias for review_priority_score",
            "review_priority_score": (
                "review triage ranking, not a standalone changeability metric"
            ),
        },
        "changed_node_count": len(impact.get("changed_nodes", [])),
        "impacted_node_count": len(impact.get("impacted_nodes", [])),
        "impacted_file_count": len(impact.get("impacted_files", [])),
        "reason_codes": reason_codes,
        "recommended_tests": recommended_tests,
        "affected_flow_rankings": affected_flow_rankings,
        "documentation_update_candidates": docs,
        "cross_artifact_proximity": cross_artifact,
        "test_gap_ranking": test_gap_ranking,
        "stability_contracts": stability_contracts,
        "signal_quality": signal_quality,
        "guidance": guidance,
        "hotspot_proximity": hotspots,
        "architecture_delta": architecture_delta,
        "next_drill_downs": {
            "impact_radius": {"tool": "review_tool", "mode": "impact"},
            "flows": {"tool": "review_tool", "mode": "affected_flows"},
            "review_context": {"tool": "review_tool", "mode": "context"},
            "architecture": {"tool": "architecture_analysis_tool", "mode": "overview"},
            "docs_for": {"tool": "query_graph_tool", "pattern": "docs_for"},
            "implementations_of": {
                "tool": "query_graph_tool",
                "pattern": "implementations_of",
            },
        },
    }
