"""Two SQLite libraries share this process: Python's ``sqlite3`` and the copy
compiled into ``dagayn._core``. Neither sees the other's locks, so a
writable connection one of them keeps open while the other closes its last
connection loses the WAL index, and every later connection of the process
fails with "disk I/O error". These tests pin the read paths that used to
leave such a connection behind.
"""

from __future__ import annotations

import sqlite3
import subprocess
import sys
from collections.abc import Iterator
from pathlib import Path

import pytest

from dagayn.search import _emb_cache, hybrid_search
from dagayn.tools._common import _evict_store_cache, _get_store

_PROVIDER_ENV = (
    "CRG_OPENAI_API_KEY",
    "CRG_OPENAI_BASE_URL",
    "CRG_OPENAI_MODEL",
    "GOOGLE_API_KEY",
    "MINIMAX_API_KEY",
)


@pytest.fixture
def unembedded_repo(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Iterator[Path]:
    """A built graph whose embeddings tables were never created, and no provider."""
    for name in _PROVIDER_ENV:
        monkeypatch.delenv(name, raising=False)
    repo = tmp_path / "repo"
    (repo / ".git").mkdir(parents=True)
    (repo / "guide.md").write_text("# Guide\n\nHow to use the guide.\n")
    (repo / "app.py").write_text("def guide():\n    return 1\n")
    subprocess.run(
        [sys.executable, "-m", "dagayn", "build", "--repo", str(repo)],
        check=True,
        capture_output=True,
    )
    conn = sqlite3.connect(repo / ".dagayn" / "graph.db")
    try:
        for name in (
            "embeddings_generation_insert",
            "embeddings_generation_update",
            "embeddings_generation_delete",
        ):
            conn.execute(f"DROP TRIGGER IF EXISTS {name}")
        conn.execute("DROP TABLE IF EXISTS embeddings_generation")
        conn.execute("DROP TABLE IF EXISTS embeddings")
        conn.commit()
    finally:
        conn.close()
    _emb_cache.clear()
    yield repo
    _emb_cache.clear()
    _evict_store_cache()


def _tables(db: Path) -> set[str]:
    conn = sqlite3.connect(db)
    try:
        return {row[0] for row in conn.execute("SELECT name FROM sqlite_master")}
    finally:
        conn.close()


def test_search_without_a_provider_leaves_no_connection_behind(unembedded_repo: Path) -> None:
    store, _root = _get_store(str(unembedded_repo), cached=False)
    try:
        result = hybrid_search(store, "guide")
    finally:
        store.close()

    assert result["results"], result
    db = unembedded_repo / ".dagayn" / "graph.db"
    # Raised "disk I/O error" once the native store's close removed the WAL
    # index from under the cached Python connection.
    tables = _tables(db)
    assert "nodes" in tables
    assert "embeddings" not in tables, "a search must not create the embeddings schema"
    assert not _emb_cache


def test_the_rust_build_creates_the_embeddings_schema(tmp_path: Path) -> None:
    """As the Python build does (its orphan-vector prune opens an EmbeddingStore)."""
    dagayn = Path(sys.executable).with_name("dagayn")
    if not dagayn.exists():
        pytest.skip("dagayn console script not installed")
    repo = tmp_path / "repo"
    (repo / ".git").mkdir(parents=True)
    (repo / "app.py").write_text("def main():\n    pass\n")
    subprocess.run([dagayn, "build", "--repo", repo], check=True, capture_output=True)
    tables = _tables(repo / ".dagayn" / "graph.db")
    assert {"embeddings", "embeddings_generation"} <= tables


def test_list_graph_stats_reads_the_graph_without_writing_it(unembedded_repo: Path) -> None:
    """It used to open an EmbeddingStore to count, creating the embeddings
    schema, which turned `dagayn status` from "not indexed" to "empty"."""
    from dagayn.tools.query import list_graph_stats

    assert list_graph_stats(repo_root=str(unembedded_repo))["embeddings_count"] == 0
    assert "embeddings" not in _tables(unembedded_repo / ".dagayn" / "graph.db")


def _python_side_reads() -> list:
    """Each way the package opens the graph with Python's ``sqlite3``."""
    from types import SimpleNamespace

    from dagayn.graph.sqlite_errors import borrowed_sqlite_connection, probe_graph_database
    from dagayn.tools._common import _data_version

    def borrowed(db: Path) -> None:
        with borrowed_sqlite_connection(SimpleNamespace(db_path=db)) as conn:
            conn.execute("SELECT count(*) FROM nodes").fetchone()

    return [
        pytest.param(lambda db: _data_version(SimpleNamespace(db_path=db)), id="data_version"),
        pytest.param(borrowed, id="borrowed_sqlite_connection"),
        pytest.param(probe_graph_database, id="probe_graph_database"),
    ]


@pytest.mark.parametrize("python_side", _python_side_reads())
def test_a_python_side_connection_leaves_the_native_store_working(
    unembedded_repo: Path, python_side
) -> None:
    from dagayn.graph import GraphStore

    db = unembedded_repo / ".dagayn" / "graph.db"
    store = GraphStore(db)
    try:
        python_side(db)
        store.set_metadata("touched", "1")
        store.commit()
        fresh = GraphStore(db)
        try:
            assert fresh.get_metadata("touched") == "1"
            fresh.set_metadata("touched", "2")
            fresh.commit()
        finally:
            fresh.close()
    finally:
        store.close()
