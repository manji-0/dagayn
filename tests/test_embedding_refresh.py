"""Gate, scope, and queue behaviour for local embedding refresh."""

from __future__ import annotations

import sqlite3
from pathlib import Path
from unittest.mock import patch

from dagayn.embeddings_store import _encode_vector, get_embedding_status
from dagayn.task_queue import _enqueue_scoped_embed_after_update
from dagayn.tools.sync_status import (
    embedding_needs_refresh,
    embedding_refresh_action,
    graph_uses_local_embedding_sidecar,
)


def _coverage_db(
    path: Path,
    *,
    embeddable: int,
    indexed: int,
    provider: str = "openai:bge-m3-gguf-q8_0@http://127.0.0.1:18080/v1#dim=1024#text=material",
) -> Path:
    conn = sqlite3.connect(path)
    conn.execute("CREATE TABLE nodes (kind TEXT NOT NULL, qualified_name TEXT NOT NULL UNIQUE)")
    conn.execute(
        "CREATE TABLE embeddings ("
        "qualified_name TEXT NOT NULL, "
        "vector BLOB NOT NULL, "
        "text_hash TEXT NOT NULL, "
        "provider TEXT NOT NULL DEFAULT 'unknown', "
        "PRIMARY KEY (qualified_name, provider))"
    )
    conn.execute("CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES (?, ?)",
        ("embedding_provider", provider),
    )
    for i in range(embeddable):
        conn.execute(
            "INSERT INTO nodes (kind, qualified_name) VALUES (?, ?)",
            ("Function", f"app.py::fn_{i}"),
        )
    blob = _encode_vector([1.0, 0.0])
    for i in range(indexed):
        conn.execute(
            "INSERT INTO embeddings (qualified_name, vector, text_hash, provider) "
            "VALUES (?, ?, ?, ?)",
            (f"app.py::fn_{i}", blob, "hash", provider),
        )
    conn.commit()
    conn.close()
    return path


class TestEmbeddingRefreshAction:
    def test_none_mode_skips(self, tmp_path: Path) -> None:
        db = _coverage_db(tmp_path / "graph.db", embeddable=0, indexed=0)
        assert embedding_refresh_action(db, local_embedding="none") == "skip"
        assert embedding_needs_refresh(db, local_embedding="none") is False

    def test_empty_index_is_inline(self, tmp_path: Path) -> None:
        db = tmp_path / "graph.db"
        conn = sqlite3.connect(db)
        conn.execute("CREATE TABLE nodes (kind TEXT NOT NULL, qualified_name TEXT NOT NULL)")
        conn.execute("INSERT INTO nodes VALUES ('Function', 'app.py::main')")
        conn.execute(
            "CREATE TABLE embeddings ("
            "qualified_name TEXT NOT NULL, vector BLOB NOT NULL, "
            "text_hash TEXT NOT NULL, provider TEXT NOT NULL, "
            "PRIMARY KEY (qualified_name, provider))"
        )
        conn.commit()
        conn.close()
        assert embedding_refresh_action(db, local_embedding="bge-m3") == "inline"
        assert embedding_needs_refresh(db, local_embedding="bge-m3") is True

    def test_complete_coverage_skips(self, tmp_path: Path) -> None:
        db = _coverage_db(tmp_path / "graph.db", embeddable=10, indexed=10)
        assert embedding_refresh_action(db, local_embedding="bge-m3") == "skip"
        assert embedding_needs_refresh(db, local_embedding="bge-m3") is False

    def test_small_hole_is_queued(self, tmp_path: Path) -> None:
        db = _coverage_db(tmp_path / "graph.db", embeddable=100, indexed=99)
        assert get_embedding_status(db)["missing_embeddings"] == 1
        assert embedding_refresh_action(db, local_embedding="bge-m3") == "queue"
        assert embedding_needs_refresh(db, local_embedding="bge-m3") is False

    def test_large_hole_is_inline(self, tmp_path: Path) -> None:
        db = _coverage_db(tmp_path / "graph.db", embeddable=20, indexed=10)
        assert embedding_refresh_action(db, local_embedding="bge-m3") == "inline"
        assert embedding_needs_refresh(db, local_embedding="bge-m3") is True

    def test_sidecar_detection(self, tmp_path: Path) -> None:
        from dagayn.tools.sync_status import sidecar_embed_payload

        local = _coverage_db(tmp_path / "local.db", embeddable=1, indexed=1)
        remote = _coverage_db(
            tmp_path / "remote.db",
            embeddable=1,
            indexed=1,
            provider="google:gemini-embedding-001#text=material",
        )
        qwen = _coverage_db(
            tmp_path / "qwen.db",
            embeddable=1,
            indexed=1,
            provider=(
                "openai:qwen3-embedding-0.6b-gguf-q8_0"
                "@http://127.0.0.1:18081/v1#dim=1024#text=material"
            ),
        )
        assert graph_uses_local_embedding_sidecar(local) is True
        assert graph_uses_local_embedding_sidecar(remote) is False
        assert sidecar_embed_payload(local)["local_embedding"] == "bge-m3"
        assert sidecar_embed_payload(qwen)["local_embedding"] == "low"


class TestEnqueueAfterUpdate:
    def test_queues_changed_and_dependent_files(self, tmp_path: Path) -> None:
        (tmp_path / ".dagayn").mkdir()
        _coverage_db(tmp_path / ".dagayn" / "graph.db", embeddable=2, indexed=2)
        captured: list[dict] = []

        def _fake_enqueue(repo_root, **kwargs):
            captured.append({"repo": repo_root, **kwargs})
            return "added", 7

        with patch("dagayn.task_queue.enqueue_embed_refresh", side_effect=_fake_enqueue):
            note = _enqueue_scoped_embed_after_update(
                tmp_path,
                {
                    "changed_files": ["a.py"],
                    "dependent_files": ["b.py"],
                },
            )

        assert note is not None and "2 file(s)" in note
        assert captured[0]["files"] == ["a.py", "b.py"]
        assert captured[0]["spawn_worker"] is False
        assert captured[0]["payload"]["local_embedding"] == "bge-m3"
        assert captured[0]["payload"]["local_embedding_port"] == 18080

    def test_infers_qwen_sidecar_from_stored_provider(self, tmp_path: Path) -> None:
        (tmp_path / ".dagayn").mkdir()
        _coverage_db(
            tmp_path / ".dagayn" / "graph.db",
            embeddable=1,
            indexed=1,
            provider=(
                "openai:qwen3-embedding-0.6b-gguf-q8_0"
                "@http://127.0.0.1:18081/v1#dim=1024#text=material"
            ),
        )
        captured: list[dict] = []

        def _fake_enqueue(repo_root, **kwargs):
            captured.append(kwargs)
            return "added", 3

        with patch("dagayn.task_queue.enqueue_embed_refresh", side_effect=_fake_enqueue):
            note = _enqueue_scoped_embed_after_update(tmp_path, {"changed_files": ["a.py"]})

        assert note is not None
        assert captured[0]["payload"]["local_embedding"] == "low"
        assert captured[0]["payload"]["local_embedding_mode"] == "llama-qwen3"
        assert captured[0]["payload"]["local_embedding_port"] == 18081

    def test_skips_when_no_local_sidecar(self, tmp_path: Path) -> None:
        (tmp_path / ".dagayn").mkdir()
        _coverage_db(
            tmp_path / ".dagayn" / "graph.db",
            embeddable=1,
            indexed=1,
            provider="google:gemini-embedding-001#text=material",
        )
        (tmp_path / ".dagayn").mkdir(exist_ok=True)
        with patch("dagayn.task_queue.enqueue_embed_refresh") as enqueue:
            note = _enqueue_scoped_embed_after_update(tmp_path, {"changed_files": ["a.py"]})
        assert note is None
        enqueue.assert_not_called()


class TestSessionPrepareQueue:
    def test_small_partial_queues_instead_of_inline(self, tmp_path: Path, monkeypatch) -> None:
        from unittest.mock import patch

        from dagayn.tools.session_prepare import session_prepare

        queued: list[dict] = []

        def _fake_enqueue(repo_root, **kwargs):
            queued.append(kwargs)
            return "added", 1

        monkeypatch.setattr("dagayn.task_queue.enqueue_embed_refresh", _fake_enqueue)
        monkeypatch.setattr("dagayn.task_queue.ensure_worker", lambda *_a, **_k: False)
        monkeypatch.setattr(
            "dagayn.tools.sync_status.embedding_refresh_action",
            lambda *_a, **_k: "queue",
        )

        class _Stats:
            total_nodes = 10
            total_edges = 4
            files_count = 2
            last_updated = "now"

        class _Store:
            def get_stats(self):
                return _Stats()

            def close(self):
                return None

        with (
            patch(
                "dagayn.tools.session_prepare.assess_graph_sync",
                lambda *_a, **_k: {
                    "state": "commit_synced",
                    "status": "synced",
                    "vcs": "git",
                    "git_head_sha": "abc",
                    "current_head_sha": "abc",
                },
            ),
            patch("dagayn.tools.session_prepare.detect_vcs", lambda *_a, **_k: "git"),
            patch(
                "dagayn.tools.session_prepare._get_store",
                lambda *_a, **_k: (_Store(), tmp_path),
            ),
            patch(
                "dagayn.tools.session_prepare._answerability_via_sqlite",
                lambda *_a, **_k: {"status": "ok", "score": 1.0},
            ),
            patch(
                "dagayn.tools.session_prepare.build_or_update_graph",
                lambda **_k: (_ for _ in ()).throw(AssertionError("inline embed must not run")),
            ),
        ):
            result = session_prepare(
                repo_root=str(tmp_path),
                local_embedding="bge-m3",
                embedding_policy="auto",
                budget_seconds=300,
                seed_worktree=False,
            )
        assert result["phases"]["embedding"] == "pending"
        assert result["reason"] == "embedding_queued"
        assert queued and queued[0]["files"] is None
