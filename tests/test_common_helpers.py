"""Unit tests for _common.py helpers.

Cover make_response, apply_output_budget, and projection_for_detail_level.
"""

from __future__ import annotations

import sqlite3
from types import SimpleNamespace
from typing import Any

from dagayn.graph import GraphStore
from dagayn.graph.sqlite_errors import is_sqlite_corrupt_error
from dagayn.tools._common import (
    apply_output_budget,
    attach_answerability,
    compact_response,
    graph_answerability_summary,
    guidance_actions_to_hints,
    handle_tool_runtime_error,
    make_guidance_item,
    make_response,
    missingness_from_answerability,
    projection_for_detail_level,
    tool_runtime_summary,
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
            next_tool_suggestions=["query_graph_tool callers_of -- inspect inbound callers"],
        )
        assert r["_hints"]["next_steps"] == [
            {
                "tool": "query_graph_tool",
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

    def test_updates_payload_while_estimating_trim_size(self) -> None:
        payload = {"items": ["x" * 100] * 8}
        result = apply_output_budget(payload, budget_tokens=120, list_priorities=["items"])
        assert result["_truncation"]["items"]["kept"] > 1


class TestAnswerability:
    def test_sqlite_errors_degrade_instead_of_raising(self) -> None:
        class BrokenConn:
            def execute(self, *_args, **_kwargs):
                raise sqlite3.OperationalError("no such table")

        store = SimpleNamespace(_conn=BrokenConn())
        stats = SimpleNamespace(
            total_nodes=3,
            files_count=1,
            languages=["python"],
            last_updated="2026-05-25T00:00:00",
            edges_by_kind={"TESTED_BY": 0, "CROSS_ARTIFACT": 2},
        )

        answerability = graph_answerability_summary(store, stats)
        assert answerability["status"] == "degraded"
        assert "missing_flows_table" in answerability["reason_codes"]
        assert "missing_communities_table" in answerability["reason_codes"]
        missingness = missingness_from_answerability(answerability)
        assert {item["reason_code"] for item in missingness} >= {
            "missing_flows_table",
            "missing_communities_table",
        }

    def test_stale_derived_structures_downgrades_answerability(self) -> None:
        class _Row:
            def __init__(self, value: int) -> None:
                self._value = value

            def fetchone(self):
                return (self._value,)

        class Conn:
            def execute(self, sql: str, params: tuple[Any, ...] = ()):
                if "FROM flows" in sql and "flow_memberships" not in sql:
                    return _Row(2)
                if "FROM communities" in sql and "nodes" not in sql:
                    return _Row(1)
                if "flow_memberships fm" in sql:
                    return _Row(3)
                if "community_id IS NULL" in sql:
                    return _Row(4)
                if "TESTED_BY" in sql or "CROSS_ARTIFACT" in sql:
                    return _Row(0)
                return _Row(0)

        store = SimpleNamespace(_conn=Conn())
        stats = SimpleNamespace(
            total_nodes=10,
            files_count=2,
            languages=["python"],
            last_updated="2026-05-25T00:00:00",
            edges_by_kind={"TESTED_BY": 1, "CROSS_ARTIFACT": 0},
        )

        answerability = graph_answerability_summary(store, stats)

        assert "stale_derived_structures" in answerability["reason_codes"]
        assert answerability["counts"]["stale_flow_memberships"] == 3
        assert answerability["counts"]["unassigned_nodes"] == 4
        assert answerability["score"] < 0.9
        missingness = missingness_from_answerability(answerability)
        assert any(item["reason_code"] == "stale_derived_structures" for item in missingness)

    def test_attach_answerability_preserves_existing_missingness(self, monkeypatch) -> None:
        class Store:
            def get_stats(self):
                return SimpleNamespace(
                    total_nodes=1,
                    files_count=1,
                    languages=["python"],
                    last_updated="2026-05-25T00:00:00",
                    edges_by_kind={},
                )

            _conn = None

            def close(self):
                pass

        monkeypatch.setattr("dagayn.tools._common._get_store", lambda _repo: (Store(), None))
        payload: dict[str, Any] = {"status": "ok", "summary": "x", "missingness": []}

        result = attach_answerability(payload, "/repo")

        assert result is payload
        assert result["answerability"]["reason_codes"] == ["no_sqlite_connection"]
        assert result["missingness"] == []
        assert result["_runtime"]["package"] == "dagayn"
        assert result["_runtime"]["package_root"]

    def test_tool_runtime_summary_identifies_process_and_package(self) -> None:
        runtime = tool_runtime_summary()

        assert runtime["package"] == "dagayn"
        assert isinstance(runtime["pid"], int)
        assert runtime["python"]
        assert runtime["package_root"].endswith("dagayn")


class TestGuidanceItems:
    def test_guidance_item_contract_snapshot(self) -> None:
        item = make_guidance_item(
            claim="Run focused tests before merging.",
            evidence={"type": "computed", "metric": "test_gap_count", "value": 2},
            confidence="high",
            missingness={"reason_code": "missing_test_edges", "severity": "medium"},
            action="pytest tests/test_tools.py -- run focused tool tests",
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

    def test_guidance_item_normalizes_invalid_values_via_contract(self) -> None:
        item = make_guidance_item(
            claim="Inspect evidence.",
            evidence={"type": "unsupported", "value": 1},
            confidence="certain",
            missingness={"reason_code": "gap", "severity": "severe"},
            action={"tool": "review_tool", "suggestion": "inspect context"},
        )

        assert item["evidence"][0]["type"] == "computed"
        assert item["confidence"] == "unknown"
        assert item["missingness"][0]["severity"] == "low"
        assert item["action"]["tool"] == "review_tool"

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


class TestSqliteCorruptHelpers:
    def test_detects_malformed_disk_image(self) -> None:
        err = sqlite3.DatabaseError("database disk image is malformed")
        assert is_sqlite_corrupt_error(err)

    def test_detects_torn_schema_message(self) -> None:
        err = RuntimeError("malformed database schema (skills)")
        assert is_sqlite_corrupt_error(err)

    def test_ignores_unrelated_errors(self) -> None:
        assert not is_sqlite_corrupt_error(sqlite3.OperationalError("database is locked"))
        assert not is_sqlite_corrupt_error(ValueError("nope"))

    def test_handle_tool_runtime_error_recovers_corrupt(self, tmp_path, monkeypatch) -> None:
        import logging

        (tmp_path / ".git").mkdir()
        (tmp_path / ".dagayn").mkdir()
        GraphStore(tmp_path / ".dagayn" / "graph.db").close()
        monkeypatch.chdir(tmp_path)

        payload = handle_tool_runtime_error(
            sqlite3.DatabaseError("database disk image is malformed"),
            logger=logging.getLogger("test"),
            context="query_graph",
            repo_root=str(tmp_path),
        )
        assert payload["status"] == "error"
        assert payload["missingness"][0]["reason_code"] == "sqlite_corrupt"
        assert payload["file_ok"] is True
        assert "Restart" in payload["next_action"] or "restart" in payload["next_action"]
