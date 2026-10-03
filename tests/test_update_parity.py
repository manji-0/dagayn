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
