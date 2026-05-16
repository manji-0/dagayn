"""Tools 4, 12, 16: review context, affected flows, detect changes."""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Any

from ..changes import analyze_changes, parse_diff_ranges, parse_git_diff_ranges  # noqa: F401
from ..coverage import infer_tests_for_node
from ..flows import get_affected_flows as _get_affected_flows
from ..graph import edge_to_dict, node_to_dict
from ..hints import generate_hints, get_session
from ..incremental import get_changed_files, get_staged_and_unstaged
from ._common import _get_store, apply_output_budget

logger = logging.getLogger(__name__)


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


def _dedupe_dicts_by_key(items: list[dict[str, Any]], key: str, limit: int) -> list[dict[str, Any]]:
    seen: set[str] = set()
    out: list[dict[str, Any]] = []
    for item in items:
        value = item.get(key)
        if not isinstance(value, str) or not value or value in seen:
            continue
        seen.add(value)
        out.append(item)
        if len(out) >= limit:
            break
    return out


def _classify_test_gap(gap: dict[str, Any]) -> str:
    file_path = str(gap.get("file", ""))
    language = str(gap.get("language", ""))
    if _is_markdown_path(file_path) or language == "markdown":
        return "documentation"
    normalized = file_path.replace("\\", "/")
    if normalized.startswith("tests/") or "/tests/" in normalized or gap.get("kind") == "Test":
        return "test_artifact"
    return "actionable"


def _rank_test_gaps(test_gaps: list[dict[str, Any]], *, limit: int = 5) -> dict[str, Any]:
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


def _review_signal_quality(
    reason_codes: list[str],
    docs: list[dict[str, Any]],
    test_gaps: list[dict[str, Any]],
) -> dict[str, Any]:
    uncertain = []
    if docs:
        uncertain.append("documentation candidates are graph-reachable markdown nodes")
    if test_gaps:
        uncertain.append(
            "test gaps use direct TESTED_BY edges plus medium-confidence naming/source heuristics"
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
            }
        ],
        "heuristics": [
            code
            for code in reason_codes
            if code in {"test_gaps", "documentation_update_candidates"}
        ],
        "uncertain": uncertain,
    }


def _recommend_tests(
    store: Any,
    changed_functions: list[dict[str, Any]],
    affected_flows: list[dict[str, Any]],
    *,
    limit: int = 10,
) -> list[dict[str, Any]]:
    """Recommend tests that directly or indirectly cover changed code."""
    recommendations: list[dict[str, Any]] = []

    for func in changed_functions:
        qualified_name = func.get("qualified_name")
        if not isinstance(qualified_name, str) or not qualified_name:
            continue
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
            }
        )

    return _dedupe_dicts_by_key(recommendations, "qualified_name", limit)


def _documentation_update_candidates(
    impact: dict[str, Any],
    changed_files: list[str],
    *,
    limit: int = 10,
) -> list[dict[str, Any]]:
    changed_set = {str(path) for path in changed_files}
    code_changed = any(not _is_markdown_path(path) for path in changed_files)
    if not code_changed:
        return []

    candidates: list[dict[str, Any]] = []
    for node in impact.get("impacted_nodes", []):
        file_path = getattr(node, "file_path", "")
        if not isinstance(file_path, str) or not _is_markdown_path(file_path):
            continue
        if file_path in changed_set:
            continue
        candidates.append(
            {
                "file": file_path,
                "section": getattr(node, "name", None),
                "qualified_name": getattr(node, "qualified_name", None),
                "reason": "markdown node reached from changed code/doc graph",
            }
        )

    return _dedupe_dicts_by_key(candidates, "qualified_name", limit)


def _hotspot_proximity(
    store: Any,
    impact: dict[str, Any],
    *,
    top_n: int = 25,
    limit: int = 5,
) -> dict[str, Any]:
    try:
        from ..analysis import find_bridge_nodes, find_hub_nodes

        hubs = find_hub_nodes(store, top_n=top_n)
        bridges = find_bridge_nodes(store, top_n=top_n)
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

    def _matches(items: list[dict[str, Any]], qns: set[str]) -> list[dict[str, Any]]:
        return [item for item in items if item.get("qualified_name") in qns][:limit]

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


def _architecture_delta_summary(
    store: Any,
    changed_files: list[str],
    *,
    limit: int = 5,
) -> dict[str, Any]:
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

        adp = find_adp_violations(store, granularity="package")
        sdp = find_sdp_violations(store, granularity="package")
        sap = find_sap_violations(store, scope_kind="package")
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


def _change_analysis_summary(
    store: Any,
    analysis: dict[str, Any],
    impact: dict[str, Any],
    changed_files: list[str],
) -> dict[str, Any]:
    risk_score = float(analysis.get("risk_score", 0.0) or 0.0)
    affected_flows = list(analysis.get("affected_flows", []))
    test_gaps = list(analysis.get("test_gaps", []))
    changed_functions = list(analysis.get("changed_functions", []))
    risk = _risk_level(risk_score)

    docs = _documentation_update_candidates(impact, changed_files)
    hotspots = _hotspot_proximity(store, impact)
    architecture_delta = _architecture_delta_summary(store, changed_files)
    recommended_tests = _recommend_tests(store, changed_functions, affected_flows)
    test_gap_ranking = _rank_test_gaps(test_gaps)

    reason_codes: list[str] = []
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
    if hotspots["changed_hubs"] or hotspots["changed_bridges"]:
        reason_codes.append("changed_hotspot")
    elif hotspots["impacted_hubs"] or hotspots["impacted_bridges"]:
        reason_codes.append("impacted_hotspot")
    if any(architecture_delta["counts"].values()):
        reason_codes.append("architecture_violation_in_changed_scope")

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

    return {
        "risk_level": risk,
        "risk_score": risk_score,
        "changed_node_count": len(impact.get("changed_nodes", [])),
        "impacted_node_count": len(impact.get("impacted_nodes", [])),
        "impacted_file_count": len(impact.get("impacted_files", [])),
        "reason_codes": reason_codes,
        "recommended_tests": recommended_tests,
        "affected_flow_rankings": affected_flow_rankings,
        "documentation_update_candidates": docs,
        "test_gap_ranking": test_gap_ranking,
        "signal_quality": _review_signal_quality(reason_codes, docs, test_gaps),
        "hotspot_proximity": hotspots,
        "architecture_delta": architecture_delta,
        "next_drill_downs": {
            "impact_radius": {"tool": "review_tool", "mode": "impact"},
            "flows": {"tool": "review_tool", "mode": "affected_flows"},
            "review_context": {"tool": "review_tool", "mode": "context"},
            "architecture": {"tool": "architecture_analysis_tool", "mode": "overview"},
        },
    }


# ---------------------------------------------------------------------------
# Tool 4: get_review_context
# ---------------------------------------------------------------------------


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
    store, root = _get_store(repo_root)
    try:
        # Get impact radius first
        if changed_files is None:
            changed_files = get_changed_files(root, base)
            if not changed_files:
                changed_files = get_staged_and_unstaged(root)

        if not changed_files:
            return {
                "status": "ok",
                "summary": "No changes detected. Nothing to review.",
                "context": {},
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
            tested_qualified = {e.target_qualified for e in test_edges}
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
                "impacted_file_count": len(impact["impacted_files"]),
                "key_entities": key_entities,
                "test_gaps": test_gap_count,
                "next_tool_suggestions": [
                    'review_tool mode="changes"',
                    'review_tool mode="affected_flows"',
                    'review_tool mode="impact"',
                ],
            }

        # Build review context
        context: dict[str, Any] = {
            "changed_files": changed_files,
            "impacted_files": impact["impacted_files"],
            "graph": {
                "changed_nodes": [node_to_dict(n) for n in impact["changed_nodes"]],
                "impacted_nodes": [node_to_dict(n) for n in impact["impacted_nodes"]],
                "edges": [edge_to_dict(e) for e in impact["edges"]],
            },
        }

        # Add source snippets for changed files
        if include_source:
            snippets = {}
            for rel_path in changed_files:
                full_path = root / rel_path
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
        }
    finally:
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
    tested_funcs = {e.target_qualified for e in test_edges}

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


# ---------------------------------------------------------------------------
# Tool 12: get_affected_flows  [REVIEW]
# ---------------------------------------------------------------------------


def get_affected_flows_func(
    changed_files: list[str] | None = None,
    base: str = "HEAD~1",
    repo_root: str | None = None,
) -> dict[str, Any]:
    """Find execution flows affected by changed files.

    [REVIEW] Identifies which execution flows pass through nodes in the
    changed files.  Useful during code review to understand which user-facing
    or critical paths are affected by a change.

    Args:
        changed_files: List of changed file paths (relative to repo root).
                       Auto-detected from git diff if omitted.
        base: Git ref for auto-detecting changes (default: HEAD~1).
        repo_root: Repository root path. Auto-detected if omitted.

    Returns:
        Affected flows sorted by criticality, with step details.
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
                "affected_flows": [],
                "total": 0,
            }

        # Convert to absolute paths for graph lookup
        abs_files = [str(root / f) for f in changed_files]
        result = _get_affected_flows(store, abs_files)

        total = result["total"]
        out = {
            "status": "ok",
            "summary": (f"{total} flow(s) affected by changes in {len(changed_files)} file(s)"),
            "changed_files": changed_files,
            "affected_flows": result["affected_flows"],
            "total": total,
        }
        out["_hints"] = generate_hints("get_affected_flows", out, get_session())
        return out
    except Exception as exc:
        return {"status": "error", "error": str(exc)}
    finally:
        store.close()


# ---------------------------------------------------------------------------
# Tool 16: detect_changes  [REVIEW]
# ---------------------------------------------------------------------------


def detect_changes_func(
    base: str = "HEAD~1",
    changed_files: list[str] | None = None,
    include_source: bool = False,
    max_depth: int = 2,
    repo_root: str | None = None,
    detail_level: str = "standard",
) -> dict[str, Any]:
    """Detect changes and produce risk-scored review guidance.

    [REVIEW] Primary tool for code review.  Maps git diffs to affected
    functions, flows, communities, and test coverage gaps.  Returns
    priority-ordered review guidance with risk scores.

    Args:
        base: Git ref to diff against (default: HEAD~1).
        changed_files: Explicit list of changed file paths (relative to repo
            root).  Auto-detected from git diff if omitted.
        include_source: If True, include source code snippets for changed
            functions.  Default: False.
        max_depth: Impact radius depth for BFS traversal.  Default: 2.
        repo_root: Repository root path.  Auto-detected if omitted.
        detail_level: Output detail level.  "standard" returns full analysis;
            "minimal" returns only summary, risk_score, changed_file_count,
            test_gap_count, and top 3 review priorities (text only).
            Default: "standard".

    Returns:
        Risk-scored analysis with changed functions, affected flows,
        test gaps, and review priorities.
    """
    store, root = _get_store(repo_root)
    try:
        # Detect changed files if not provided.
        if changed_files is None:
            changed_files = get_changed_files(root, base)
            if not changed_files:
                changed_files = get_staged_and_unstaged(root)

        if not changed_files:
            return {
                "status": "ok",
                "summary": "No changed files detected.",
                "risk_score": 0.0,
                "changed_functions": [],
                "affected_flows": [],
                "test_gaps": [],
                "review_priorities": [],
            }

        # Convert to absolute paths for graph lookup.
        abs_files = [str(root / f) for f in changed_files]

        # Parse diff ranges for line-level mapping.
        diff_ranges = parse_diff_ranges(str(root), base)
        # Remap to absolute paths so they match graph file_paths.
        abs_ranges: dict[str, list[tuple[int, int]]] = {}
        for rel_path, ranges in diff_ranges.items():
            abs_path = str(root / rel_path)
            abs_ranges[abs_path] = ranges

        analysis = analyze_changes(
            store,
            changed_files=abs_files,
            changed_ranges=abs_ranges if abs_ranges else None,
            repo_root=str(root),
            base=base,
        )

        impact = store.get_impact_radius(abs_files, max_depth=max_depth)
        analysis_summary = _change_analysis_summary(
            store,
            analysis,
            impact,
            changed_files,
        )

        # Optionally include source snippets for changed functions.
        if include_source:
            for func in analysis.get("changed_functions", []):
                fp = func.get("file_path")
                ls = func.get("line_start")
                le = func.get("line_end")
                if fp and ls and le:
                    file_path = Path(fp)
                    if not file_path.is_absolute():
                        file_path = root / file_path
                    if file_path.is_file():
                        try:
                            lines = file_path.read_text(errors="replace").splitlines()
                            start = max(0, ls - 1)
                            end = min(len(lines), le)
                            func["source"] = "\n".join(
                                f"{i + 1}: {lines[i]}" for i in range(start, end)
                            )
                        except (OSError, UnicodeDecodeError):
                            func["source"] = "(could not read file)"

        if detail_level == "minimal":
            priorities = analysis.get("review_priorities", [])
            top_priorities = [p.get("name", p.get("qualified_name", "")) for p in priorities[:3]]
            result: dict[str, Any] = {
                "status": "ok",
                "summary": analysis.get("summary", ""),
                "risk_score": analysis.get("risk_score", 0.0),
                "risk_level": analysis_summary["risk_level"],
                "reason_codes": analysis_summary["reason_codes"],
                "changed_file_count": len(changed_files),
                "changed_node_count": analysis_summary["changed_node_count"],
                "impacted_node_count": analysis_summary["impacted_node_count"],
                "impacted_file_count": analysis_summary["impacted_file_count"],
                "test_gap_count": len(analysis.get("test_gaps", [])),
                "test_gap_ranking": analysis_summary["test_gap_ranking"],
                "signal_quality": analysis_summary["signal_quality"],
                "recommended_tests": analysis_summary["recommended_tests"][:5],
                "affected_flow_rankings": analysis_summary["affected_flow_rankings"][:5],
                "documentation_update_candidates": analysis_summary[
                    "documentation_update_candidates"
                ][:5],
                "review_priorities": top_priorities,
                "next_drill_downs": analysis_summary["next_drill_downs"],
            }
        else:
            result = {
                "status": "ok",
                "changed_files": changed_files,
                **analysis,
                "analysis_summary": analysis_summary,
            }
            apply_output_budget(
                result,
                budget_tokens=8000,
                list_priorities=[
                    "analysis_summary.recommended_tests",
                    "analysis_summary.affected_flow_rankings",
                    "analysis_summary.documentation_update_candidates",
                    "review_priorities",
                    "affected_flows",
                    "test_gaps",
                    "changed_functions",
                ],
            )
        result["_hints"] = generate_hints("detect_changes", result, get_session())
        return result
    except Exception as exc:
        return {"status": "error", "error": str(exc)}
    finally:
        store.close()
