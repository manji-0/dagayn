"""Shared pytest fixtures for the dagayn test suite."""

from __future__ import annotations

import shutil
from pathlib import Path

import pytest

from dagayn.graph import GraphStore
from dagayn.incremental import full_build

PARITY_FIXTURE_DIR = Path(__file__).parent / "fixtures" / "parity"
PARITY_FIXTURE_NAMES = [
    "python_only",
    "terraform_only",
    "markdown_only",
    "notebook",
    "mixed",
]


def build_parity_fixture(source: Path, dest: Path) -> Path:
    """Copy fixture source files to dest, stub a .git dir, run full_build.

    Returns the path to the resulting graph.db.
    """
    for item in source.iterdir():
        if item.name in (".git", ".dagayn"):
            continue
        target = dest / item.name
        if item.is_dir():
            shutil.copytree(item, target)
        else:
            shutil.copy2(item, target)

    # A non-empty .git dir signals to collect_all_files that this is a git
    # repo root, causing it to fall back to rglob when git ls-files returns
    # nothing (no commits yet).  The file set is still deterministic because
    # dest contains only our fixture files.
    (dest / ".git").mkdir()

    db_path = dest / ".dagayn" / "graph.db"
    store = GraphStore(db_path)
    full_build(dest, store)
    store.close()
    return db_path


@pytest.fixture(scope="session")
def parity_fixture_dbs(tmp_path_factory):
    """Build all parity fixtures once per session; return mapping name → db_path."""
    dbs: dict[str, Path] = {}
    for name in PARITY_FIXTURE_NAMES:
        source = PARITY_FIXTURE_DIR / name
        dest = tmp_path_factory.mktemp(f"parity_{name}")
        dbs[name] = build_parity_fixture(source, dest)
    return dbs


@pytest.fixture()
def main_repo(tmp_path: Path) -> Path:
    """A git repo with one commit and gitignored dagayn paths."""
    import subprocess

    def _git(*args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["git", "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false", *args],
            capture_output=True,
            text=True,
            cwd=str(repo),
            timeout=10,
        )

    repo = tmp_path / "main"
    repo.mkdir()
    _git("init")
    _git("config", "user.email", "test@test.com")
    _git("config", "user.name", "Test")
    _git("checkout", "-B", "main")
    (repo / "hello.py").write_text("def greet():\n    return 'hello'\n", encoding="utf-8")
    (repo / ".gitignore").write_text(".dagayn/\n.mcp.json\n.cursor/\n", encoding="utf-8")
    _git("add", "hello.py", ".gitignore")
    _git("commit", "-m", "initial")
    return repo


@pytest.fixture()
def linked_worktree(main_repo: Path) -> Path:
    """A linked worktree of ``main_repo`` on its own branch."""
    import subprocess

    worktree = main_repo.parent / "wt-feature"
    result = subprocess.run(
        [
            "git",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
            "worktree",
            "add",
            "-b",
            "feature",
            str(worktree),
        ],
        capture_output=True,
        text=True,
        cwd=str(main_repo),
        timeout=10,
    )
    assert result.returncode == 0, result.stderr
    return worktree
