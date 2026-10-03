"""The Rust ``dagayn`` build and update write what the Python CLI writes.

Each snapshot fixture is copied into two git repositories; one is built and
then updated through three rounds of edits by the Python CLI, the other by the
binary named in ``DAGAYN_RUST_CLI``. After every round the graph (every node
and edge), the metadata, and the printed summary must match. Skipped when the
variable is unset; CI sets it to the binary it builds.
"""

from __future__ import annotations

import os
import shlex
import sqlite3
import subprocess
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent / "tools"))
import mcp_snapshot  # noqa: E402
from parity_export import export_db  # noqa: E402

RUST_CLI = os.environ.get("DAGAYN_RUST_CLI", "")
PYTHON_CLI = [sys.executable, "-m", "dagayn"]
#: Metadata the two runs cannot share: when it happened and where.
_RUN_KEYS = ("last_updated", "last_postprocessed_at", "fts_indexed_at", "repo_root")

pytestmark = pytest.mark.skipif(not RUST_CLI, reason="DAGAYN_RUST_CLI is not set")


def _git(repo: Path, env: dict[str, str], *args: str) -> None:
    subprocess.run(
        ["git", *args],
        cwd=repo,
        env={**env, **mcp_snapshot._GIT_IDENTITY},
        check=True,
        capture_output=True,
    )


def _edit(repo: Path, env: dict[str, str], round_: int) -> None:
    sources = sorted(
        path
        for path in repo.rglob("*")
        if path.is_file()
        and ".git" not in path.parts
        and ".dagayn" not in path.parts
        and path.suffix in {".py", ".ts", ".js", ".tf", ".md", ".toml", ".json"}
    )
    if round_ == 0:
        # Uncommitted edit plus an untracked file.
        if sources:
            sources[0].write_text(sources[0].read_text() + "\n\n# edited\n")
        (repo / "added_new.py").write_text("def brand_new():\n    return 1\n")
    elif round_ == 1:
        # Committed deletion; the round-0 changes are committed with it.
        if len(sources) > 1:
            _git(repo, env, "rm", "-qf", str(sources[1].relative_to(repo)))
        _git(repo, env, "add", "-A")
        _git(repo, env, "commit", "-qm", "round 1", "--no-gpg-sign")
    else:
        # Committed rename.
        (repo / "added_new.py").rename(repo / "renamed_new.py")
        _git(repo, env, "add", "-A")
        _git(repo, env, "commit", "-qm", "round 2", "--no-gpg-sign")


def _state(repo: Path, stdout: str) -> tuple[str, dict[str, str], str]:
    db = repo / ".dagayn" / "graph.db"
    conn = sqlite3.connect(db)
    try:
        metadata = dict(conn.execute("SELECT key, value FROM metadata").fetchall())
    finally:
        conn.close()
    for key in _RUN_KEYS:
        metadata.pop(key, None)
    return export_db(db, entity_lines=True), metadata, stdout.splitlines()[0]


def _run(name: str, cli: list[str], flags: list[str]) -> list[tuple[str, dict[str, str], str]]:
    states = []
    with mcp_snapshot.built_fixture(name) as (repo, env):
        for round_ in range(3):
            _edit(repo, env, round_)
            done = subprocess.run(
                [*cli, "update", "--repo", str(repo), *flags],
                cwd=repo,
                env=env,
                capture_output=True,
                text=True,
                check=False,
            )
            assert done.returncode == 0, done.stderr
            states.append(_state(repo, done.stdout))
    return states


@pytest.mark.parametrize("flags", [[], ["--skip-flows"]], ids=["full", "skip_flows"])
@pytest.mark.parametrize("name", sorted(mcp_snapshot.FIXTURE_CASES))
def test_rust_update_matches_python(name: str, flags: list[str]) -> None:
    python = _run(name, PYTHON_CLI, flags)
    rust = _run(name, shlex.split(RUST_CLI), flags)
    for round_, (expected, actual) in enumerate(zip(python, rust, strict=True)):
        assert actual[2] == expected[2], f"round {round_}: summary differs"
        assert actual[1] == expected[1], f"round {round_}: metadata differs"
        assert actual[0] == expected[0], f"round {round_}: graph differs"


def _add_embeddings(db: Path) -> None:
    """Two provider partitions, one orphan vector, and a stored active provider."""
    conn = sqlite3.connect(db)
    try:
        conn.execute(
            "CREATE TABLE IF NOT EXISTS embeddings (qualified_name TEXT NOT NULL, "
            "vector BLOB NOT NULL, text_hash TEXT NOT NULL, "
            "provider TEXT NOT NULL DEFAULT 'unknown', PRIMARY KEY (qualified_name, provider))"
        )
        names = [
            row[0]
            for row in conn.execute(
                "SELECT qualified_name FROM nodes WHERE kind != 'File' ORDER BY qualified_name"
            )
        ]
        rows = [(name, b"\0", "h", "Model#dim=8") for name in names[:-1]]
        rows += [(name, b"\0", "h", "old-model") for name in names]
        rows.append(("gone.py::vanished", b"\0", "h", "Model#dim=8"))
        conn.executemany("INSERT INTO embeddings VALUES (?, ?, ?, ?)", rows)
        conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES ('embedding_provider', 'model')"
        )
        conn.commit()
    finally:
        conn.close()


def _status(cli: list[str], repo: Path, env: dict[str, str]) -> str:
    done = subprocess.run(
        [*cli, "status", "--repo", str(repo)],
        cwd=repo,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    assert done.returncode == 0, done.stderr
    return done.stdout


@pytest.mark.parametrize("name", sorted(mcp_snapshot.FIXTURE_CASES))
def test_rust_status_matches_python(name: str) -> None:
    rust = shlex.split(RUST_CLI)
    with mcp_snapshot.built_fixture(name) as (repo, env):
        seen = []

        def check() -> None:
            expected = _status(PYTHON_CLI, repo, env)
            assert _status(rust, repo, env) == expected
            seen.append(expected.splitlines()[-1])

        check()  # commit_synced
        _add_embeddings(repo / ".dagayn" / "graph.db")
        check()  # embedding coverage across two partitions, one orphan vector
        (repo / "added_new.py").write_text("def brand_new():\n    return 1\n")
        check()  # worktree_behind
        subprocess.run([*PYTHON_CLI, "update", "--repo", str(repo)], env=env, check=True)
        check()  # worktree_ahead
        _git(repo, env, "add", "-A")
        _git(repo, env, "commit", "-qm", "commit the edit", "--no-gpg-sign")
        check()  # commit_drift
        _git(repo, env, "checkout", "-q", "-b", "other")
        check()  # another branch at the same commit
    assert len(set(seen)) >= 3, seen


def test_rust_status_of_a_repository_without_a_graph(tmp_path: Path) -> None:
    rust = shlex.split(RUST_CLI)
    env = mcp_snapshot._isolated_env(tmp_path)
    for label in ("python", "rust"):
        repo = tmp_path / label
        repo.mkdir()
        (repo / "app.py").write_text("def main():\n    pass\n")
        mcp_snapshot._git_commit_all(repo, env)
    assert _status(rust, tmp_path / "rust", env) == _status(PYTHON_CLI, tmp_path / "python", env)
