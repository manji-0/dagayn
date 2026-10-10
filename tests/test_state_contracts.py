from __future__ import annotations

import pytest
from pydantic import TypeAdapter, ValidationError

from dagayn.contracts.state_types import (
    AnswerabilitySummary,
    ChangeEdgeRecord,
    ChangeNodeRecord,
    EmbeddingCoverageStatus,
    GuidanceItem,
    MissingnessItem,
    RefactorRenameRequest,
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
)


def test_flow_request_accepts_only_entry_points() -> None:
    assert parse_flow_request(target="run").mode == "entry_points"
    with pytest.raises(ValidationError):
        parse_flow_request(mode="get")


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


def test_change_records_keep_typed_fields_and_extensions() -> None:
    node = TypeAdapter(ChangeNodeRecord).validate_python(
        {
            "name": "run",
            "qualified_name": "pkg::run",
            "file_path": "src/pkg.py",
            "line_start": 1,
            "line_end": 2,
            "risk_score": 0.8,
            "payload": "forward-compatible",
        }
    )
    edge = TypeAdapter(ChangeEdgeRecord).validate_python(
        {"source": "pkg::run", "target": "pkg::load", "change_status": "added"}
    )

    assert dict(node)["payload"] == "forward-compatible"
    assert node["risk_score"] == 0.8
    assert edge["change_status"] == "added"
    with pytest.raises(ValidationError):
        TypeAdapter(ChangeEdgeRecord).validate_python({"change_status": "renamed"})


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


def test_refactor_error_envelope_allows_extra_metadata() -> None:
    error = seal_refactor_error(
        {"status": "error", "error": "bad input", "answerability": {"status": "ok"}}
    )

    assert error["status"] == "error"
    assert error["summary"] == "bad input"
    assert "answerability" in error
