"""Shared git worktree helpers for freshness and worktree tests."""

from __future__ import annotations

import sqlite3
import subprocess
from pathlib import Path


def git(repo: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false", *args],
        capture_output=True,
        text=True,
        cwd=str(repo),
        timeout=10,
    )


def write_minimal_graph_db(path: Path, *, head_sha: str, repo_root: Path) -> None:
    """Create a minimal graph.db with the metadata dagayn stores."""
    path.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(str(path))
    try:
        conn.execute("CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT)")
        conn.execute("CREATE TABLE nodes (id INTEGER PRIMARY KEY, file_path TEXT)")
        conn.executemany(
            "INSERT INTO metadata (key, value) VALUES (?, ?)",
            [
                ("git_head_sha", head_sha),
                ("git_branch", "main"),
                ("repo_root", str(repo_root)),
            ],
        )
        conn.execute("INSERT INTO nodes (file_path) VALUES ('hello.py')")
        conn.commit()
    finally:
        conn.close()


def graph_metadata(db_path: Path, key: str) -> str | None:
    conn = sqlite3.connect(str(db_path))
    try:
        row = conn.execute("SELECT value FROM metadata WHERE key = ?", (key,)).fetchone()
    finally:
        conn.close()
    return row[0] if row else None
