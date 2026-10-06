"""Where graph data lives: ``CRG_DATA_DIR`` adoption, migration and failure paths.

The basic ``<repo>/.dagayn`` and ``CRG_DATA_DIR`` layouts are covered in
``test_incremental.py`` (``TestDataDir``); these pin what happens when an
existing graph has to be found under another name, or cannot be moved.
"""

from __future__ import annotations

import logging
import os
import sqlite3
import sys
from pathlib import Path

import pytest

from dagayn.paths import data_dir_for, get_data_dir, repo_slug, same_repo_path

needs_posix_permissions = pytest.mark.skipif(
    sys.platform == "win32" or (hasattr(os, "geteuid") and os.geteuid() == 0),
    reason="needs POSIX permission bits enforced (not Windows, not root)",
)


def _graph_recording(db_path: Path, repo_root: Path | None) -> Path:
    """Write a minimal graph; ``repo_root=None`` leaves the metadata out."""
    db_path.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(db_path)
    try:
        conn.execute("CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT)")
        if repo_root is not None:
            conn.execute(
                "INSERT INTO metadata (key, value) VALUES ('repo_root', ?)",
                (str(repo_root),),
            )
        conn.commit()
    finally:
        conn.close()
    return db_path


@pytest.fixture()
def read_only(request: pytest.FixtureRequest):
    """Make directories read-only for one test, restoring them afterwards."""
    changed: list[Path] = []

    def _apply(path: Path) -> None:
        path.chmod(0o555)
        changed.append(path)

    def _restore() -> None:
        for path in changed:
            path.chmod(0o755)

    request.addfinalizer(_restore)
    return _apply


@pytest.fixture()
def shared(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> tuple[Path, Path]:
    """``(CRG_DATA_DIR, repo)`` with the variable exported."""
    external = tmp_path / "graphs"
    external.mkdir()
    repo = tmp_path / "project"
    repo.mkdir()
    monkeypatch.setenv("CRG_DATA_DIR", str(external))
    return external.resolve(), repo


class TestRepoSlug:
    def test_missing_checkout_gets_one_slug_for_every_spelling(self, tmp_path):
        """Before the directory exists there is no inode; ``..`` must not split it."""
        missing = tmp_path / "not-cloned-yet"
        spelled_with_dots = tmp_path / "elsewhere" / ".." / "not-cloned-yet"

        assert repo_slug(missing) == repo_slug(spelled_with_dots)
        assert repo_slug(missing).startswith("not-cloned-yet-")
        assert repo_slug(tmp_path / "a b!c").startswith("a-b-c-")


@pytest.mark.skipif(sys.platform == "win32", reason="symlinks need privileges on Windows")
class TestSameRepoPath:
    def test_symlink_and_target_are_the_same_repo(self, tmp_path):
        real = tmp_path / "real"
        real.mkdir()
        link = tmp_path / "link"
        link.symlink_to(real, target_is_directory=True)

        assert same_repo_path(link, real)
        assert same_repo_path(str(real / "sub" / ".."), real)
        assert not same_repo_path(real, tmp_path)

    def test_missing_paths_compare_by_resolved_text(self, tmp_path):
        assert same_repo_path(tmp_path / "gone" / ".." / "x", tmp_path / "x")
        assert not same_repo_path(tmp_path / "gone-a", tmp_path / "gone-b")


class TestSharedDataDirAdoption:
    def test_renamed_graph_directory_is_found_by_its_recorded_root(self, shared):
        """A graph under an outdated slug is still this repository's graph."""
        external, repo = shared
        old = external / "project-0123456789ab"
        _graph_recording(old / "graph.db", repo.resolve())

        # Lookup finds it without creating or moving anything.
        assert data_dir_for(repo) == old
        assert not (external / repo_slug(repo)).exists()

        # The mutating path moves it to the canonical name and keeps the graph.
        data_dir = get_data_dir(repo)
        assert data_dir == external / repo_slug(repo)
        assert not old.exists()
        assert (data_dir / "graph.db").is_file()
        assert (data_dir / ".gitignore").is_file()

    def test_graphs_of_other_repositories_are_not_adopted(self, shared, tmp_path):
        external, repo = shared
        other = tmp_path / "other"
        other.mkdir()
        foreign = external / "other-aaaaaaaaaaaa"
        _graph_recording(foreign / "graph.db", other.resolve())
        anonymous = external / "unknown-bbbbbbbbbbbb"
        _graph_recording(anonymous / "graph.db", None)
        (external / "stray-file").write_text("not a directory", encoding="utf-8")

        canonical = external / repo_slug(repo)
        assert data_dir_for(repo) == canonical
        assert get_data_dir(repo) == canonical
        assert (foreign / "graph.db").is_file()
        assert (anonymous / "graph.db").is_file()

    def test_shared_graph_side_files_are_dropped_after_the_move(self, shared):
        """Leftover side-files cannot be matched to a moved database; they must not follow it."""
        external, repo = shared
        legacy = _graph_recording(external / "graph.db", repo.resolve())
        # Empty, as a cleanly closed WAL-mode database leaves them.
        for suffix in ("-wal", "-shm", "-journal"):
            (external / f"graph.db{suffix}").write_bytes(b"")

        data_dir = get_data_dir(repo)

        assert (data_dir / "graph.db").is_file()
        assert not legacy.exists()
        for suffix in ("-wal", "-shm", "-journal"):
            assert not (external / f"graph.db{suffix}").exists()
            assert not (data_dir / f"graph.db{suffix}").exists()


@needs_posix_permissions
class TestUnwritableDataDirs:
    def test_unmovable_renamed_graph_is_used_in_place(self, shared, read_only, caplog):
        external, repo = shared
        old = external / "project-0123456789ab"
        _graph_recording(old / "graph.db", repo.resolve())
        read_only(external)

        with caplog.at_level(logging.WARNING, logger="dagayn.paths"):
            data_dir = get_data_dir(repo)

        assert data_dir == old
        assert (old / "graph.db").is_file()
        assert any("Could not move" in record.getMessage() for record in caplog.records)

    def test_unmovable_shared_graph_stays_where_it_is(self, shared, read_only, caplog):
        external, repo = shared
        canonical = external / repo_slug(repo)
        canonical.mkdir()
        legacy = _graph_recording(external / "graph.db", repo.resolve())
        read_only(external)

        with caplog.at_level(logging.WARNING, logger="dagayn.paths"):
            data_dir = get_data_dir(repo)

        assert data_dir == canonical
        assert legacy.is_file()
        assert not (canonical / "graph.db").exists()
        assert any("Could not move" in record.getMessage() for record in caplog.records)

    def test_read_only_data_dir_skips_the_inner_gitignore(self, tmp_path, monkeypatch, read_only):
        """The inner .gitignore is a best-effort guard, not a reason to fail lookup."""
        monkeypatch.delenv("CRG_DATA_DIR", raising=False)
        data_dir = tmp_path / ".dagayn"
        data_dir.mkdir()
        read_only(data_dir)

        assert get_data_dir(tmp_path) == data_dir
        assert not (data_dir / ".gitignore").exists()
