"""Guarantee tests for session / worktree / Subagent graph freshness.

Use-case catalog: docs/SESSION-GRAPH-FRESHNESS.md

| ID    | Covered by |
| ----- | ---------- |
| UC-S1 | hook wiring + session_prepare on drift |
| UC-S2 | dirty_worktree + HEAD drift prepare |
| UC-H1 | HEAD relocate prepare |
| UC-W1 | worktree create seed+prepare |
| UC-W2 | worktree re-enter after branch commit |
| UC-W3 | worktree delete leaves main intact |
| UC-A1 | worktree + get_minimal_context(auto_prepare) |
| UC-M1 | get_minimal_context(auto_prepare=True) |
| UC-M2 | ensure_graph seed_worktree + MCP budget |
| UC-E1 | documented only (ongoing update, not bootstrap) |
"""

from __future__ import annotations

import json
from pathlib import Path
from unittest.mock import patch

from worktree_fixtures import git

from dagayn.graph import GraphStore
from dagayn.parser import NodeInfo
from dagayn.skills import (
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
    needs_structure_prepare,
)


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


class TestAssessGraphSyncContract:
    """UC sync matrix: empty / git_drift / dirty_worktree / synced."""

    def test_empty_graph(self, main_repo: Path):
        GraphStore(str(main_repo / ".dagayn" / "graph.db")).close()
        sync = _assess(main_repo)
        assert sync["status"] == "empty"
        assert needs_structure_prepare(sync) is True

    def test_git_drift_when_head_differs(self, main_repo: Path):
        _seed_store(main_repo, head_sha="0" * 40)
        sync = _assess(main_repo)
        assert sync["status"] == "git_drift"
        assert needs_structure_prepare(sync) is True

    def test_dirty_worktree_when_head_matches(self, main_repo: Path):
        _seed_store(main_repo, head_sha=_head(main_repo))
        (main_repo / "hello.py").write_text(
            "def greet():\n    return 'dirty'\n",
            encoding="utf-8",
        )
        sync = _assess(main_repo)
        assert sync["status"] == "dirty_worktree"
        assert sync["worktree_dirty"] is True
        assert needs_structure_prepare(sync) is True

    def test_synced_when_head_matches_and_clean(self, main_repo: Path):
        _seed_store(main_repo, head_sha=_head(main_repo))
        sync = _assess(main_repo)
        assert sync["status"] == "synced"
        assert needs_structure_prepare(sync) is False
        assert needs_structure_prepare(sync, force=True) is True


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
        assert result["sync"]["status"] == "synced"
        assert result["status"] == "ok"

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
        assert result["sync"]["status"] == "synced"
        assert result["status"] == "ok"

    def test_budget_exhausted_before_structure_is_partial(self, main_repo: Path):
        _seed_store(main_repo, head_sha="0" * 40)
        with patch("dagayn.tools.session_prepare.build_or_update_graph") as build:
            result = session_prepare(
                repo_root=str(main_repo),
                local_embedding="none",
                embedding_policy="skip",
                budget_seconds=0.0001,
            )
        # Tiny budget may skip structure; never claim synced success.
        if result["phases"]["structure"] == "skipped_budget":
            build.assert_not_called()
            assert result["status"] == "partial"
            assert result["sync"]["status"] != "synced"
        else:
            # Race: prepare finished before deadline — still a valid outcome.
            assert result["status"] in {"ok", "partial"}

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


class TestMinimalContextAutoPrepare:
    """UC-M1: MCP first-tool path auto-prepares on drift."""

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
        assert result["status"] in {"ok", "partial"}
        assert (linked_worktree / ".dagayn" / "graph.db").exists()
        sync = _assess(linked_worktree)
        assert sync["status"] == "synced"
        assert sync["git_head_sha"] == _head(linked_worktree)
        # Main checkout graph remains present and separate.
        assert (main_repo / ".dagayn" / "graph.db").exists()
        assert _assess(main_repo)["status"] == "synced"

    def test_uc_w2_reenter_after_branch_commit(self, main_repo: Path, linked_worktree: Path):
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
        assert _assess(linked_worktree)["status"] == "synced"

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
        assert result["status"] in {"ok", "partial"}
        assert result["reason"] in {"git_drift", "graph_ready"}
        sync = _assess(linked_worktree)
        assert sync["status"] == "synced"
        assert sync["git_head_sha"] == _head(linked_worktree)

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
        assert sync["status"] == "synced"
        assert sync["git_head_sha"] == _head(main_repo)

    def test_uc_h1_head_relocate_prepare(self, main_repo: Path):
        from dagayn.tools.build import build_or_update_graph

        build_or_update_graph(
            full_rebuild=True,
            repo_root=str(main_repo),
            postprocess="minimal",
            local_embedding="none",
        )
        assert _assess(main_repo)["status"] == "synced"

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
        assert result["status"] in {"ok", "partial"}
        assert result["reason"] == "git_drift" or result["sync"]["status"] == "synced"
        assert _assess(main_repo)["status"] == "synced"

    def test_uc_s2_dirty_worktree_prepare(self, main_repo: Path):
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
        assert result["status"] in {"ok", "partial"}
        assert result["phases"]["structure"] == "done"
        assert result["reason"] == "dirty_worktree"
        # Uncommitted edits keep worktree_dirty true; prepare still refreshed structure.
        assert result["sync"]["status"] in {"synced", "dirty_worktree"}

    def test_uc_a1_subagent_standin_auto_prepare(self, main_repo: Path, linked_worktree: Path):
        from dagayn.tools.build import build_or_update_graph

        build_or_update_graph(
            full_rebuild=True,
            repo_root=str(main_repo),
            postprocess="minimal",
            local_embedding="none",
        )
        # Simulate Subagent landing in a fresh worktree with no local graph.
        assert not (linked_worktree / ".dagayn" / "graph.db").exists()

        result = get_minimal_context(
            task="implement feature in worktree",
            repo_root=str(linked_worktree),
            auto_prepare=True,
            local_embedding="none",
            prepare_budget_seconds=120,
        )
        assert (linked_worktree / ".dagayn" / "graph.db").exists()
        assert result["sync"]["status"] == "synced"
        assert result.get("prepare") is not None


class TestHookWiringFreshness:
    """UC-S1 / UC-W1 / UC-H1 install wiring (config content, not process spawn)."""

    def test_uc_s1_claude_session_start_uses_session_prepare(self):
        config = generate_hooks_config(Path("/repo"), worktree_hook=True)
        session = config["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        assert "session prepare" in session
        assert "--budget-seconds" in session or "45" in session

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
