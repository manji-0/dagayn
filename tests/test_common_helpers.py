"""Unit tests for _common.py helpers: make_response, apply_output_budget, projection_for_detail_level."""

from __future__ import annotations

import pytest

from dagayn.tools._common import apply_output_budget, make_response, projection_for_detail_level


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
