"""Tools 4, 16: review context and detect-changes MCP implementations."""

from __future__ import annotations

import logging
from pathlib import Path

from ..changes import (  # noqa: F401
    analyze_changes,
    parse_diff_ranges,
    parse_diff_result,
    parse_git_diff_ranges,
)
from ..contracts.state_types import seal_missingness_item
from ..coverage import infer_tests_for_node
from ..hints import generate_hints, get_session
from ._common import (
    ToolPayload,
    ToolStoreScope,
    _error_response,
    _get_store,
    apply_output_budget,
    graph_answerability_summary,
    guidance_actions_to_hints,
    missingness_from_answerability,
)
from .review_context import get_review_context
from .review_helpers import (
    SUPPLEMENTAL_TEST_DENSITY_NODE_LIMIT,
    _change_analysis_summary,
    _resolve_changed_files,
)

logger = logging.getLogger(__name__)


def detect_changes_func(
    base: str = "HEAD~1",
    changed_files: list[str] | None = None,
    include_source: bool = False,
    max_depth: int = 2,
    repo_root: str | None = None,
    detail_level: str = "standard",
) -> ToolPayload:
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
    with ToolStoreScope(logger=logger, context="detect_changes") as scope:
        store, root = scope.track(_get_store(repo_root))
        answerability = graph_answerability_summary(store)
        missingness = missingness_from_answerability(answerability)
        changed_files, change_file_sources = _resolve_changed_files(root, base, changed_files)

        if not changed_files:
            return {
                "status": "ok",
                "summary": "No changed files detected.",
                "risk_score": 0.0,
                "changed_functions": [],
                "affected_flows": [],
                "test_gaps": [],
                "review_priorities": [],
                "answerability": answerability,
                "missingness": missingness,
            }

        abs_files = [str(root / f) for f in changed_files]

        diff_result = parse_diff_result(str(root), base)
        if diff_result.status == "base_unresolved":
            # The git diff failed; ``changed_files`` above is a worktree-wide
            # listing and every node-level count collapses to 0, which reads as
            # "no code changed". Only ``diff_parse_status`` disclosed it, so the
            # summary, status and missingness all looked clean.
            return _error_response(
                (
                    f"Could not resolve the diff base {base!r} in {root}. "
                    "Pass a reachable ref (the default HEAD~1 does not exist in a "
                    "single-commit repository, and a rebase or gc can make a "
                    "recorded sha unreachable)."
                ),
                status="error",
                base=base,
                diff_parse_status=diff_result.status,
                answerability=answerability,
                missingness=[
                    *missingness,
                    seal_missingness_item(
                        {
                            "reason_code": "diff_base_unreachable",
                            "severity": "high",
                            "claim_effect": (
                                "no diff could be computed, so nothing here describes what"
                                " actually changed"
                            ),
                        }
                    ),
                ],
            )
        abs_ranges: dict[str, list[tuple[int, int]]] = {}
        for rel_path, ranges in diff_result.ranges.items():
            abs_path = str(root / rel_path)
            abs_ranges[abs_path] = ranges

        analysis = analyze_changes(
            store,
            changed_files=abs_files,
            changed_ranges=abs_ranges,
            repo_root=str(root),
            base=base,
            include_heuristic_test_gap_evidence=True,
            heuristic_test_gap_node_limit=SUPPLEMENTAL_TEST_DENSITY_NODE_LIMIT,
            diff_parse_status=diff_result.status,
        )

        impact = store.get_impact_radius(abs_files, max_depth=max_depth)
        analysis_summary = _change_analysis_summary(
            store,
            analysis,
            impact,
            changed_files,
            detail_level=detail_level,
        )

        if include_source:
            for func in analysis.changed_functions:
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
            priorities = analysis.review_priorities
            top_priorities = [p.get("name", p.get("qualified_name", "")) for p in priorities[:3]]
            result: ToolPayload = {
                "status": "ok",
                "summary": analysis.summary,
                "risk_score": analysis.risk_score,
                "review_priority_score": analysis.review_priority_score,
                "score_semantics": (
                    analysis.score_semantics or analysis_summary.get("score_semantics", {})
                ),
                "risk_level": analysis_summary["risk_level"],
                "reason_codes": analysis_summary["reason_codes"],
                "changed_file_count": len(changed_files),
                "change_file_sources": change_file_sources,
                "change_entity_summary": analysis.change_entity_summary,
                "changed_node_count": analysis_summary["changed_node_count"],
                "impacted_node_count": analysis_summary["impacted_node_count"],
                "impacted_file_count": analysis_summary["impacted_file_count"],
                "test_gap_count": len(analysis.test_gaps),
                "test_gap_evidence": analysis.test_gap_evidence,
                "test_gap_ranking": analysis_summary["test_gap_ranking"],
                "signal_quality": analysis_summary["signal_quality"],
                "recommended_tests": analysis_summary["recommended_tests"][:5],
                "affected_flow_rankings": analysis_summary["affected_flow_rankings"][:5],
                "documentation_update_candidates": analysis_summary[
                    "documentation_update_candidates"
                ][:5],
                "stability_contracts": analysis_summary["stability_contracts"][:5],
                "guidance": analysis_summary["guidance"][:3],
                "architecture_delta": {
                    "mode": analysis_summary["architecture_delta"]["mode"],
                    "changed_scopes": analysis_summary["architecture_delta"]["changed_scopes"],
                    "counts": analysis_summary["architecture_delta"]["counts"],
                    "baseline_comparison": analysis_summary["architecture_delta"][
                        "baseline_comparison"
                    ],
                },
                "review_priorities": top_priorities,
                "next_drill_downs": analysis_summary["next_drill_downs"],
                "answerability": answerability,
                "missingness": missingness,
            }
        else:
            result = {
                "status": "ok",
                "changed_files": changed_files,
                "change_file_sources": change_file_sources,
                **analysis.model_dump(),
                "analysis_summary": analysis_summary,
                "answerability": answerability,
                "missingness": missingness,
            }
            apply_output_budget(
                result,
                budget_tokens=8000,
                list_priorities=[
                    "analysis_summary.recommended_tests",
                    "analysis_summary.affected_flow_rankings",
                    "analysis_summary.documentation_update_candidates",
                    "analysis_summary.stability_contracts",
                    "analysis_summary.guidance",
                    "review_priorities",
                    "affected_flows",
                    "test_gaps",
                    "changed_functions",
                ],
            )
        guidance = analysis_summary.get("guidance", [])
        hints = guidance_actions_to_hints(guidance if isinstance(guidance, list) else [])
        if not hints["next_steps"]:
            hints = generate_hints("detect_changes", result, get_session())
        result["_hints"] = hints
        return result
    return scope.error


__all__ = [
    "detect_changes_func",
    "get_review_context",
    "infer_tests_for_node",
]
