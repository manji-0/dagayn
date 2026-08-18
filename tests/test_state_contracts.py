from __future__ import annotations

import pytest
from pydantic import ValidationError

from dagayn.state_types import (
    AnswerabilitySummary,
    ArchitectureCommunityRequest,
    ChangeAnalysisResult,
    DroppedMarkdownArtifactResolution,
    EmbeddingCoverageStatus,
    FlowGetRequest,
    GuidanceItem,
    MissingnessItem,
    RefactorRenameRequest,
    ResolvedMarkdownArtifactResolution,
    StillUnresolvedMarkdownArtifactResolution,
    build_markdown_artifact_resolution,
    format_validation_error,
    parse_architecture_analysis_request,
    parse_flow_request,
    parse_refactor_request,
    parse_review_request,
    seal_answerability_summary,
    seal_dispatcher_error,
    seal_dispatcher_ok,
    seal_embedding_status,
    seal_guidance_item,
    seal_missingness_item,
    seal_reachability_info,
    seal_refactor_error,
    seal_refactor_not_found,
    seal_refactor_ok,
)


def test_resolved_markdown_artifact_resolution() -> None:
    resolution = build_markdown_artifact_resolution(
        state="resolved",
        edge_id=7,
        target_qualified="pkg.mod::fn",
        target_language="python",
        confidence=0.8,
        confidence_tier="HIGH",
        extra={"original_symbol_name": "fn"},
    )

    assert isinstance(resolution, ResolvedMarkdownArtifactResolution)
    assert resolution.state == "resolved"
    assert resolution.target_qualified == "pkg.mod::fn"


def test_implicit_drop_requires_edge_id_only() -> None:
    resolution = build_markdown_artifact_resolution(state="dropped", edge_id=3)

    assert isinstance(resolution, DroppedMarkdownArtifactResolution)
    assert resolution.target_qualified is None


def test_demoted_drop_requires_full_payload() -> None:
    with pytest.raises(ValidationError):
        build_markdown_artifact_resolution(
            state="dropped",
            edge_id=3,
            target_qualified="<unresolved:fn>",
            confidence=0.2,
        )


def test_still_unresolved_markdown_artifact_resolution() -> None:
    resolution = build_markdown_artifact_resolution(
        state="still_unresolved",
        edge_id=9,
        target_qualified="<unresolved:fn>",
        confidence=0.2,
        confidence_tier="LOW",
    )

    assert isinstance(resolution, StillUnresolvedMarkdownArtifactResolution)


def test_flow_get_request_requires_selector() -> None:
    with pytest.raises(ValidationError) as exc_info:
        parse_flow_request(mode="get")

    assert 'mode="get" requires flow_id or flow_name.' in format_validation_error(exc_info.value)


def test_flow_get_request_ignores_list_only_fields() -> None:
    request = parse_flow_request(
        mode="get",
        flow_id=4,
        sort_by="depth",
        limit=5,
    )

    assert isinstance(request, FlowGetRequest)
    assert request.flow_id == 4


def test_review_request_accepts_all_modes() -> None:
    for mode in ("changes", "context", "affected_flows", "impact"):
        request = parse_review_request(mode=mode, repo_root="/repo")
        assert request.mode == mode
        assert request.repo_root == "/repo"


def test_refactor_rename_request_requires_names() -> None:
    with pytest.raises(ValidationError):
        parse_refactor_request(mode="rename")


def test_refactor_dead_code_ignores_rename_fields() -> None:
    request = parse_refactor_request(
        mode="dead_code",
        old_name="ignored",
        new_name="ignored",
        limit=10,
    )

    assert request.mode == "dead_code"
    assert request.limit == 10


def test_refactor_rename_request_parses_names() -> None:
    request = parse_refactor_request(mode="rename", old_name="old", new_name="new")

    assert isinstance(request, RefactorRenameRequest)
    assert request.old_name == "old"
    assert request.new_name == "new"


def test_dispatcher_envelopes_allow_extra_metadata() -> None:
    ok = seal_dispatcher_ok(
        {
            "status": "ok",
            "mode": "list",
            "called_subtool": "list_flows",
            "summary": "done",
            "answerability": {"status": "ok"},
        }
    )
    error = seal_dispatcher_error(
        {
            "status": "error",
            "mode": "get",
            "called_subtool": None,
            "summary": "bad",
            "error": "bad",
            "answerability": {"status": "ok"},
        }
    )

    assert ok["status"] == "ok"
    assert error["status"] == "error"
    assert "answerability" in ok
    assert "answerability" in error


def test_change_analysis_records_keep_typed_fields_and_extensions() -> None:
    result = ChangeAnalysisResult.model_validate(
        {
            "changed_functions": [
                {
                    "name": "run",
                    "qualified_name": "pkg::run",
                    "file_path": "src/pkg.py",
                    "line_start": 1,
                    "line_end": 2,
                    "risk_score": 0.8,
                    "payload": "forward-compatible",
                }
            ],
            "changed_edges": [
                {
                    "source": "pkg::run",
                    "target": "pkg::load",
                    "change_status": "added",
                }
            ],
            "change_entity_summary": {
                "nodes": {"existing": 1, "added": 0, "unknown": 0},
                "edges": {"existing": 0, "added": 1, "unknown": 0},
            },
            "affected_flows": [
                {
                    "name": "main",
                    "steps": [
                        {
                            "name": "run",
                            "qualified_name": "pkg::run",
                            "node_id": 1,
                        }
                    ],
                }
            ],
            "test_gaps": [{"name": "run", "coverage_confidence": "none"}],
            "test_gap_evidence": {"direct_tested_by_edges": True},
            "review_priorities": [{"name": "run", "risk_score": 0.8}],
        }
    )

    assert result.model_dump()["changed_functions"][0]["payload"] == "forward-compatible"
    assert result.changed_edges[0]["change_status"] == "added"
    assert result.affected_flows[0]["steps"][0]["node_id"] == 1
    assert result.test_gaps[0]["coverage_confidence"] == "none"


def test_architecture_community_request_requires_selector() -> None:
    with pytest.raises(ValidationError) as exc_info:
        parse_architecture_analysis_request(mode="community")

    assert 'mode="community" requires community_id or community_name.' in format_validation_error(
        exc_info.value
    )


def test_architecture_community_request_parses_selector() -> None:
    request = parse_architecture_analysis_request(
        mode="community",
        community_name="auth",
        include_members=True,
    )

    assert isinstance(request, ArchitectureCommunityRequest)
    assert request.community_name == "auth"
    assert request.include_members is True


def test_architecture_request_accepts_all_modes() -> None:
    modes = (
        "overview",
        "communities",
        "community",
        "hubs",
        "bridges",
        "knowledge_gaps",
        "surprising_connections",
        "adp_violations",
        "sdp_metrics",
        "sdp_violations",
        "sap_metrics",
        "sap_violations",
    )
    for mode in modes:
        payload: dict[str, object] = {"mode": mode, "repo_root": "/repo"}
        if mode == "community":
            payload["community_id"] = 1
        request = parse_architecture_analysis_request(**payload)
        assert request.mode == mode
        assert request.repo_root == "/repo"


def test_architecture_request_rejects_unknown_dependency_profile() -> None:
    with pytest.raises(ValidationError) as exc_info:
        parse_architecture_analysis_request(mode="sdp_metrics", dependency_profile="typo")

    assert "Unknown dependency_profile" in format_validation_error(exc_info.value)


def test_guidance_item_normalizes_boundary_fields() -> None:
    item = seal_guidance_item(
        {
            "claim": "Inspect impact before merging.",
            "evidence": {"type": "typo", "metric": "risk", "value": 0.7},
            "confidence": "certain",
            "missingness": {"reason_code": "missing_tests", "severity": "severe"},
            "action": {"tool": "review_tool", "suggestion": "inspect impact"},
            "reason_codes": ["risk"],
            "counts": {"changed_files": 2},
        }
    )

    assert isinstance(GuidanceItem.model_validate(item), GuidanceItem)
    assert item["evidence"][0]["type"] == "computed"
    assert item["confidence"] == "unknown"
    assert item["missingness"][0]["severity"] == "low"


def test_answerability_and_missingness_contracts_allow_extra_metadata() -> None:
    answerability = seal_answerability_summary(
        {
            "status": "degraded",
            "score": 0.5,
            "reason_codes": ["missing_flows"],
            "parse": [10, 2, True],
            "answerability": [0, 1, 2, 3, 0.0],
        }
    )
    missingness = seal_missingness_item(
        {
            "reason_code": "missing_flows",
            "severity": "medium",
            "claim_effect": "flow claims are incomplete",
            "source": "answerability",
        }
    )

    assert isinstance(AnswerabilitySummary.model_validate(answerability), AnswerabilitySummary)
    assert isinstance(MissingnessItem.model_validate(missingness), MissingnessItem)
    assert answerability["answerability"] == [0, 1, 2, 3, 0.0]
    assert missingness["source"] == "answerability"


def test_embedding_status_contract_distinguishes_coverage_states() -> None:
    complete = seal_embedding_status(
        {
            "status": "complete",
            "total_embeddings": 1,
            "provider_counts": {"local:test": 1},
            "embeddable_nodes": 1,
            "indexed_embeddings": 1,
            "missing_embeddings": 0,
            "orphan_embeddings": 0,
        }
    )
    unavailable = seal_embedding_status(
        {
            "status": "unavailable",
            "total_embeddings": 0,
            "provider_counts": {},
            "error": "database is missing",
        }
    )

    assert isinstance(EmbeddingCoverageStatus.model_validate(complete), EmbeddingCoverageStatus)
    assert complete["status"] == "complete"
    assert unavailable["status"] == "unavailable"
    assert unavailable["error"] == "database is missing"


def test_reachability_contract_enforces_state_shape() -> None:
    not_found = seal_reachability_info(
        {"state": "not_found", "truncated": False, "max_depth": 3, "nodes_visited": 0}
    )
    truncated = seal_reachability_info(
        {"state": "truncated", "truncated": True, "max_depth": 3, "nodes_visited": 4}
    )

    assert not_found == {
        "state": "not_found",
        "truncated": False,
        "max_depth": 3,
        "nodes_visited": 0,
    }
    assert truncated["state"] == "truncated"
    assert truncated["truncated"] is True
    with pytest.raises(ValidationError):
        seal_reachability_info(
            {"state": "not_found", "truncated": True, "max_depth": 3, "nodes_visited": 0}
        )


def test_refactor_envelopes_allow_extra_metadata() -> None:
    ok = seal_refactor_ok(
        {
            "status": "ok",
            "summary": "done",
            "answerability": {"status": "ok"},
            "dead_code": [],
        }
    )
    error = seal_refactor_error({"status": "error", "error": "bad input"})
    not_found = seal_refactor_not_found(
        {
            "status": "not_found",
            "summary": "missing symbol",
            "missingness": [],
        }
    )

    assert ok["status"] == "ok"
    assert error["status"] == "error"
    assert error["summary"] == "bad input"
    assert not_found["status"] == "not_found"
    assert "answerability" in ok
    assert "missingness" in not_found
