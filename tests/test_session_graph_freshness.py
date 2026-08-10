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
| UC-E1 | documented only (ongoing update, not bootstrap) |
"""

from __future__ import annotations

import argparse
import io
import json
from pathlib import Path
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


def _assess(repo: Path) -> dict:
    store = GraphStore(str(repo / ".dagayn" / "graph.db"))
    try:
        return assess_graph_sync(store, repo)
    finally:
        store.close()


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

        def _fake_prepare(**kwargs):
            assert kwargs.get("seed_worktree") is True
            _seed_store(main_repo, head_sha=_head(main_repo))
            return {
                "status": "ok",
                "action": "incremental",
                "reason": "git_drift",
                "phases": {"structure": "done", "embedding": "not_requested"},
                "sync": {"status": "synced"},
            }

        with patch(
            "dagayn.tools.session_prepare.session_prepare",
            side_effect=_fake_prepare,
        ) as prepare:
            result = get_minimal_context(
                task="explore codebase",
                repo_root=str(main_repo),
                auto_prepare=True,
                local_embedding="none",
                prepare_budget_seconds=60,
            )

        prepare.assert_called_once()
        assert result["prepare"]["action"] == "incremental"
        assert result["sync"]["status"] == "synced"

    def test_auto_prepare_skipped_when_synced(self, main_repo: Path):
        _seed_store(main_repo, head_sha=_head(main_repo))
        with patch("dagayn.tools.session_prepare.session_prepare") as prepare:
            result = get_minimal_context(
                task="explore codebase",
                repo_root=str(main_repo),
                auto_prepare=True,
                local_embedding="none",
            )
        prepare.assert_not_called()
        assert "prepare" not in result or result.get("prepare") is None
        assert result["sync"]["status"] == "synced"

    def test_uc_m1_dirty_does_not_auto_prepare_loop(self, main_repo: Path):
        _seed_store(main_repo, head_sha=_head(main_repo))
        (main_repo / "hello.py").write_text(
            "def greet():\n    return 'dirty'\n",
            encoding="utf-8",
        )
        assert _assess(main_repo)["status"] == "dirty_worktree"
        with patch("dagayn.tools.session_prepare.session_prepare") as prepare:
            result = get_minimal_context(
                task="explore codebase",
                repo_root=str(main_repo),
                auto_prepare=True,
                local_embedding="none",
            )
        prepare.assert_not_called()
        assert result["sync"]["status"] == "dirty_worktree"
        assert "ensure_graph_tool" not in result.get("recommended_action", "")


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

        result = get_minimal_context(
            task="implement feature in worktree",
            repo_root=str(linked_worktree),
            auto_prepare=True,
            local_embedding="none",
            prepare_budget_seconds=120,
        )
        assert result.get("prepare") is not None
        assert is_structure_ready(_assess(linked_worktree))
        assert result["sync"]["status"] in {"synced", "dirty_worktree"}


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
