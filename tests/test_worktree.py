"""Tests for git worktree support: hooks, MCP config, and graph inheritance.

Claude Code (``claude --worktree`` / ``EnterWorktree``) and Cursor run agent
sessions inside linked git worktrees, which are fresh checkouts without the
gitignored ``.mcp.json`` / ``.dagayn/`` that dagayn relies on. These tests use
real temporary repositories and real ``git worktree add`` so the resolution
logic is exercised against actual git behavior.
"""

from __future__ import annotations

import json
import sqlite3
from pathlib import Path

from worktree_fixtures import (
    git as _git,
)
from worktree_fixtures import (
    graph_metadata as _metadata,
)
from worktree_fixtures import (
    write_minimal_graph_db as _write_graph_db,
)

from dagayn.skills import (
    ensure_worktree_include,
    generate_hooks_config,
    install_cursor_worktree_setup,
    install_git_hook,
    worktree_include_patterns,
)
from dagayn.worktree import (
    SEED_ENV_VAR,
    copy_worktree_config,
    ensure_worktree_graph,
    git_common_dir,
    git_hooks_dir,
    is_gitignored,
    is_linked_worktree,
    main_worktree_root,
    parse_hook_payload,
    resolve_hook_repo,
    seed_worktree_graph,
    worktree_label,
)


class TestWorktreeDetection:
    def test_main_checkout_is_not_a_linked_worktree(self, main_repo: Path):
        assert is_linked_worktree(main_repo) is False
        assert worktree_label(main_repo) is None

    def test_linked_worktree_is_detected(self, linked_worktree: Path):
        assert is_linked_worktree(linked_worktree) is True
        assert worktree_label(linked_worktree) == "wt-feature"

    def test_main_worktree_root_from_worktree(self, main_repo: Path, linked_worktree: Path):
        resolved = main_worktree_root(linked_worktree)
        assert resolved is not None
        assert resolved.resolve() == main_repo.resolve()

    def test_common_dir_is_shared(self, main_repo: Path, linked_worktree: Path):
        from_main = git_common_dir(main_repo)
        from_worktree = git_common_dir(linked_worktree)
        assert from_main is not None and from_worktree is not None
        assert from_main.resolve() == from_worktree.resolve()

    def test_non_git_directory(self, tmp_path: Path):
        plain = tmp_path / "plain"
        plain.mkdir()
        assert main_worktree_root(plain) is None
        assert is_linked_worktree(plain) is False


class TestGitHooksDir:
    def test_worktree_resolves_to_shared_hooks_dir(self, main_repo: Path, linked_worktree: Path):
        hooks_dir = git_hooks_dir(linked_worktree)
        assert hooks_dir is not None
        assert hooks_dir.resolve() == (main_repo / ".git" / "hooks").resolve()

    def test_honors_core_hooks_path(self, main_repo: Path):
        custom = main_repo / "githooks"
        custom.mkdir()
        _git(main_repo, "config", "core.hooksPath", "githooks")
        hooks_dir = git_hooks_dir(main_repo)
        assert hooks_dir is not None
        assert hooks_dir.resolve() == custom.resolve()

    def test_falls_back_to_dot_git_hooks_without_git(self, tmp_path: Path, monkeypatch):
        """A synthetic .git directory (no git binary usable) still resolves."""
        repo = tmp_path / "fake"
        (repo / ".git").mkdir(parents=True)
        monkeypatch.setattr("dagayn.worktree._git", lambda *args, **kwargs: None)
        hooks_dir = git_hooks_dir(repo)
        assert hooks_dir == repo / ".git" / "hooks"


class TestInstallGitHookInWorktree:
    def test_installs_into_shared_hooks_dir(self, main_repo: Path, linked_worktree: Path):
        result = install_git_hook(linked_worktree)
        assert result is not None
        shared_hooks = (main_repo / ".git" / "hooks").resolve()
        assert result.parent.resolve() == shared_hooks
        assert (shared_hooks / "pre-commit").exists()
        assert (shared_hooks / "post-commit").exists()

    def test_returns_none_outside_git(self, tmp_path: Path):
        plain = tmp_path / "plain"
        plain.mkdir()
        assert install_git_hook(plain) is None


class TestSeedWorktreeGraph:
    def test_inherits_graph_from_main_checkout(self, main_repo: Path, linked_worktree: Path):
        head = _git(main_repo, "rev-parse", "HEAD").stdout.strip()
        _write_graph_db(main_repo / ".dagayn" / "graph.db", head_sha=head, repo_root=main_repo)

        result = seed_worktree_graph(linked_worktree)

        assert result.status == "seeded", result.reason
        assert result.base_sha == head
        dest = linked_worktree / ".dagayn" / "graph.db"
        assert dest.exists()
        # The copy is re-rooted so it describes the worktree, not the main checkout.
        assert _metadata(dest, "repo_root") == str(linked_worktree)
        assert _metadata(dest, "git_head_sha") == head

    def test_skips_when_worktree_already_has_a_graph(self, main_repo: Path, linked_worktree: Path):
        _write_graph_db(main_repo / ".dagayn" / "graph.db", head_sha="abc", repo_root=main_repo)
        _write_graph_db(
            linked_worktree / ".dagayn" / "graph.db",
            head_sha="own",
            repo_root=linked_worktree,
        )

        result = seed_worktree_graph(linked_worktree)

        assert result.status == "skipped"
        assert _metadata(linked_worktree / ".dagayn" / "graph.db", "git_head_sha") == "own"

    def test_replaces_empty_stub_left_by_status(self, main_repo: Path, linked_worktree: Path):
        """GraphStore/status create a schema-only stub; that must not block seed.

        Regression: existence + size > 0 made sync skip inheritance, leaving
        worktrees at 0 nodes after a status-only SessionStart.
        """
        head = _git(main_repo, "rev-parse", "HEAD").stdout.strip()
        _write_graph_db(main_repo / ".dagayn" / "graph.db", head_sha=head, repo_root=main_repo)
        stub = linked_worktree / ".dagayn" / "graph.db"
        stub.parent.mkdir(parents=True, exist_ok=True)
        conn = sqlite3.connect(str(stub))
        try:
            conn.execute("CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT)")
            conn.execute("CREATE TABLE nodes (id INTEGER PRIMARY KEY, file_path TEXT)")
            conn.execute(
                "INSERT INTO metadata (key, value) VALUES (?, ?)",
                ("repo_root", str(linked_worktree)),
            )
            conn.commit()
        finally:
            conn.close()
        assert stub.stat().st_size > 0

        result = seed_worktree_graph(linked_worktree)

        assert result.status == "seeded", result.reason
        assert result.base_sha == head
        assert _metadata(stub, "git_head_sha") == head
        conn = sqlite3.connect(str(stub))
        try:
            count = conn.execute("SELECT COUNT(*) FROM nodes").fetchone()[0]
        finally:
            conn.close()
        assert count >= 1

    def test_skips_in_main_checkout(self, main_repo: Path):
        result = seed_worktree_graph(main_repo)
        assert result.status == "skipped"
        assert not (main_repo / ".dagayn" / "graph.db").exists()

    def test_skips_when_main_has_no_graph(self, linked_worktree: Path):
        result = seed_worktree_graph(linked_worktree)
        assert result.status == "skipped"
        assert "no graph" in result.reason

    def test_env_var_disables_inheritance(self, main_repo, linked_worktree, monkeypatch):
        _write_graph_db(main_repo / ".dagayn" / "graph.db", head_sha="abc", repo_root=main_repo)
        monkeypatch.setenv(SEED_ENV_VAR, "0")

        result = seed_worktree_graph(linked_worktree)

        assert result.status == "skipped"
        assert not (linked_worktree / ".dagayn" / "graph.db").exists()

    def test_explicit_data_dir_disables_inheritance(self, main_repo, linked_worktree, monkeypatch):
        _write_graph_db(main_repo / ".dagayn" / "graph.db", head_sha="abc", repo_root=main_repo)
        monkeypatch.setenv("CRG_DATA_DIR", str(main_repo / ".dagayn"))

        result = seed_worktree_graph(linked_worktree)

        assert result.status == "skipped"

    def test_wal_content_is_included(self, main_repo: Path, linked_worktree: Path):
        """A plain file copy would lose uncommitted WAL pages; backup() keeps them."""
        source = main_repo / ".dagayn" / "graph.db"
        _write_graph_db(source, head_sha="abc", repo_root=main_repo)
        conn = sqlite3.connect(str(source))
        try:
            conn.execute("PRAGMA journal_mode=WAL")
            conn.execute("INSERT INTO nodes (file_path) VALUES ('late.py')")
            conn.commit()
        finally:
            conn.close()

        assert seed_worktree_graph(linked_worktree).status == "seeded"

        dest = linked_worktree / ".dagayn" / "graph.db"
        conn = sqlite3.connect(str(dest))
        try:
            rows = [row[0] for row in conn.execute("SELECT file_path FROM nodes").fetchall()]
        finally:
            conn.close()
        assert "late.py" in rows

    def test_ensure_never_raises(self, tmp_path: Path):
        missing = tmp_path / "nope"
        assert ensure_worktree_graph(missing).status in ("skipped", "failed")


class TestResolveHookRepo:
    def test_prefers_worktree_path_from_tool_response(
        self, main_repo, linked_worktree, monkeypatch
    ):
        monkeypatch.chdir(main_repo)
        payload = {
            "hook_event_name": "PostToolUse",
            "tool_name": "EnterWorktree",
            "cwd": str(main_repo),
            "tool_response": {"worktree_path": str(linked_worktree)},
        }
        resolved = resolve_hook_repo(payload)
        assert resolved is not None
        assert resolved.resolve() == linked_worktree.resolve()

    def test_uses_cursor_workspace_roots(self, linked_worktree: Path, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        payload = {"hook_event_name": "sessionStart", "workspace_roots": [str(linked_worktree)]}
        resolved = resolve_hook_repo(payload)
        assert resolved is not None
        assert resolved.resolve() == linked_worktree.resolve()

    def test_uses_file_path_for_edit_hooks(self, linked_worktree: Path, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        payload = {"file_path": str(linked_worktree / "hello.py")}
        resolved = resolve_hook_repo(payload)
        assert resolved is not None
        assert resolved.resolve() == linked_worktree.resolve()

    def test_falls_back_to_project_dir_env(self, linked_worktree: Path, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        monkeypatch.setenv("CURSOR_PROJECT_DIR", str(linked_worktree))
        monkeypatch.delenv("CLAUDE_PROJECT_DIR", raising=False)
        resolved = resolve_hook_repo({})
        assert resolved is not None
        assert resolved.resolve() == linked_worktree.resolve()

    def test_no_cwd_fallback_returns_none(self, main_repo: Path, monkeypatch):
        monkeypatch.chdir(main_repo)
        monkeypatch.delenv("CURSOR_PROJECT_DIR", raising=False)
        monkeypatch.delenv("CLAUDE_PROJECT_DIR", raising=False)
        assert resolve_hook_repo({}, fallback_cwd=False) is None

    def test_cwd_fallback_resolves_repo(self, main_repo: Path, monkeypatch):
        monkeypatch.chdir(main_repo)
        monkeypatch.delenv("CURSOR_PROJECT_DIR", raising=False)
        monkeypatch.delenv("CLAUDE_PROJECT_DIR", raising=False)
        resolved = resolve_hook_repo({})
        assert resolved is not None
        assert resolved.resolve() == main_repo.resolve()

    def test_ignores_unusable_payloads(self):
        assert parse_hook_payload("") is None
        assert parse_hook_payload("not json") is None
        assert parse_hook_payload(json.dumps({"a": 1})) == {"a": 1}


class TestWorktreeInclude:
    def test_only_gitignored_existing_files_are_included(self, main_repo: Path):
        (main_repo / ".mcp.json").write_text("{}", encoding="utf-8")
        (main_repo / ".cursor").mkdir()
        (main_repo / ".cursor" / "mcp.json").write_text("{}", encoding="utf-8")

        patterns = worktree_include_patterns(main_repo)

        assert ".mcp.json" in patterns
        assert ".cursor/mcp.json" in patterns
        # .opencode.json was never created.
        assert ".opencode.json" not in patterns

    def test_tracked_files_are_excluded(self, main_repo: Path):
        (main_repo / ".opencode.json").write_text("{}", encoding="utf-8")
        _git(main_repo, "add", "-f", ".opencode.json")
        _git(main_repo, "commit", "-m", "track opencode config")

        assert is_gitignored(main_repo, ".opencode.json") is False
        assert ".opencode.json" not in worktree_include_patterns(main_repo)

    def test_creates_and_updates_managed_block(self, main_repo: Path):
        assert ensure_worktree_include(main_repo, [".mcp.json"]) == "created"
        content = (main_repo / ".worktreeinclude").read_text(encoding="utf-8")
        assert ".mcp.json" in content

        assert ensure_worktree_include(main_repo, [".mcp.json"]) == "unchanged"
        assert ensure_worktree_include(main_repo, [".mcp.json", ".cursor/mcp.json"]) == "updated"

        updated = (main_repo / ".worktreeinclude").read_text(encoding="utf-8")
        assert updated.count("dagayn worktree include") == 2  # start + end marker
        assert ".cursor/mcp.json" in updated

    def test_preserves_user_entries(self, main_repo: Path):
        path = main_repo / ".worktreeinclude"
        path.write_text(".env\n.env.local\n", encoding="utf-8")

        ensure_worktree_include(main_repo, [".mcp.json"])

        content = path.read_text(encoding="utf-8")
        assert ".env.local" in content
        assert ".mcp.json" in content

    def test_no_patterns_is_a_noop(self, main_repo: Path):
        assert ensure_worktree_include(main_repo, []) == "skipped"
        assert not (main_repo / ".worktreeinclude").exists()


class TestCopyWorktreeConfig:
    def test_copies_gitignored_mcp_config(self, main_repo: Path, linked_worktree: Path):
        (main_repo / ".mcp.json").write_text('{"mcpServers": {}}', encoding="utf-8")
        (main_repo / ".cursor").mkdir()
        (main_repo / ".cursor" / "mcp.json").write_text("{}", encoding="utf-8")

        copied = copy_worktree_config(linked_worktree)

        assert ".mcp.json" in copied
        assert ".cursor/mcp.json" in copied
        assert (linked_worktree / ".mcp.json").read_text(encoding="utf-8") == '{"mcpServers": {}}'
        assert (linked_worktree / ".cursor" / "mcp.json").exists()

    def test_copies_skill_directories(self, main_repo: Path, linked_worktree: Path):
        skills = main_repo / ".claude" / "skills"
        skills.mkdir(parents=True)
        (skills / "graph.md").write_text("# skill", encoding="utf-8")

        copied = copy_worktree_config(linked_worktree)

        assert ".claude/skills" in copied
        assert (linked_worktree / ".claude" / "skills" / "graph.md").exists()

    def test_never_overwrites_worktree_config(self, main_repo: Path, linked_worktree: Path):
        (main_repo / ".mcp.json").write_text('{"from": "main"}', encoding="utf-8")
        (linked_worktree / ".mcp.json").write_text('{"from": "worktree"}', encoding="utf-8")

        copied = copy_worktree_config(linked_worktree)

        assert ".mcp.json" not in copied
        assert (linked_worktree / ".mcp.json").read_text(encoding="utf-8") == '{"from": "worktree"}'

    def test_noop_in_main_checkout(self, main_repo: Path):
        (main_repo / ".mcp.json").write_text("{}", encoding="utf-8")
        assert copy_worktree_config(main_repo) == []


class TestCursorWorktreeSetup:
    def test_creates_config_with_sync_command(self, main_repo: Path):
        assert install_cursor_worktree_setup(main_repo) == "created"

        config = json.loads((main_repo / ".cursor" / "worktrees.json").read_text(encoding="utf-8"))
        assert config["setup-worktree"] == ["dagayn session prepare --budget-seconds 45"]

    def test_preserves_user_commands_and_is_idempotent(self, main_repo: Path):
        config_path = main_repo / ".cursor" / "worktrees.json"
        config_path.parent.mkdir(parents=True)
        config_path.write_text(json.dumps({"setup-worktree-unix": ["npm ci"]}), encoding="utf-8")

        assert install_cursor_worktree_setup(main_repo) == "updated"
        assert install_cursor_worktree_setup(main_repo) == "unchanged"

        config = json.loads(config_path.read_text(encoding="utf-8"))
        assert config["setup-worktree-unix"] == [
            "npm ci",
            "dagayn session prepare --budget-seconds 45",
        ]
        # The generic key is left alone when an OS-specific key already exists.
        assert "setup-worktree" not in config

    def test_leaves_setup_scripts_to_the_user(self, main_repo: Path):
        config_path = main_repo / ".cursor" / "worktrees.json"
        config_path.parent.mkdir(parents=True)
        config_path.write_text(
            json.dumps({"setup-worktree": "./scripts/setup-worktree.sh"}), encoding="utf-8"
        )

        assert install_cursor_worktree_setup(main_repo) == "manual"
        config = json.loads(config_path.read_text(encoding="utf-8"))
        assert config["setup-worktree"] == "./scripts/setup-worktree.sh"

    def test_dry_run_writes_nothing(self, main_repo: Path):
        assert install_cursor_worktree_setup(main_repo, dry_run=True) == "created"
        assert not (main_repo / ".cursor" / "worktrees.json").exists()


class TestWorktreeHookEntry:
    def test_claude_hooks_include_worktree_sync(self, tmp_path: Path):
        config = generate_hooks_config(tmp_path)
        commands = [
            hook["command"] for entry in config["hooks"]["PostToolUse"] for hook in entry["hooks"]
        ]
        assert any("dagayn session prepare --from-hook" in command for command in commands)
        matchers = [entry["matcher"] for entry in config["hooks"]["PostToolUse"]]
        assert "EnterWorktree|ExitWorktree" in matchers

    def test_worktree_hook_can_be_disabled(self, tmp_path: Path):
        config = generate_hooks_config(tmp_path, worktree_hook=False)
        matchers = [entry["matcher"] for entry in config["hooks"]["PostToolUse"]]
        assert "EnterWorktree|ExitWorktree" not in matchers

    def test_repo_resolution_prefers_git_then_project_dir(self, tmp_path: Path):
        config = generate_hooks_config(tmp_path)
        command = config["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
        assert command.index("git rev-parse --show-toplevel") < command.index("CLAUDE_PROJECT_DIR")
        assert '[ -n "$repo" ]' in command

    def test_worktree_sync_carries_extra_update_args(self, tmp_path: Path):
        config = generate_hooks_config(tmp_path, extra_update_args=["--local-embedding"])
        command = next(
            hook["command"]
            for entry in config["hooks"]["PostToolUse"]
            for hook in entry["hooks"]
            if "session prepare" in hook["command"]
        )
        assert "--local-embedding" in command
