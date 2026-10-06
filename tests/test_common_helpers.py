from __future__ import annotations

import sqlite3
from types import SimpleNamespace
from typing import Any

import pytest

from dagayn.graph import GraphStore
from dagayn.graph.sqlite_errors import is_sqlite_corrupt_error
from dagayn.tools._common import (
    attach_answerability,
    graph_answerability_summary,
    guidance_actions_to_hints,
    handle_tool_runtime_error,
    make_guidance_item,
    make_response,
    missingness_from_answerability,
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
                "suggestion": "query_graph_tool callers_of -- inspect inbound callers",
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

    def test_answerability_uses_db_path_without_conn(self, tmp_path) -> None:
        graph_db = tmp_path / "graph.db"
        seed = GraphStore(graph_db)
        seed.close()

        class NativeLikeStore:
            db_path = graph_db

            def get_stats(self):
                sql_store = GraphStore(self.db_path)
                try:
                    return sql_store.get_stats()
                finally:
                    sql_store.close()

        native_store = NativeLikeStore()
        answerability = graph_answerability_summary(native_store, native_store.get_stats())
        assert answerability["reason_codes"] != ["no_sqlite_connection"]

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
        monkeypatch.setattr("dagayn.tools._common.repo_context_snapshot", lambda: None)
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
            {
                "tool": "query_graph_tool",
                "suggestion": "query_graph_tool callers_of -- inspect inbound callers",
            }
        ]
        assert hints["warnings"] == ["ambiguous_symbol"]


class TestProjectionForDetailLevel:
    ITEM = {"name": "foo", "size": 10, "lang": "py", "description": "bar", "extra": "baz"}


class TestSqliteCorruptHelpers:
    def test_detects_malformed_disk_image(self) -> None:
        err = sqlite3.DatabaseError("database disk image is malformed")
        assert is_sqlite_corrupt_error(err)

    def test_detects_torn_schema_message(self) -> None:
        err = RuntimeError("malformed database schema (skills)")
        assert is_sqlite_corrupt_error(err)

    def test_detects_both_wordings_of_notadb(self) -> None:
        # SQLite reports SQLITE_NOTADB with either wording depending on build;
        # the short form used to escape as a traceback out of the CLI.
        assert is_sqlite_corrupt_error(sqlite3.DatabaseError("file is not a database"))
        assert is_sqlite_corrupt_error(
            sqlite3.DatabaseError("file is encrypted or is not a database")
        )

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


class _Row:
    def __init__(self, value: int) -> None:
        self._value = value

    def fetchone(self) -> tuple[int]:
        return (self._value,)


class _HealthyConn:
    """Flows and communities present, nothing stale, configurable unresolved counts."""

    def __init__(self, *, unresolved_code_spans: int = 0, unresolved: int = 0) -> None:
        self.unresolved_code_spans = unresolved_code_spans
        self.unresolved = unresolved

    def execute(self, sql: str, params: tuple[Any, ...] = ()) -> _Row:
        if "code_span" in sql:
            return _Row(self.unresolved_code_spans)
        if "<unresolved:" in sql:
            return _Row(self.unresolved)
        if "flow_memberships" in sql or "community_id IS NULL" in sql:
            return _Row(0)
        return _Row(1)  # flows, communities


def _stats(**edges_by_kind: int) -> SimpleNamespace:
    return SimpleNamespace(
        total_nodes=10,
        files_count=2,
        languages=["python"],
        last_updated="2026-05-25T00:00:00",
        edges_by_kind={"TESTED_BY": 1, **edges_by_kind},
    )


class TestAnswerabilityFreshness:
    """A graph that answers for another commit or extractor must say so."""

    def _summary(self, freshness: dict[str, Any] | None) -> Any:
        store = SimpleNamespace(_conn=_HealthyConn())
        return graph_answerability_summary(store, _stats(), freshness=freshness)

    def test_fresh_graph_is_ok(self) -> None:
        summary = self._summary({"state": "commit_synced", "git_head_sha": "a"})
        assert summary["reason_codes"] == []
        assert summary["status"] == "ok"
        assert summary["score"] == 1.0

    def test_graph_of_another_commit(self) -> None:
        summary = self._summary(
            {"state": "commit_drift", "git_head_sha": "aaa", "current_head_sha": "bbb"}
        )
        assert summary["reason_codes"] == ["graph_describes_another_commit"]
        assert summary["score"] == 0.75
        assert summary["counts"]["graph_head_sha"] == "aaa"
        assert summary["counts"]["current_head_sha"] == "bbb"
        severities = {
            m["reason_code"]: m["severity"] for m in missingness_from_answerability(summary)
        }
        assert severities == {"graph_describes_another_commit": "high"}

    def test_extractor_drift_alone_is_not_reported_as_another_commit(self) -> None:
        """Same HEAD, older parser: the commit is right, the extraction is not."""
        summary = self._summary(
            {
                "state": "commit_drift",
                "extractor_drift": True,
                "git_head_sha": "aaa",
                "current_head_sha": "aaa",
            }
        )
        assert summary["reason_codes"] == ["graph_built_by_older_extractor"]
        assert summary["score"] == 0.75

    def test_dirty_worktree_and_older_extractor_both_count(self) -> None:
        summary = self._summary(
            {
                "state": "worktree_ahead",
                "worktree_dirty": True,
                "extractor_drift": True,
                "git_head_sha": "aaa",
                "current_head_sha": "aaa",
            }
        )
        assert summary["reason_codes"] == [
            "uncommitted_changes_may_be_unindexed",
            "graph_built_by_older_extractor",
        ]
        assert summary["score"] == 0.65
        assert summary["status"] == "degraded"
        assert summary["counts"]["worktree_dirty"] is True

    def test_unknown_freshness_state_adds_nothing(self) -> None:
        summary = self._summary({"state": None, "git_head_sha": "aaa"})
        assert summary["reason_codes"] == []
        assert "graph_head_sha" not in summary["counts"]

    def test_freshness_failure_never_breaks_the_summary(self, monkeypatch) -> None:
        def broken(*_args: object) -> dict[str, Any]:
            raise RuntimeError("git exploded")

        monkeypatch.setattr("dagayn.tools._common.commit_tier_freshness", broken)
        store = SimpleNamespace(_conn=_HealthyConn(), get_repo_root=lambda: "/repo")
        summary = graph_answerability_summary(store, _stats())
        assert summary["reason_codes"] == []
        assert summary["status"] == "ok"


class TestAnswerabilityCounts:
    def test_unreadable_stats_make_answerability_unknown(self) -> None:
        def get_stats() -> Any:
            raise sqlite3.OperationalError("no such table: nodes")

        summary = graph_answerability_summary(SimpleNamespace(get_stats=get_stats))
        assert summary["status"] == "unknown"
        assert summary["reason_codes"] == ["missing_graph_stats"]

    def test_unresolved_markdown_code_spans_are_not_counted_as_broken_links(self) -> None:
        """Code spans in prose are expected to stay unresolved; links are not."""
        mostly_spans = SimpleNamespace(_conn=_HealthyConn(unresolved_code_spans=6, unresolved=8))
        summary = graph_answerability_summary(mostly_spans, _stats(CROSS_ARTIFACT=10))
        # 10 edges - 6 spans = 4 reportable, of which 8 - 6 = 2 unresolved (50%).
        assert "many_unresolved_cross_artifact_edges" in summary["reason_codes"]
        assert dict(summary).get("unresolved_edges") == 2
        assert summary["counts"]["reportable_cross_artifact_edges"] == 4
        assert summary["counts"]["reportable_unresolved_cross_artifact_edges"] == 2

        only_spans = SimpleNamespace(_conn=_HealthyConn(unresolved_code_spans=6, unresolved=6))
        summary = graph_answerability_summary(only_spans, _stats(CROSS_ARTIFACT=10))
        assert "many_unresolved_cross_artifact_edges" not in summary["reason_codes"]
        assert "unresolved_edges" not in summary


class TestAttachAnswerability:
    _SUMMARY = {
        "status": "degraded",
        "score": 0.5,
        "reason_codes": ["missing_flows"],
        "parse": [1, 1, True],
    }

    def test_complete_payload_does_not_open_the_graph(self, monkeypatch) -> None:
        opened: list[object] = []

        def record_open(*args: object) -> Any:
            opened.append(args)
            raise ValueError("must not be opened")

        monkeypatch.setattr("dagayn.tools._common._get_store", record_open)
        payload: dict[str, Any] = {
            "status": "ok",
            "answerability": dict(self._SUMMARY),
            "missingness": [],
        }

        result = attach_answerability(payload, "/repo")

        assert opened == []
        assert result["answerability"]["reason_codes"] == ["missing_flows"]
        assert result["missingness"] == []
        assert "_runtime" in result

    def test_missingness_follows_the_tool_supplied_answerability(self, monkeypatch) -> None:
        closed: list[bool] = []

        class Store:
            _conn = _HealthyConn()

            def get_stats(self) -> SimpleNamespace:
                return _stats()

            def get_repo_root(self) -> None:
                return None

            def close(self) -> None:
                closed.append(True)

        monkeypatch.setattr("dagayn.tools._common._get_store", lambda _repo: (Store(), None))
        payload: dict[str, Any] = {"status": "ok", "answerability": dict(self._SUMMARY)}

        result = attach_answerability(payload, "/repo")

        assert closed == [True]
        assert result["answerability"]["reason_codes"] == ["missing_flows"]
        assert [m["reason_code"] for m in result["missingness"]] == ["missing_flows"]

    def test_unopenable_graph_reports_answerability_unavailable(self, monkeypatch) -> None:
        def refuse(_repo: object) -> Any:
            raise ValueError("repo_root does not look like a project root")

        monkeypatch.setattr("dagayn.tools._common._get_store", refuse)
        result = attach_answerability({"status": "ok"}, "/nowhere")
        assert result["answerability"]["reason_codes"] == ["answerability_unavailable"]
        assert result["missingness"][0]["reason_code"] == "answerability_unavailable"


class TestGuidanceActionsToHintsShapes:
    def test_mapping_actions_empty_actions_and_limit(self) -> None:
        hints = guidance_actions_to_hints(
            [
                {
                    "action": {
                        "tool": "query_graph_tool",
                        "command": "query_graph_tool tests_for X",
                    },
                    "missingness": {
                        "reason_code": "graph_describes_another_commit",
                        "severity": "high",
                    },
                },
                {"action": ""},
                {
                    "action": {"suggestion": "read the file by hand"},
                    "missingness": [
                        {"reason_code": "minor_gap", "severity": "low"},
                        {"reason_code": "missing_flows", "severity": "medium"},
                        {"severity": "high"},
                    ],
                },
                {"action": "review_tool(mode='changes') -- past the limit"},
            ],
            limit=2,
        )

        assert hints["next_steps"] == [
            {"tool": "query_graph_tool", "suggestion": "query_graph_tool tests_for X"},
            {"tool": "manual", "suggestion": "read the file by hand"},
        ]
        assert hints["warnings"] == ["graph_describes_another_commit", "missing_flows"]


class TestToolRuntimeErrors:
    def test_unexpected_exception_is_logged_with_traceback(self, caplog) -> None:
        import logging

        log = logging.getLogger("test.tools")
        with caplog.at_level(logging.WARNING, logger="test.tools"):
            try:
                raise RuntimeError("native panic")
            except RuntimeError as exc:
                payload = handle_tool_runtime_error(exc, logger=log, context="get_review")

        assert payload["status"] == "error"
        assert payload["error"] == "native panic"
        assert payload["missingness"][0]["reason_code"] == "unexpected_tool_failure"
        assert "file_ok" not in payload
        record = caplog.records[-1]
        assert record.levelno == logging.ERROR
        assert record.exc_info is not None

    def test_expected_exception_is_a_runtime_error_without_traceback(self, caplog) -> None:
        import logging

        log = logging.getLogger("test.tools")
        with caplog.at_level(logging.WARNING, logger="test.tools"):
            payload = handle_tool_runtime_error(KeyError("node"), logger=log, context="query_graph")

        assert payload["missingness"][0]["reason_code"] == "tool_runtime_error"
        assert caplog.records[-1].levelno == logging.WARNING
        assert caplog.records[-1].exc_info is None

    def test_recover_without_a_path_closes_everything_and_reports_ok(self) -> None:
        from dagayn.tools._common import recover_corrupt_graph

        assert recover_corrupt_graph() is True
        assert recover_corrupt_graph(":memory:") is True


class TestStoreOpenFailure:
    def test_failed_open_releases_the_read_lock(self, tmp_path) -> None:
        """A graph that cannot be opened must not leave writers locked out."""
        from dagayn.tools._common import _get_store
        from dagayn.write_lock import graph_lock_is_held, graph_write_lock

        (tmp_path / ".git").mkdir()
        db = tmp_path / ".dagayn" / "graph.db"
        db.mkdir(parents=True)  # a directory where the database file should be

        with pytest.raises(RuntimeError, match="unable to open database"):
            _get_store(str(tmp_path), cached=False)

        assert not graph_lock_is_held(db.resolve())
        with graph_write_lock(db.resolve(), blocking=False):
            pass

    def test_data_version_is_none_when_it_cannot_be_read(self, tmp_path) -> None:
        from dagayn.tools._common import _data_version

        assert _data_version(SimpleNamespace()) is None
        assert _data_version(SimpleNamespace(db_path=tmp_path / "missing" / "graph.db")) is None

        db = tmp_path / "graph.db"
        GraphStore(db).close()
        assert isinstance(_data_version(SimpleNamespace(db_path=db)), int)
