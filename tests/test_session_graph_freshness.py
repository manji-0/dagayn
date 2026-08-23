"""Guarantee tests for session / worktree / Subagent graph freshness.

Use-case catalog: docs/SESSION-GRAPH-FRESHNESS.md

| ID    | Covered by |
| ----- | ---------- |
| UC-S1 | hook wiring (Claude/Cursor/OpenCode) + prepare on drift |
| UC-S2 | dirty prepare (structure-ready) + resume noop when ready |
| UC-H1 | HEAD relocate prepare + OpenCode relocate wiring |
| UC-W1 | worktree create via session_prepare + worktree sync CLI |
| UC-W2 | re-enter: seed skipped + catch-up from stored sha |
| UC-W3 | worktree delete leaves main intact |
| UC-A1 | serve seed alone insufficient; auto_prepare catch-up |
| UC-M1 | auto_prepare on drift; no dirty loop |
| UC-M2 | ensure_graph MCP budget; retry after partial |
| UC-M3 | non-repo root: MCP auto_prepare and session_prepare refuse to build |
| UC-E1 | documented only (ongoing update, not bootstrap) |
"""

from __future__ import annotations

import argparse
import io
import json
from pathlib import Path
from typing import Any
from unittest.mock import patch

from worktree_fixtures import git

from dagayn.graph import GraphStore
from dagayn.incremental import full_build
from dagayn.parser import NodeInfo
from dagayn.skills import (
    _opencode_plugin_content,
    generate_hooks_config,
    install_cursor_hooks,
    install_cursor_worktree_setup,
)
from dagayn.tools.context import get_minimal_context
from dagayn.tools.ensure import ensure_graph
from dagayn.tools.session_prepare import (
    default_prepare_budget_seconds,
    session_prepare,
)
from dagayn.tools.sync_status import (
    SyncPayload,
    assess_graph_sync,
    is_structure_ready,
    needs_mcp_auto_prepare,
    needs_structure_prepare,
    sync_state,
)
from dagayn.worktree import ensure_worktree_graph, seed_worktree_graph


def _head(repo: Path) -> str:
    return git(repo, "rev-parse", "HEAD").stdout.strip()


def _seed_store(repo: Path, *, head_sha: str | None = None) -> Path:
    """Seed a non-empty GraphStore graph for sync assessment."""
    db = repo / ".dagayn" / "graph.db"
    db.parent.mkdir(parents=True, exist_ok=True)
    store = GraphStore(str(db))
    store.upsert_node(
        NodeInfo(
            kind="File",
            name="hello.py",
            file_path=str(repo / "hello.py"),
            line_start=1,
            line_end=2,
            language="python",
        )
    )
    store.set_metadata("last_updated", "2026-08-10T00:00:00")
    if head_sha is not None:
        store.set_metadata("git_head_sha", head_sha)
    store.commit()
    store.close()
    return db


def _assess(repo: Path) -> SyncPayload:
    store = GraphStore(str(repo / ".dagayn" / "graph.db"))
    try:
        return assess_graph_sync(store, repo)
    finally:
        store.close()


def _assess_verified(repo: Path) -> SyncPayload:
    """Assess with content verification uncapped, as session prepare does."""
    store = GraphStore(str(repo / ".dagayn" / "graph.db"))
    try:
        return assess_graph_sync(store, repo, max_hash_candidates=None)
    finally:
        store.close()


def _node_names(repo: Path) -> set[str]:
    import sqlite3

    conn = sqlite3.connect(repo / ".dagayn" / "graph.db")
    try:
        return {row[0] for row in conn.execute("SELECT name FROM nodes")}
    finally:
        conn.close()


def _assert_structure_ready(result: dict, repo: Path) -> None:
    assert result["status"] == "ok", result
    assert is_structure_ready(result["sync"]), result["sync"]
    assert is_structure_ready(_assess(repo))


class TestAssessGraphSyncContract:
    """UC sync matrix: the five GraphSyncState members and their predicates."""

    def test_empty_graph(self, main_repo: Path):
        GraphStore(str(main_repo / ".dagayn" / "graph.db")).close()
        sync = _assess(main_repo)
        assert sync["state"] == "unbuilt"
        assert sync["status"] == "empty"
        assert needs_structure_prepare(sync) is True
        assert needs_mcp_auto_prepare(sync) is True
        assert is_structure_ready(sync) is False

    def test_non_git_root_assessed_as_none_and_never_auto_prepares(self, tmp_path: Path):
        """UC-M3: sync assessment carries vcs; non-repo roots never bootstrap."""
        db = tmp_path / ".dagayn" / "graph.db"
        GraphStore(str(db)).close()
        sync = _assess(tmp_path)
        assert sync["vcs"] == "none"
        assert sync["state"] == "unbuilt"
        assert sync["status"] == "empty"
        assert needs_mcp_auto_prepare(sync) is False
        # Structure prepare remains the explicit/session-start path; the
        # session_prepare guard below is what stops the build.
        assert needs_structure_prepare(sync) is True
        # Legacy dicts without vcs keep the old behavior.
        assert needs_mcp_auto_prepare({"state": "unbuilt", "status": "empty"}) is True

    def test_git_drift_when_head_differs(self, main_repo: Path):
        _seed_store(main_repo, head_sha="0" * 40)
        sync = _assess(main_repo)
        assert sync["state"] == "commit_drift"
        assert sync["status"] == "git_drift"
        assert needs_structure_prepare(sync) is True
        assert needs_mcp_auto_prepare(sync) is True
        assert is_structure_ready(sync) is False

    def test_dirty_worktree_is_structure_ready(self, main_repo: Path):
        _seed_store(main_repo, head_sha=_head(main_repo))
        (main_repo / "hello.py").write_text(
            "def greet():\n    return 'dirty'\n",
            encoding="utf-8",
        )
        sync = _assess(main_repo)
        assert sync["state"] == "worktree_behind"
        assert sync["status"] == "dirty_worktree"
        assert sync["worktree_dirty"] is True
        assert sync["pending_files"] == ["hello.py"]
        assert needs_structure_prepare(sync) is True
        assert needs_mcp_auto_prepare(sync) is False
        assert is_structure_ready(sync) is True

    def test_synced_when_head_matches_and_clean(self, main_repo: Path):
        _seed_store(main_repo, head_sha=_head(main_repo))
        sync = _assess(main_repo)
        assert sync["state"] == "commit_synced"
        assert sync["status"] == "synced"
        assert needs_structure_prepare(sync) is False
        assert needs_mcp_auto_prepare(sync) is False
        assert needs_structure_prepare(sync, force=True) is True
        assert is_structure_ready(sync) is True

    def test_worktree_ahead_when_dirty_edits_already_indexed(self, main_repo: Path):
        """An edit hook indexed the dirty tree: nothing left to prepare."""
        (main_repo / "hello.py").write_text(
            "def greet():\n    return 'indexed dirty'\n",
            encoding="utf-8",
        )
        db = main_repo / ".dagayn" / "graph.db"
        db.parent.mkdir(parents=True, exist_ok=True)
        store = GraphStore(str(db))
        try:
            full_build(main_repo, store)
        finally:
            store.close()

        sync = _assess(main_repo)
        assert sync["state"] == "worktree_ahead"
        # Legacy consumers still see the single dirty status.
        assert sync["status"] == "dirty_worktree"
        assert sync["worktree_dirty"] is True
        assert "hello.py" in sync["indexed_files"]
        assert is_structure_ready(sync) is True
        assert needs_mcp_auto_prepare(sync) is False
        # The point of the state: no repeated re-index on every session start.
        assert needs_structure_prepare(sync) is False
        assert needs_structure_prepare(sync, force=True) is True

    def test_discarded_edit_on_clean_tree_is_behind_not_synced(self, main_repo: Path):
        """The graph holding a reverted edit is drift the commit tier cannot see.

        HEAD never moved and git reports a clean tree, so only comparing the
        graph's stored file hashes against disk catches it.
        """
        (main_repo / "hello.py").write_text(
            "def greet():\n    return 'discarded'\n",
            encoding="utf-8",
        )
        db = main_repo / ".dagayn" / "graph.db"
        db.parent.mkdir(parents=True, exist_ok=True)
        store = GraphStore(str(db))
        try:
            full_build(main_repo, store)
        finally:
            store.close()
        assert _assess(main_repo)["state"] == "worktree_ahead"

        git(main_repo, "checkout", "--", "hello.py")

        sync = _assess(main_repo)
        assert sync["state"] == "worktree_behind"
        assert sync["status"] == "dirty_worktree"
        assert sync["worktree_dirty"] is False
        assert sync["pending_files"] == ["hello.py"]
        assert needs_structure_prepare(sync) is True
        # Still HEAD-aligned: analysis is not blocked, it is just behind.
        assert is_structure_ready(sync) is True
        assert needs_mcp_auto_prepare(sync) is False

    def test_deleted_file_the_graph_still_holds_is_behind(self, main_repo: Path):
        db = main_repo / ".dagayn" / "graph.db"
        db.parent.mkdir(parents=True, exist_ok=True)
        store = GraphStore(str(db))
        try:
            full_build(main_repo, store)
        finally:
            store.close()
        assert _assess(main_repo)["state"] == "commit_synced"

        (main_repo / "hello.py").unlink()

        sync = _assess(main_repo)
        assert sync["state"] == "worktree_behind"
        assert sync["pending_files"] == ["hello.py"]

    def test_untracked_unparseable_file_does_not_hold_state_behind(self, main_repo: Path):
        """A dirty file dagayn would never index must not pin the state."""
        (main_repo / "notes.md.bak").write_text("scratch\n", encoding="utf-8")
        db = main_repo / ".dagayn" / "graph.db"
        db.parent.mkdir(parents=True, exist_ok=True)
        store = GraphStore(str(db))
        try:
            full_build(main_repo, store)
        finally:
            store.close()

        sync = _assess(main_repo)
        assert sync["state"] == "worktree_ahead"
        assert needs_structure_prepare(sync) is False

    def test_status_command_reports_the_assessed_state(self, main_repo: Path, capsys):
        """``dagayn status`` must not disagree with what prepare acts on."""
        from dagayn.cli.commands.build import _print_sync_state

        (main_repo / "hello.py").write_text(
            "def greet():\n    return 'discarded'\n",
            encoding="utf-8",
        )
        db = main_repo / ".dagayn" / "graph.db"
        db.parent.mkdir(parents=True, exist_ok=True)
        store = GraphStore(str(db))
        try:
            full_build(main_repo, store)
            git(main_repo, "checkout", "--", "hello.py")
            _print_sync_state(main_repo, store)
        finally:
            store.close()

        out = capsys.readouterr().out
        assert "Graph state: worktree_behind" in out
        assert "Needs re-indexing: hello.py" in out

    def test_legacy_status_only_payload_still_answers_predicates(self):
        """Callers holding a pre-union assessment keep working (dirty = behind)."""
        assert sync_state({"status": "git_drift"}) == "commit_drift"
        assert sync_state({"status": "dirty_worktree"}) == "worktree_behind"
        assert sync_state({"status": "nonsense"}) is None
        assert needs_structure_prepare({"status": "dirty_worktree"}) is True
        assert is_structure_ready({"status": "synced"}) is True
        assert is_structure_ready({}) is False


class TestSessionPrepareContract:
    """UC-S1/S2/H1/M2 contract paths with mocked structure builds."""

    def test_uc_s1_prepare_on_git_drift_runs_incremental(self, main_repo: Path):
        _seed_store(main_repo, head_sha="0" * 40)

        def _fake_build(**kwargs):
            assert kwargs["full_rebuild"] is False
            assert kwargs["postprocess"] == "minimal"
            assert kwargs["local_embedding"] == "none"
            _seed_store(main_repo, head_sha=_head(main_repo))
            return {"status": "ok", "summary": "Incremental update complete."}

        with patch(
            "dagayn.tools.session_prepare.build_or_update_graph",
            side_effect=_fake_build,
        ) as build:
            result = session_prepare(
                repo_root=str(main_repo),
                local_embedding="none",
                embedding_policy="skip",
                budget_seconds=60,
            )

        build.assert_called_once()
        assert result["action"] == "incremental"
        assert result["reason"] == "git_drift"
        assert result["phases"]["structure"] == "done"
        _assert_structure_ready(result, main_repo)

    def test_uc_s2_noop_when_already_synced(self, main_repo: Path):
        _seed_store(main_repo, head_sha=_head(main_repo))
        with patch("dagayn.tools.session_prepare.build_or_update_graph") as build:
            result = session_prepare(
                repo_root=str(main_repo),
                local_embedding="none",
                embedding_policy="skip",
                budget_seconds=60,
            )
        build.assert_not_called()
        assert result["action"] == "noop"
        assert result["reason"] == "graph_ready"
        _assert_structure_ready(result, main_repo)

    def test_budget_exhausted_before_structure_is_partial(self, main_repo: Path):
        _seed_store(main_repo, head_sha="0" * 40)
        with (
            patch("dagayn.tools.session_prepare._remaining_seconds", return_value=0.0),
            patch("dagayn.tools.session_prepare.build_or_update_graph") as build,
        ):
            result = session_prepare(
                repo_root=str(main_repo),
                local_embedding="none",
                embedding_policy="skip",
                budget_seconds=45,
            )
        build.assert_not_called()
        assert result["phases"]["structure"] == "skipped_budget"
        assert result["status"] == "partial"
        assert is_structure_ready(result["sync"]) is False

    def test_uc_m2_ensure_graph_uses_mcp_budget_and_seeds(self, main_repo: Path):
        _seed_store(main_repo, head_sha=_head(main_repo))
        with patch(
            "dagayn.tools.ensure.session_prepare",
            return_value={"status": "ok", "action": "noop"},
        ) as prepare:
            ensure_graph(repo_root=str(main_repo), local_embedding="none")
        kwargs = prepare.call_args.kwargs
        assert kwargs["seed_worktree"] is True
        assert kwargs["budget_seconds"] == default_prepare_budget_seconds(mcp=True)
        assert kwargs["embedding_policy"] == "auto"

    def test_uc_m2_retry_after_partial_reaches_ready(self, main_repo: Path):
        _seed_store(main_repo, head_sha="0" * 40)
        with (
            patch("dagayn.tools.session_prepare._remaining_seconds", return_value=0.0),
            patch("dagayn.tools.session_prepare.build_or_update_graph") as build,
        ):
            first = session_prepare(
                repo_root=str(main_repo),
                local_embedding="none",
                embedding_policy="skip",
                budget_seconds=45,
            )
        build.assert_not_called()
        assert first["status"] == "partial"
        assert is_structure_ready(first["sync"]) is False

        def _fake_build(**kwargs):
            _seed_store(main_repo, head_sha=_head(main_repo))
            return {"status": "ok", "summary": "Incremental update complete."}

        with patch(
            "dagayn.tools.session_prepare.build_or_update_graph",
            side_effect=_fake_build,
        ):
            second = ensure_graph(
                repo_root=str(main_repo),
                local_embedding="none",
                embedding_policy="skip",
                budget_seconds=60,
            )
        _assert_structure_ready(second, main_repo)


class TestMinimalContextAutoPrepare:
    """UC-M1: MCP first-tool path auto-prepares on drift, not dirty loops."""

    def test_uc_m1_auto_prepare_on_git_drift(self, main_repo: Path):
        _seed_store(main_repo, head_sha="0" * 40)

        with (
            patch(
                "dagayn.task_queue.enqueue_session_prepare",
                return_value=("added", 1),
            ) as enqueue,
            patch("dagayn.tools.session_prepare.session_prepare") as prepare,
        ):
            result = get_minimal_context(
                task="explore codebase",
                repo_root=str(main_repo),
                auto_prepare=True,
                local_embedding="none",
                prepare_budget_seconds=60,
            )

        enqueue.assert_called_once()
        prepare.assert_not_called()
        assert result["prepare"]["action"] == "queued"
        assert result["prepare"]["reason"] == "enqueued_background_prepare"
        assert result["repair"]["kind"] == "prepare"
        assert result["sync"]["status"] == "git_drift"

    def test_auto_prepare_skipped_when_synced(self, main_repo: Path):
        _seed_store(main_repo, head_sha=_head(main_repo))
        with patch("dagayn.task_queue.enqueue_session_prepare") as enqueue:
            result = get_minimal_context(
                task="explore codebase",
                repo_root=str(main_repo),
                auto_prepare=True,
                local_embedding="none",
            )
        enqueue.assert_not_called()
        assert "prepare" not in result or result.get("prepare") is None
        assert result["sync"]["status"] == "synced"

    def test_uc_m1_dirty_does_not_auto_prepare_loop(self, main_repo: Path):
        _seed_store(main_repo, head_sha=_head(main_repo))
        (main_repo / "hello.py").write_text(
            "def greet():\n    return 'dirty'\n",
            encoding="utf-8",
        )
        assert _assess(main_repo)["status"] == "dirty_worktree"
        with patch("dagayn.task_queue.enqueue_session_prepare") as enqueue:
            result = get_minimal_context(
                task="explore codebase",
                repo_root=str(main_repo),
                auto_prepare=True,
                local_embedding="none",
            )
        enqueue.assert_not_called()
        assert result["sync"]["status"] == "dirty_worktree"
        assert "ensure_graph_tool" not in result.get("recommended_action", "")

    def test_uc_m3_non_git_root_never_auto_prepares(self, tmp_path: Path):
        """UC-M3: a misdetected non-repo root (e.g. $HOME) must not bootstrap.

        A leftover empty ``.dagayn/graph.db`` at a non-repo root previously
        passed the project-root validation and triggered a full build of the
        whole non-repo tree. It now reports the sync state with
        ``vcs == "none"`` and leaves the graph untouched.
        """
        GraphStore(str(tmp_path / ".dagayn" / "graph.db")).close()
        with patch("dagayn.task_queue.enqueue_session_prepare") as enqueue:
            result = get_minimal_context(
                task="explore codebase",
                repo_root=str(tmp_path),
                auto_prepare=True,
                local_embedding="none",
                prepare_budget_seconds=60,
            )
        enqueue.assert_not_called()
        assert result["sync"]["vcs"] == "none"
        assert result["sync"]["state"] == "unbuilt"


class TestSessionPrepareNonGitRoot:
    """UC-M3: session_prepare refuses to bootstrap a non-repo root."""

    def test_session_prepare_skips_non_git_root(self, tmp_path: Path):
        result = session_prepare(
            repo_root=str(tmp_path),
            local_embedding="none",
            embedding_policy="skip",
            budget_seconds=60,
        )
        assert result["reason"] == "not_vcs_repo"
        assert result["action"] == "noop"
        assert result["phases"]["structure"] == "noop"
        assert not (tmp_path / ".dagayn" / "graph.db").exists()

    def test_ensure_graph_skips_non_git_root(self, tmp_path: Path):
        from dagayn.tools.ensure import ensure_graph

        result = ensure_graph(
            repo_root=str(tmp_path),
            local_embedding="none",
            budget_seconds=60,
        )
        assert result["reason"] == "not_vcs_repo"
        assert not (tmp_path / ".dagayn" / "graph.db").exists()


class TestWorktreeFreshnessIntegration:
    """Real-git UC-W1/W2/W3/A1/H1/S2 integration."""

    def test_uc_w1_worktree_create_seed_and_prepare(self, main_repo: Path, linked_worktree: Path):
        from dagayn.tools.build import build_or_update_graph

        build = build_or_update_graph(
            full_rebuild=True,
            repo_root=str(main_repo),
            postprocess="minimal",
            local_embedding="none",
        )
        assert build.get("status") != "error", build

        result = session_prepare(
            repo_root=str(linked_worktree),
            local_embedding="none",
            embedding_policy="skip",
            budget_seconds=120,
            seed_worktree=True,
        )
        assert (linked_worktree / ".dagayn" / "graph.db").exists()
        _assert_structure_ready(result, linked_worktree)
        assert result["sync"]["git_head_sha"] == _head(linked_worktree)
        assert is_structure_ready(_assess(main_repo))

    def test_uc_w1_worktree_sync_cli(self, main_repo: Path, linked_worktree: Path):
        from dagayn.cli.commands.worktree import _handle_sync
        from dagayn.tools.build import build_or_update_graph

        build_or_update_graph(
            full_rebuild=True,
            repo_root=str(main_repo),
            postprocess="minimal",
            local_embedding="none",
        )
        args = argparse.Namespace(
            repo=str(linked_worktree),
            from_hook=False,
            base=None,
            seed_only=False,
            copy_config=True,
            build_if_missing=False,
            as_json=False,
            local_embedding="none",
            local_embedding_mode=None,
            local_embedding_port=18080,
            local_embedding_bin="auto",
            keep_local_embedding_server=False,
            local_embedding_timeout=300,
            local_embedding_request_timeout=60,
            local_embedding_batch_size=1,
        )
        _handle_sync(args)
        assert (linked_worktree / ".dagayn" / "graph.db").exists()
        assert is_structure_ready(_assess(linked_worktree))
        assert _assess(linked_worktree)["git_head_sha"] == _head(linked_worktree)

    def test_uc_w1_from_hook_prepare_resolves_worktree(
        self, main_repo: Path, linked_worktree: Path
    ):
        from dagayn.tools.build import build_or_update_graph

        build_or_update_graph(
            full_rebuild=True,
            repo_root=str(main_repo),
            postprocess="minimal",
            local_embedding="none",
        )
        payload = json.dumps(
            {
                "tool_name": "EnterWorktree",
                "tool_response": {"worktree_path": str(linked_worktree)},
            }
        )

        class _HookStdin(io.StringIO):
            def isatty(self) -> bool:  # noqa: ANN001
                return False

        with patch("sys.stdin", _HookStdin(payload)):
            result = session_prepare(
                from_hook=True,
                local_embedding="none",
                embedding_policy="skip",
                budget_seconds=120,
            )
        assert Path(result["repo_root"]).resolve() == linked_worktree.resolve()
        _assert_structure_ready(result, linked_worktree)

    def test_uc_w2_reenter_seed_skipped_then_catch_up(self, main_repo: Path, linked_worktree: Path):
        from dagayn.tools.build import build_or_update_graph

        build_or_update_graph(
            full_rebuild=True,
            repo_root=str(main_repo),
            postprocess="minimal",
            local_embedding="none",
        )
        first = session_prepare(
            repo_root=str(linked_worktree),
            local_embedding="none",
            embedding_policy="skip",
            budget_seconds=120,
        )
        _assert_structure_ready(first, linked_worktree)
        seed = seed_worktree_graph(linked_worktree)
        assert seed.status == "skipped"
        assert "already has a graph" in seed.reason

        (linked_worktree / "feature.py").write_text(
            "def feature():\n    return 1\n",
            encoding="utf-8",
        )
        git(linked_worktree, "add", "feature.py")
        git(linked_worktree, "commit", "-m", "feature work")
        assert _assess(linked_worktree)["status"] == "git_drift"

        result = session_prepare(
            repo_root=str(linked_worktree),
            local_embedding="none",
            embedding_policy="skip",
            budget_seconds=120,
        )
        assert result["reason"] == "git_drift"
        assert result["action"] == "incremental"
        _assert_structure_ready(result, linked_worktree)
        assert result["sync"]["git_head_sha"] == _head(linked_worktree)

    def test_uc_w3_worktree_delete_leaves_main_intact(self, main_repo: Path, linked_worktree: Path):
        from dagayn.tools.build import build_or_update_graph

        build_or_update_graph(
            full_rebuild=True,
            repo_root=str(main_repo),
            postprocess="minimal",
            local_embedding="none",
        )
        session_prepare(
            repo_root=str(linked_worktree),
            local_embedding="none",
            embedding_policy="skip",
            budget_seconds=120,
        )
        wt_path = linked_worktree.resolve()
        removed = git(main_repo, "worktree", "remove", "--force", str(wt_path))
        assert removed.returncode == 0, removed.stderr
        assert not wt_path.exists()
        sync = _assess(main_repo)
        assert is_structure_ready(sync)
        assert sync["git_head_sha"] == _head(main_repo)

    def test_uc_h1_head_relocate_prepare(self, main_repo: Path):
        from dagayn.tools.build import build_or_update_graph

        build_or_update_graph(
            full_rebuild=True,
            repo_root=str(main_repo),
            postprocess="minimal",
            local_embedding="none",
        )
        assert is_structure_ready(_assess(main_repo))

        (main_repo / "next.py").write_text("def next_step():\n    return 2\n", encoding="utf-8")
        git(main_repo, "add", "next.py")
        git(main_repo, "commit", "-m", "advance head")
        assert _assess(main_repo)["status"] == "git_drift"

        result = session_prepare(
            repo_root=str(main_repo),
            local_embedding="none",
            embedding_policy="skip",
            budget_seconds=120,
        )
        assert result["reason"] == "git_drift"
        assert result["action"] == "incremental"
        _assert_structure_ready(result, main_repo)

    def test_uc_s2_dirty_worktree_prepare_is_structure_ready(self, main_repo: Path):
        from dagayn.tools.build import build_or_update_graph

        build_or_update_graph(
            full_rebuild=True,
            repo_root=str(main_repo),
            postprocess="minimal",
            local_embedding="none",
        )
        (main_repo / "hello.py").write_text(
            "def greet():\n    return 'dirty'\n",
            encoding="utf-8",
        )
        assert _assess(main_repo)["status"] == "dirty_worktree"

        result = session_prepare(
            repo_root=str(main_repo),
            local_embedding="none",
            embedding_policy="skip",
            budget_seconds=120,
        )
        assert result["reason"] == "dirty_worktree"
        assert result["phases"]["structure"] == "done"
        _assert_structure_ready(result, main_repo)
        assert result["sync"]["status"] == "dirty_worktree"

    def test_uc_a1_serve_seed_alone_insufficient_then_auto_prepare(
        self, main_repo: Path, linked_worktree: Path
    ):
        from dagayn.tools.build import build_or_update_graph

        build_or_update_graph(
            full_rebuild=True,
            repo_root=str(main_repo),
            postprocess="minimal",
            local_embedding="none",
        )
        seeded = ensure_worktree_graph(linked_worktree)
        assert seeded.status == "seeded"
        assert is_structure_ready(_assess(linked_worktree))

        (linked_worktree / "agent.py").write_text(
            "def agent():\n    return True\n",
            encoding="utf-8",
        )
        git(linked_worktree, "add", "agent.py")
        git(linked_worktree, "commit", "-m", "subagent commit")
        assert _assess(linked_worktree)["status"] == "git_drift"
        # serve-style seed alone does not catch up an existing worktree graph.
        again = ensure_worktree_graph(linked_worktree)
        assert again.status == "skipped"
        assert _assess(linked_worktree)["status"] == "git_drift"

        with patch(
            "dagayn.task_queue.enqueue_session_prepare",
            return_value=("added", 1),
        ) as enqueue:
            result = get_minimal_context(
                task="implement feature in worktree",
                repo_root=str(linked_worktree),
                auto_prepare=True,
                local_embedding="none",
                prepare_budget_seconds=120,
            )
        enqueue.assert_called_once()
        assert result["prepare"]["action"] == "queued"
        assert result["repair"]["kind"] == "prepare"
        assert result["sync"]["status"] == "git_drift"
        assert not is_structure_ready(_assess(linked_worktree))

        prepared = session_prepare(
            repo_root=str(linked_worktree),
            local_embedding="none",
            embedding_policy="skip",
            budget_seconds=120,
        )
        _assert_structure_ready(prepared, linked_worktree)


class TestHookWiringFreshness:
    """UC-S1 / UC-W1 / UC-H1 install wiring (config content, not process spawn)."""

    def test_uc_s1_claude_session_start_uses_session_prepare(self):
        config = generate_hooks_config(Path("/repo"), worktree_hook=True)
        session = config["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        assert "session prepare" in session
        assert "--budget-seconds" in session

    def test_uc_w1_claude_enter_worktree_uses_session_prepare(self):
        config = generate_hooks_config(Path("/repo"), worktree_hook=True)
        post = config["hooks"]["PostToolUse"]
        enter = [e for e in post if e.get("matcher") == "EnterWorktree|ExitWorktree"]
        assert enter, "EnterWorktree hook missing"
        cmd = enter[0]["hooks"][0]["command"]
        assert "session prepare" in cmd
        assert "--from-hook" in cmd

    def test_uc_w1_cursor_worktrees_json_uses_session_prepare(self, main_repo: Path):
        status = install_cursor_worktree_setup(main_repo)
        assert status in {"created", "updated", "unchanged"}
        data = json.loads((main_repo / ".cursor" / "worktrees.json").read_text())
        commands = data.get("setup-worktree") or []
        assert any("session prepare" in str(cmd) for cmd in commands)

    def test_uc_s1_uc_h1_cursor_scripts_use_session_prepare(self, tmp_path: Path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_cursor_hooks()
        start = (tmp_path / ".cursor" / "hooks" / "crg-session-start.sh").read_text(
            encoding="utf-8"
        )
        relocate = (tmp_path / ".cursor" / "hooks" / "crg-relocate.sh").read_text(encoding="utf-8")
        assert "session prepare" in start
        assert "session prepare" in relocate
        hooks = json.loads((tmp_path / ".cursor" / "hooks.json").read_text(encoding="utf-8"))
        assert "sessionStart" in hooks["hooks"]
        assert "afterShellExecution" in hooks["hooks"]

    def test_uc_s1_uc_h1_opencode_plugin_uses_session_prepare(self):
        content = _opencode_plugin_content()
        assert '"session.created"' in content
        assert "dagayn session prepare" in content
        assert "checkout|switch|reset|pull|merge|rebase|cherry-pick" in content
        assert '"tool.execute.after"' in content


class TestContentDriftConvergence:
    """A state must be reachable *out of*, not just into.

    ``worktree_behind`` proven by content verification used to be a fixed
    point: the git diff that drives ``incremental_update`` cannot contain a
    file whose on-disk bytes equal the base commit, so prepare re-ran forever,
    reported "No changes detected", and the graph kept serving the wrong
    content.
    """

    def test_discarded_edit_is_reindexed_by_prepare(self, main_repo: Path):
        from dagayn.tools.build import build_or_update_graph

        target = main_repo / "hello.py"
        original = target.read_text(encoding="utf-8")
        build_or_update_graph(
            full_rebuild=True,
            repo_root=str(main_repo),
            postprocess="minimal",
            local_embedding="none",
        )

        # An edit hook indexes an uncommitted edit, which is then discarded.
        target.write_text("def discarded_edit():\n    return 2\n", encoding="utf-8")
        build_or_update_graph(
            full_rebuild=False,
            repo_root=str(main_repo),
            base="HEAD",
            postprocess="minimal",
            local_embedding="none",
        )
        target.write_text(original, encoding="utf-8")

        before = _assess_verified(main_repo)
        assert sync_state(before) == "worktree_behind"
        assert before["pending_files"] == ["hello.py"]

        session_prepare(repo_root=str(main_repo), budget_seconds=None)

        after = _assess_verified(main_repo)
        assert sync_state(after) == "commit_synced", after
        assert "discarded_edit" not in _node_names(main_repo)

    def test_unverified_content_is_not_reported_as_verified(self, main_repo: Path):
        """The cheap cap must not let the dirty-only answer pass as verified.

        A fresh worktree checkout moves every indexed file's mtime, so the cap
        is hit on any real repository -- exactly where claiming ``commit_synced``
        is least justified.
        """
        import os

        from dagayn.tools.build import build_or_update_graph

        build_or_update_graph(
            full_rebuild=True,
            repo_root=str(main_repo),
            postprocess="minimal",
            local_embedding="none",
        )
        # Move the mtime without touching the bytes, as a checkout would.
        target = main_repo / "hello.py"
        stat = target.stat()
        os.utime(target, ns=(stat.st_atime_ns, stat.st_mtime_ns + 1_000_000_000))

        store = GraphStore(str(main_repo / ".dagayn" / "graph.db"))
        try:
            capped = assess_graph_sync(store, main_repo, max_hash_candidates=0)
            uncapped = assess_graph_sync(store, main_repo, max_hash_candidates=None)
        finally:
            store.close()

        assert capped["content_verified"] is False
        assert capped["unverified_file_count"] >= 1
        # Uncapped verification hashes the bytes and finds them unchanged.
        assert uncapped["content_verified"] is True
        assert sync_state(uncapped) == "commit_synced"


class TestSessionPrepareHardStop:
    """A prepare phase already running must still be stoppable.

    ``budget_seconds`` only gates whether the *next* phase starts, so a single
    long phase outlives it without limit: one observed run took ~5 minutes
    against a 45s budget, and another held the exclusive graph lock for 26 hours
    (21.5 h of CPU) — the stall every MCP call on that graph waited behind.
    """

    def test_hard_stop_is_a_multiple_of_the_advisory_budget(self):
        from dagayn.tools.session_prepare import (
            PREPARE_BUDGET_HARD_STOP_FACTOR,
            prepare_hard_stop_seconds,
        )

        # Killing at exactly the budget would kill a phase that needs slightly
        # longer on every session start, and the graph would never converge.
        assert PREPARE_BUDGET_HARD_STOP_FACTOR > 1
        assert prepare_hard_stop_seconds(45) == 45 * PREPARE_BUDGET_HARD_STOP_FACTOR

    def test_disabled_budget_stays_unbounded(self):
        from dagayn.tools.session_prepare import prepare_hard_stop_seconds

        assert prepare_hard_stop_seconds(None) is None
        assert prepare_hard_stop_seconds(0) is None

    def test_cli_arms_the_watchdog_with_the_hard_stop(self, monkeypatch, main_repo: Path):
        from dagayn.cli.commands import session as session_cli
        from dagayn.tools.session_prepare import PREPARE_BUDGET_HARD_STOP_FACTOR

        armed: list[tuple[float | None, str]] = []
        cancelled: list[bool] = []

        class FakeTimer:
            def cancel(self) -> None:
                cancelled.append(True)

        def fake_watchdog(budget, *, label="update"):
            armed.append((budget, label))
            return FakeTimer()

        monkeypatch.setattr("dagayn.hook_guard.start_budget_watchdog", fake_watchdog)
        monkeypatch.setattr(
            session_cli,
            "_run_session_prepare",
            lambda _args, _budget: {"summary": "done", "phases": {}},
        )

        args = argparse.Namespace(
            session_command="prepare",
            repo=str(main_repo),
            budget_seconds=30,
            as_json=False,
        )
        session_cli.handle(args, argparse.ArgumentParser())

        assert armed == [(30 * PREPARE_BUDGET_HARD_STOP_FACTOR, "session prepare")]
        assert cancelled == [True], "the watchdog must be cancelled once prepare returns"

    def test_cli_arms_the_watchdog_from_the_default_budget(self, monkeypatch, main_repo: Path):
        from dagayn.cli.commands import session as session_cli
        from dagayn.tools.session_prepare import (
            PREPARE_BUDGET_HARD_STOP_FACTOR,
            default_prepare_budget_seconds,
        )

        armed: list[tuple[float | None, str]] = []
        monkeypatch.setattr(
            "dagayn.hook_guard.start_budget_watchdog",
            lambda budget, *, label="update": armed.append((budget, label)) or None,
        )
        monkeypatch.setattr(
            session_cli,
            "_run_session_prepare",
            lambda _args, _budget: {"summary": "done", "phases": {}},
        )

        args = argparse.Namespace(
            session_command="prepare",
            repo=str(main_repo),
            budget_seconds=None,
            as_json=False,
        )
        session_cli.handle(args, argparse.ArgumentParser())

        default_budget = default_prepare_budget_seconds(mcp=False)
        assert default_budget is not None, "the hook default must be a real budget"
        assert armed == [(default_budget * PREPARE_BUDGET_HARD_STOP_FACTOR, "session prepare")]

    def test_explicitly_unbounded_budget_arms_nothing(self, monkeypatch, main_repo: Path):
        from dagayn.cli.commands import session as session_cli

        armed: list[Any] = []
        monkeypatch.setattr(
            "dagayn.hook_guard.start_budget_watchdog",
            lambda budget, *, label="update": armed.append(budget) or None,
        )
        monkeypatch.setattr(
            session_cli,
            "_run_session_prepare",
            lambda _args, _budget: {"summary": "done", "phases": {}},
        )

        args = argparse.Namespace(
            session_command="prepare",
            repo=str(main_repo),
            budget_seconds=0,
            as_json=False,
        )
        session_cli.handle(args, argparse.ArgumentParser())

        assert armed == [None], "0 disables the budget, so nothing may be killed"
