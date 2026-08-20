"""Regression tests for wrong-repository resolution.

A user-level MCP entry (Cursor's ``~/.cursor/mcp.json``) launches the server
with neither ``cwd`` nor ``--repo``, so the repository is resolved from an
ambient working directory. When that directory belonged to another checkout —
or was ``$HOME`` — tools answered from the wrong repository's graph, and
nothing in the response or the logs said so.
"""

from __future__ import annotations

import sqlite3
from pathlib import Path

import pytest

from dagayn.incremental_files import find_project_root
from dagayn.paths import ALLOW_WIDE_ROOT_ENV, recorded_repo_root, unsafe_root_reason
from dagayn.tools._common import (
    RepoRootMismatchError,
    _get_store,
    attach_repo_context,
    repo_context_snapshot,
    reset_repo_context,
)

_HINT_VARS = ("CRG_REPO_ROOT", "CURSOR_PROJECT_DIR", "CLAUDE_PROJECT_DIR", "WORKSPACE_FOLDER_PATHS")


@pytest.fixture(autouse=True)
def _clear_env(monkeypatch):
    for var in _HINT_VARS + (ALLOW_WIDE_ROOT_ENV,):
        monkeypatch.delenv(var, raising=False)


def _git_repo(path: Path) -> Path:
    (path / ".git").mkdir(parents=True)
    return path


def _graph_recording(db_path: Path, repo_root: Path | str) -> Path:
    """Write a minimal graph whose metadata claims *repo_root*."""
    db_path.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(db_path)
    try:
        conn.execute("CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT)")
        conn.execute(
            "INSERT INTO metadata (key, value) VALUES ('repo_root', ?)",
            (str(repo_root),),
        )
        conn.commit()
    finally:
        conn.close()
    return db_path


class TestUnsafeRootReason:
    def test_home_directory_is_rejected(self, tmp_path, monkeypatch):
        home = tmp_path / "home"
        home.mkdir()
        monkeypatch.setenv("HOME", str(home))
        monkeypatch.setenv("USERPROFILE", str(home))
        assert unsafe_root_reason(home) == "your home directory"

    def test_filesystem_root_is_rejected(self, tmp_path):
        assert unsafe_root_reason(Path(tmp_path.anchor)) == "the filesystem root"

    def test_ordinary_directory_is_fine(self, tmp_path, monkeypatch):
        monkeypatch.setenv("HOME", str(tmp_path / "home"))
        monkeypatch.setenv("USERPROFILE", str(tmp_path / "home"))
        assert unsafe_root_reason(tmp_path / "project") is None

    def test_env_override_allows_home(self, tmp_path, monkeypatch):
        home = tmp_path / "home"
        home.mkdir()
        monkeypatch.setenv("HOME", str(home))
        monkeypatch.setenv("USERPROFILE", str(home))
        monkeypatch.setenv(ALLOW_WIDE_ROOT_ENV, "1")
        assert unsafe_root_reason(home) is None


class TestRecordedRepoRoot:
    def test_reads_metadata(self, tmp_path):
        db = _graph_recording(tmp_path / "graph.db", "/somewhere/else")
        assert recorded_repo_root(db) == Path("/somewhere/else")

    def test_missing_file_is_unknown(self, tmp_path):
        assert recorded_repo_root(tmp_path / "absent.db") is None

    def test_graph_without_metadata_is_unknown(self, tmp_path):
        db = tmp_path / "bare.db"
        sqlite3.connect(db).close()
        assert recorded_repo_root(db) is None


class TestGetStoreRepoGuard:
    def test_rejects_graph_describing_another_repo(self, tmp_path):
        repo = _git_repo(tmp_path / "here")
        other = _git_repo(tmp_path / "there")
        _graph_recording(repo / ".dagayn" / "graph.db", other)

        with pytest.raises(RepoRootMismatchError) as excinfo:
            _get_store(str(repo))
        assert str(other) in str(excinfo.value)

    def test_accepts_graph_of_a_moved_checkout(self, tmp_path):
        """A recorded root that no longer exists means the checkout moved."""
        repo = _git_repo(tmp_path / "moved")
        _graph_recording(repo / ".dagayn" / "graph.db", tmp_path / "gone")

        store, root = _get_store(str(repo), cached=False)
        try:
            assert root == repo.resolve()
        finally:
            store.close()

    def test_accepts_matching_graph(self, tmp_path):
        repo = _git_repo(tmp_path / "match")
        _graph_recording(repo / ".dagayn" / "graph.db", repo.resolve())

        store, root = _get_store(str(repo), cached=False)
        try:
            assert root == repo.resolve()
        finally:
            store.close()

    def test_rejects_auto_detected_home_root(self, tmp_path, monkeypatch):
        home = tmp_path / "home"
        home.mkdir()
        monkeypatch.setenv("HOME", str(home))
        monkeypatch.setenv("USERPROFILE", str(home))
        monkeypatch.setattr("dagayn.tools._common.find_project_root", lambda: home)

        with pytest.raises(ValueError, match="home directory"):
            _get_store()
        # The refusal must not leave a data directory behind there.
        assert not (home / ".dagayn").exists()


class TestRepoContext:
    def test_get_store_records_the_repository(self, tmp_path):
        repo = _git_repo(tmp_path / "ctx")
        reset_repo_context()
        store, _root = _get_store(str(repo), cached=False)
        store.close()

        record = repo_context_snapshot()
        assert record is not None
        assert record["repo_root"] == str(repo.resolve())
        assert record["db_path"] == str(repo.resolve() / ".dagayn" / "graph.db")
        assert record["source"] == "explicit"

    def test_attach_repo_context_adds_repo_field(self, tmp_path):
        repo = _git_repo(tmp_path / "attach")
        store, _root = _get_store(str(repo), cached=False)
        store.close()

        payload = attach_repo_context({"status": "ok"})
        assert payload["_repo"]["repo_root"] == str(repo.resolve())

    def test_attach_is_a_noop_without_a_resolution(self):
        reset_repo_context()
        assert "_repo" not in attach_repo_context({"status": "ok"})

    def test_mcp_wrapper_attaches_repo_to_responses(self, tmp_path, monkeypatch):
        from dagayn import main
        from dagayn.tools import _common

        repo = _git_repo(tmp_path / "wrapped")

        def fake_list_graph_stats(**_kwargs):
            _common.set_repo_context(repo, repo / ".dagayn" / "graph.db", explicit=True)
            return {"status": "ok"}

        monkeypatch.setattr("dagayn.tools.list_graph_stats", fake_list_graph_stats, raising=False)
        payload = main._tool("list_graph_stats")(repo_root=str(repo))
        assert payload["_repo"]["repo_root"] == str(repo)

    def test_mcp_wrapper_does_not_leak_a_previous_repo(self, tmp_path, monkeypatch):
        from dagayn import main
        from dagayn.tools import _common

        repo = _git_repo(tmp_path / "stale")
        _common.set_repo_context(repo, repo / ".dagayn" / "graph.db", explicit=True)

        monkeypatch.setattr(
            "dagayn.tools.list_graph_stats",
            lambda **_kwargs: {"status": "ok"},
            raising=False,
        )
        payload = main._tool("list_graph_stats")()
        assert "_repo" not in payload


class TestFindProjectRootAmbientCwd:
    def test_workspace_hint_beats_unrelated_ambient_cwd(self, tmp_path, monkeypatch):
        """cwd inherited from a shell in repo A must not answer for workspace B."""
        shell_repo = _git_repo(tmp_path / "repo-a")
        workspace = _git_repo(tmp_path / "repo-b")
        monkeypatch.chdir(shell_repo)
        monkeypatch.setenv("WORKSPACE_FOLDER_PATHS", str(workspace))

        assert find_project_root() == workspace.resolve()

    def test_ambient_cwd_wins_when_it_is_inside_the_workspace(self, tmp_path, monkeypatch):
        workspace = _git_repo(tmp_path / "ws")
        nested = _git_repo(workspace / "vendored")
        monkeypatch.chdir(nested)
        monkeypatch.setenv("WORKSPACE_FOLDER_PATHS", str(workspace))

        assert find_project_root() == nested.resolve()

    def test_explicit_start_beats_workspace_hint(self, tmp_path, monkeypatch):
        explicit = _git_repo(tmp_path / "explicit")
        workspace = _git_repo(tmp_path / "hinted")
        monkeypatch.setenv("WORKSPACE_FOLDER_PATHS", str(workspace))

        assert find_project_root(explicit) == explicit.resolve()

    def test_multi_root_prefers_the_folder_holding_the_cwd(self, tmp_path, monkeypatch):
        """Graph mtime must not outrank the workspace the caller is standing in."""
        quiet = _git_repo(tmp_path / "quiet")
        busy = _git_repo(tmp_path / "busy")
        graph = busy / ".dagayn" / "graph.db"
        graph.parent.mkdir(parents=True)
        graph.write_bytes(b"sqlite")  # busy looks richer, but cwd is in quiet
        monkeypatch.chdir(quiet)
        monkeypatch.setenv("WORKSPACE_FOLDER_PATHS", f"{busy},{quiet}")

        assert find_project_root() == quiet.resolve()


class TestSessionPrepareResolution:
    def test_honors_workspace_hint_over_home_cwd(self, tmp_path, monkeypatch):
        from dagayn.tools.session_prepare import _resolve_repo

        home = tmp_path / "home"
        home.mkdir()
        workspace = _git_repo(tmp_path / "project")
        monkeypatch.chdir(home)
        monkeypatch.setenv("CURSOR_PROJECT_DIR", str(workspace))

        assert _resolve_repo(None) == workspace.resolve()
