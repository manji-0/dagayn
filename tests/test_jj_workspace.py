"""Tests for jj (Jujutsu) secondary workspace support.

``jj workspace add`` (used by ``track``) creates a working copy with ``.jj/``
but no ``.git``, nested under the main checkout at ``.worktrees/<slug>``. Git
commands run there answer for the main checkout. The synthetic tests pin the
on-disk layout rules without a jj binary; the ``real_jj`` tests exercise an
actual colocated repository and are skipped when jj is not installed.
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

import pytest
from worktree_fixtures import git as _git

from dagayn import jj_workspace
from dagayn.changes import parse_git_diff
from dagayn.graph import GraphStore
from dagayn.incremental import (
    _git_branch_info,
    collect_all_files,
    detect_vcs,
    find_repo_root,
    full_build,
    get_changed_file_sources,
    get_db_path,
    incremental_update,
    is_project_root,
)
from dagayn.incremental_files import resolve_commit_sha
from dagayn.jj_workspace import JjWorkspaceError
from dagayn.skills import _SHELL_JJ_WORKSPACE_NARROWING
from dagayn.tools.sync_status import assess_graph_sync
from dagayn.worktree import (
    is_gitignored,
    is_linked_worktree,
    main_worktree_root,
    resolve_hook_repo,
    seed_worktree_graph,
)

real_jj = pytest.mark.skipif(shutil.which("jj") is None, reason="jj is not installed")


def _synthetic_workspace(main_repo: Path, *, relative: bool) -> Path:
    """Lay out ``.jj`` metadata the way jj does, without running jj."""
    store = main_repo / ".jj" / "repo" / "store"
    store.mkdir(parents=True)
    (store / "git_target").write_text("../../../.git", encoding="utf-8")
    workspace = main_repo / ".worktrees" / "task"
    (workspace / ".jj").mkdir(parents=True)
    target = "../../../.jj/repo" if relative else str(main_repo / ".jj" / "repo")
    (workspace / ".jj" / "repo").write_text(target, encoding="utf-8")
    return workspace


class TestSyntheticLayout:
    @pytest.mark.parametrize("relative", [False, True])
    def test_git_dir_follows_repo_pointer(self, main_repo: Path, relative: bool):
        workspace = _synthetic_workspace(main_repo, relative=relative)
        assert jj_workspace.jj_git_dir(workspace) == (main_repo / ".git").resolve()
        assert jj_workspace.main_workspace_root(workspace) == main_repo.resolve()

    def test_colocated_main_checkout_stays_git(self, main_repo: Path):
        _synthetic_workspace(main_repo, relative=False)
        assert jj_workspace.is_jj_workspace(main_repo) is False
        assert detect_vcs(main_repo) == "git"

    def test_workspace_is_its_own_root(self, main_repo: Path):
        workspace = _synthetic_workspace(main_repo, relative=False)
        nested = workspace / "pkg"
        nested.mkdir()
        assert find_repo_root(nested) == workspace
        assert detect_vcs(workspace) == "jj"
        assert is_project_root(workspace) is True

    def test_workspace_counts_as_linked_worktree(self, main_repo: Path):
        workspace = _synthetic_workspace(main_repo, relative=False)
        main = main_worktree_root(workspace)
        assert main is not None and main.resolve() == main_repo.resolve()
        assert is_linked_worktree(workspace) is True

    def test_hook_payload_file_resolves_to_workspace(self, main_repo: Path):
        workspace = _synthetic_workspace(main_repo, relative=False)
        edited = workspace / "feature.py"
        edited.write_text("x = 1\n", encoding="utf-8")
        resolved = resolve_hook_repo({"file_path": str(edited)}, fallback_cwd=False)
        assert resolved is not None and resolved.resolve() == workspace.resolve()

    def test_unbacked_jj_directory_is_ignored(self, tmp_path: Path):
        orphan = tmp_path / "orphan"
        (orphan / ".jj").mkdir(parents=True)
        assert jj_workspace.is_jj_workspace(orphan) is False
        assert detect_vcs(orphan) == "none"


def _jj(cwd: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["jj", "--no-pager", "--color=never", *args],
        capture_output=True,
        text=True,
        cwd=str(cwd),
        timeout=30,
        check=True,
    )


@pytest.fixture()
def jj_env(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    config = tmp_path / "jj-config.toml"
    config.write_text('[user]\nname = "Test"\nemail = "test@test.com"\n', encoding="utf-8")
    monkeypatch.setenv("JJ_CONFIG", str(config))


@pytest.fixture()
def jj_workspace_dir(main_repo: Path, jj_env: None) -> Path:
    """A ``track``-style workspace: ``.worktrees/task`` on ``main``."""
    exclude = main_repo / ".git" / "info" / "exclude"
    exclude.parent.mkdir(parents=True, exist_ok=True)
    exclude.write_text(".worktrees/\n", encoding="utf-8")
    _jj(main_repo, "git", "init", "--colocate")
    workspace = main_repo / ".worktrees" / "task"
    workspace.parent.mkdir()
    _jj(main_repo, "workspace", "add", str(workspace), "-r", "main")
    return workspace


@real_jj
class TestRealJjWorkspace:
    def test_detects_workspace(self, main_repo: Path, jj_workspace_dir: Path):
        assert detect_vcs(jj_workspace_dir) == "jj"
        assert find_repo_root(jj_workspace_dir) == jj_workspace_dir
        head = _git(main_repo, "rev-parse", "HEAD").stdout.strip()
        assert _git_branch_info(jj_workspace_dir)[1] == head

    def test_files_come_from_the_workspace(self, main_repo: Path, jj_workspace_dir: Path):
        (jj_workspace_dir / "feature.py").write_text("def feature():\n    return 1\n")
        (main_repo / "main_only.py").write_text("def main_only():\n    return 2\n")

        files = collect_all_files(jj_workspace_dir)

        assert "feature.py" in files
        assert "hello.py" in files
        assert "main_only.py" not in files

    def test_working_copy_edits_are_unstaged_changes(self, jj_workspace_dir: Path):
        (jj_workspace_dir / "feature.py").write_text("def feature():\n    return 1\n")

        sources = get_changed_file_sources(jj_workspace_dir, "HEAD")

        assert sources["worktree"] == ["feature.py"]
        assert sources["unstaged"] == ["feature.py"]
        assert sources["base_diff"] == []

    def test_committed_changes_move_head(self, main_repo: Path, jj_workspace_dir: Path):
        main_sha = _git(main_repo, "rev-parse", "HEAD").stdout.strip()
        (jj_workspace_dir / "feature.py").write_text("def feature():\n    return 1\n")
        _jj(jj_workspace_dir, "commit", "-m", "feat: add feature")

        head = resolve_commit_sha(jj_workspace_dir, "HEAD")
        sources = get_changed_file_sources(jj_workspace_dir, main_sha)

        assert head is not None and head != main_sha
        assert _git_branch_info(jj_workspace_dir)[1] == head
        assert sources["base_diff"] == ["feature.py"]
        assert sources["worktree"] == []

    def test_diff_ranges_cover_the_working_copy(self, jj_workspace_dir: Path):
        (jj_workspace_dir / "hello.py").write_text(
            "def greet():\n    return 'hello'\n\n\ndef wave():\n    return 'bye'\n"
        )

        result = parse_git_diff(str(jj_workspace_dir), "HEAD")

        assert result.status == "ok"
        assert "hello.py" in result.ranges

    def test_ignore_rules_are_the_workspace_s(self, jj_workspace_dir: Path):
        assert is_gitignored(jj_workspace_dir, ".dagayn/graph.db") is True
        assert is_gitignored(jj_workspace_dir, "hello.py") is False

    def test_seeded_graph_catches_up_on_workspace_edits(
        self, main_repo: Path, jj_workspace_dir: Path
    ):
        main_store = GraphStore(get_db_path(main_repo))
        try:
            full_build(main_repo, main_store)
        finally:
            main_store.close()

        seed = seed_worktree_graph(jj_workspace_dir)
        assert seed.seeded and seed.dest is not None, seed.reason
        (jj_workspace_dir / "feature.py").write_text("def feature():\n    return 1\n")

        store = GraphStore(seed.dest)
        try:
            incremental_update(jj_workspace_dir, store, base=seed.base_sha or "HEAD")
            names = {node.name for node in store.get_nodes_by_file("feature.py")}
        finally:
            store.close()
        assert "feature" in names


def _make_stale(main_repo: Path) -> None:
    """Rewrite the workspace's commit from the main workspace, as a rebase would."""
    (main_repo / "moved.py").write_text("MOVED = 1\n", encoding="utf-8")
    _jj(main_repo, "commit", "-m", "main moves")
    _jj(main_repo, "rebase", "-s", "task@", "-d", "@-")


@real_jj
class TestStaleJjWorkspace:
    def test_file_set_refuses_instead_of_reporting_empty(
        self, main_repo: Path, jj_workspace_dir: Path
    ):
        _make_stale(main_repo)

        with pytest.raises(JjWorkspaceError, match="update-stale"):
            collect_all_files(jj_workspace_dir)
        with pytest.raises(JjWorkspaceError, match="update-stale"):
            get_changed_file_sources(jj_workspace_dir, "HEAD")

    def test_existing_graph_is_not_reported_fresh(self, main_repo: Path, jj_workspace_dir: Path):
        store = GraphStore(get_db_path(jj_workspace_dir))
        try:
            full_build(jj_workspace_dir, store)
            _make_stale(main_repo)

            sync = assess_graph_sync(store, jj_workspace_dir)
        finally:
            store.close()

        assert sync["state"] == "commit_drift"


@real_jj
class TestShellNarrowing:
    @staticmethod
    def _resolve(cwd: Path) -> str:
        script = (
            'repo="$(git rev-parse --show-toplevel 2>/dev/null || true)"; '
            f'{_SHELL_JJ_WORKSPACE_NARROWING}; printf "%s" "$repo"'
        )
        return subprocess.run(
            ["bash", "-c", script], capture_output=True, text=True, cwd=str(cwd), timeout=30
        ).stdout

    def test_narrows_to_nested_workspace(self, jj_workspace_dir: Path):
        assert Path(self._resolve(jj_workspace_dir)).resolve() == jj_workspace_dir.resolve()

    def test_main_checkout_is_unchanged(self, main_repo: Path, jj_workspace_dir: Path):
        assert Path(self._resolve(main_repo)).resolve() == main_repo.resolve()
