from __future__ import annotations

import pytest
from pydantic import ValidationError

from dagayn.state_types import (
    DroppedMarkdownArtifactResolution,
    FlowGetRequest,
    RefactorRenameRequest,
    ResolvedMarkdownArtifactResolution,
    StillUnresolvedMarkdownArtifactResolution,
    build_markdown_artifact_resolution,
    format_validation_error,
    parse_flow_request,
    parse_refactor_request,
    parse_review_request,
    seal_dispatcher_error,
    seal_dispatcher_ok,
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
