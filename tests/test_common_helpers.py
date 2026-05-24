"""Unit tests for _common.py helpers.

Cover make_response, apply_output_budget, and projection_for_detail_level.
"""

from __future__ import annotations

from dagayn.tools._common import (
    apply_output_budget,
    compact_response,
    guidance_actions_to_hints,
    make_guidance_item,
    make_response,
    projection_for_detail_level,
)


class TestMakeResponse:
    def test_minimal(self) -> None:
        r = make_response("ok", "all good")
        assert r == {"status": "ok", "summary": "all good"}

    def test_extra_fields(self) -> None:
        r = make_response("ok", "done", count=3, items=["a", "b"])
        assert r["status"] == "ok"
        assert r["count"] == 3
        assert r["items"] == ["a", "b"]

    def test_hints(self) -> None:
        r = make_response("ok", "done", hints=["use X", "try Y"])
        assert r["_hints"] == ["use X", "try Y"]

    def test_next_tool_suggestions_truncated_at_3(self) -> None:
        r = make_response("ok", "done", next_tool_suggestions=["a", "b", "c", "d"])
        assert r["next_tool_suggestions"] == ["a", "b", "c"]

    def test_next_tool_suggestions_backfill_hints(self) -> None:
        r = make_response(
            "ok",
            "done",
            next_tool_suggestions=["query_graph callers_of -- inspect inbound callers"],
        )
        assert r["_hints"]["next_steps"] == [
            {
                "tool": "query_graph",
                "suggestion": "inspect inbound callers",
            }
        ]
        assert r["_hints"]["related"] == []
        assert r["_hints"]["warnings"] == []

    def test_empty_hints_not_included(self) -> None:
        r = make_response("ok", "done", hints=[])
        assert "_hints" not in r

    def test_status_before_summary_before_fields(self) -> None:
        r = make_response("ok", "msg", foo="bar")
        keys = list(r.keys())
        assert keys[0] == "status"
        assert keys[1] == "summary"


class TestApplyOutputBudget:
    def test_no_trimming_when_within_budget(self) -> None:
        payload = {"status": "ok", "items": ["a", "b"]}
        result = apply_output_budget(payload, budget_tokens=10000)
        assert result["items"] == ["a", "b"]
        assert "truncated" not in result

    def test_trims_lowest_priority_first(self) -> None:
        large = ["x" * 100] * 200
        payload = {
            "status": "ok",
            "high": ["important"] * 5,
            "low": large,
        }
        result = apply_output_budget(payload, budget_tokens=100, list_priorities=["high", "low"])
        assert len(result["low"]) < 200
        assert result["truncated"] is True
        assert "low" in result["_truncation"]

    def test_marks_truncated_true(self) -> None:
        payload = {"items": ["x" * 1000] * 100}
        result = apply_output_budget(payload, budget_tokens=50, list_priorities=["items"])
        assert result["truncated"] is True

    def test_truncation_metadata(self) -> None:
        payload = {"items": list(range(200))}
        result = apply_output_budget(payload, budget_tokens=50, list_priorities=["items"])
        assert "items" in result["_truncation"]
        assert result["_truncation"]["items"]["total"] == 200
        assert result["_truncation"]["items"]["kept"] < 200

    def test_ignores_non_list_fields(self) -> None:
        payload = {"status": "ok", "count": 42}
        result = apply_output_budget(payload, budget_tokens=1, list_priorities=["count"])
        assert result["count"] == 42

    def test_empty_priorities(self) -> None:
        payload = {"items": ["x" * 1000] * 100}
        result = apply_output_budget(payload, budget_tokens=1, list_priorities=[])
        assert "truncated" in result

    def test_trims_nested_list_fields(self) -> None:
        payload = {"analysis_summary": {"guidance": [{"blob": "x" * 100}] * 200}}
        result = apply_output_budget(
            payload,
            budget_tokens=100,
            list_priorities=["analysis_summary.guidance"],
        )
        assert result["truncated"] is True
        assert "analysis_summary.guidance" in result["_truncation"]
        assert len(result["analysis_summary"]["guidance"]) < 200


class TestGuidanceItems:
    def test_guidance_item_contract_snapshot(self) -> None:
        item = make_guidance_item(
            claim="Run focused tests before merging.",
            evidence={"type": "computed", "metric": "test_gap_count", "value": 2},
            confidence="high",
            missingness={"reason_code": "missing_test_edges", "severity": "medium"},
            action='pytest tests/test_tools.py -- run focused tool tests',
            reason_codes=["test_gaps"],
            counts={"test_gap_count": 2},
        )
        assert set(item) >= {
            "claim",
            "evidence",
            "confidence",
            "missingness",
            "action",
            "reason_codes",
            "counts",
        }
        assert item["confidence"] == "high"
        assert item["evidence"][0]["type"] == "computed"

    def test_guidance_actions_to_hints(self) -> None:
        hints = guidance_actions_to_hints(
            [
                make_guidance_item(
                    claim="Inspect callers.",
                    action="query_graph_tool callers_of -- inspect inbound callers",
                    missingness={"reason_code": "ambiguous_symbol", "severity": "high"},
                )
            ]
        )
        assert hints["next_steps"] == [
            {"tool": "query_graph_tool", "suggestion": "inspect inbound callers"}
        ]
        assert hints["warnings"] == ["ambiguous_symbol"]


class TestProjectionForDetailLevel:
    ITEM = {"name": "foo", "size": 10, "lang": "py", "description": "bar", "extra": "baz"}

    def test_minimal_returns_only_minimal_fields(self) -> None:
        r = projection_for_detail_level(self.ITEM, "minimal", ["name", "size"])
        assert set(r.keys()) == {"name", "size"}

    def test_standard_includes_minimal_and_standard_fields(self) -> None:
        r = projection_for_detail_level(
            self.ITEM, "standard", ["name", "size"], ["lang", "description"]
        )
        assert set(r.keys()) == {"name", "size", "lang", "description"}

    def test_standard_without_fields_standard_returns_all(self) -> None:
        r = projection_for_detail_level(self.ITEM, "standard", ["name"])
        assert r == dict(self.ITEM)

    def test_verbose_returns_all(self) -> None:
        r = projection_for_detail_level(self.ITEM, "verbose", ["name"])
        assert r == dict(self.ITEM)

    def test_missing_keys_are_skipped(self) -> None:
        r = projection_for_detail_level(self.ITEM, "minimal", ["name", "nonexistent"])
        assert "nonexistent" not in r
        assert r["name"] == "foo"


class TestCompactResponse:
    def test_top_flows_and_affected_flows_are_distinct(self) -> None:
        r = compact_response(
            summary="ok",
            top_flows=["checkout", "login", "search"],
            flows_affected=["login"],
        )
        assert r["top_flows"] == ["checkout", "login", "search"]
        assert r["flows_affected"] == ["login"]
