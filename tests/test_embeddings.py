"""Tests for the embeddings module."""

import json
import os
import sqlite3
import time
from unittest.mock import MagicMock, patch

import pytest

from dagayn.cli.commands.build import _print_embedding_status
from dagayn.embeddings import (
    LOCAL_DEFAULT_MODEL,
    EmbeddingStore,
    LocalEmbeddingProvider,
    MiniMaxEmbeddingProvider,
    OpenAIEmbeddingProvider,
    _cosine_similarity,
    _decode_vector,
    _encode_vector,
    _is_localhost_url,
    _node_to_text,
    get_embedding_status,
    get_provider,
    provider_from_persisted_name,
)
from dagayn.graph import GraphNode


class TestVectorEncoding:
    def test_roundtrip(self):
        original = [1.0, 2.5, -3.14, 0.0, 100.0]
        blob = _encode_vector(original)
        decoded = _decode_vector(blob)
        assert len(decoded) == len(original)
        for a, b in zip(original, decoded):
            assert abs(a - b) < 1e-5

    def test_empty_vector(self):
        blob = _encode_vector([])
        decoded = _decode_vector(blob)
        assert decoded == []

    def test_blob_size(self):
        vec = [1.0, 2.0, 3.0]
        blob = _encode_vector(vec)
        assert len(blob) == 12  # 3 floats * 4 bytes each


class TestEmbeddingStatus:
    def _make_db(self, db_path, *, with_embeddings=True):
        conn = sqlite3.connect(db_path)
        conn.execute("CREATE TABLE nodes (kind TEXT NOT NULL, qualified_name TEXT NOT NULL UNIQUE)")
        if with_embeddings:
            conn.execute(
                "CREATE TABLE embeddings ("
                "qualified_name TEXT PRIMARY KEY, "
                "vector BLOB NOT NULL, "
                "text_hash TEXT NOT NULL, "
                "provider TEXT NOT NULL DEFAULT 'unknown')"
            )
        return conn

    def test_not_indexed_when_embedding_table_missing(self, tmp_path):
        db_path = tmp_path / "graph.db"
        conn = self._make_db(db_path, with_embeddings=False)
        conn.close()

        assert get_embedding_status(db_path) == {
            "status": "not_indexed",
            "total_embeddings": 0,
            "provider_counts": {},
        }

    def test_reports_complete_provider_coverage(self, tmp_path):
        db_path = tmp_path / "graph.db"
        conn = self._make_db(db_path)
        conn.executemany(
            "INSERT INTO nodes (kind, qualified_name) VALUES (?, ?)",
            [("File", "app.py"), ("Function", "app.py::main")],
        )
        conn.execute(
            "INSERT INTO embeddings (qualified_name, vector, text_hash, provider) "
            "VALUES (?, ?, ?, ?)",
            ("app.py::main", _encode_vector([1.0, 0.0]), "hash", "local:test"),
        )
        conn.commit()
        conn.close()

        assert get_embedding_status(db_path) == {
            "status": "complete",
            "total_embeddings": 1,
            "provider_counts": {"local:test": 1},
            "embeddable_nodes": 1,
            "indexed_embeddings": 1,
            "missing_embeddings": 0,
            "orphan_embeddings": 0,
        }

    def test_reports_partial_and_stale_coverage(self, tmp_path):
        db_path = tmp_path / "graph.db"
        conn = self._make_db(db_path)
        conn.executemany(
            "INSERT INTO nodes (kind, qualified_name) VALUES (?, ?)",
            [("Function", "app.py::main"), ("Class", "app.py::Widget")],
        )
        conn.executemany(
            "INSERT INTO embeddings (qualified_name, vector, text_hash, provider) "
            "VALUES (?, ?, ?, ?)",
            [
                ("app.py::main", _encode_vector([1.0, 0.0]), "hash", "local:test"),
                ("old.py::gone", _encode_vector([0.0, 1.0]), "hash", "openai:test"),
            ],
        )
        conn.commit()
        conn.close()

        status = get_embedding_status(db_path)

        assert status["status"] == "stale"
        assert status["total_embeddings"] == 2
        assert status["provider_counts"] == {"local:test": 1, "openai:test": 1}
        assert status["embeddable_nodes"] == 2
        assert status["indexed_embeddings"] == 1
        assert status["missing_embeddings"] == 1
        assert status["orphan_embeddings"] == 1

    def test_cli_status_prints_embedding_state(self, tmp_path, capsys):
        db_path = tmp_path / "graph.db"
        conn = self._make_db(db_path)
        conn.executemany(
            "INSERT INTO nodes (kind, qualified_name) VALUES (?, ?)",
            [("Function", "app.py::main"), ("Function", "app.py::helper")],
        )
        conn.execute(
            "INSERT INTO embeddings (qualified_name, vector, text_hash, provider) "
            "VALUES (?, ?, ?, ?)",
            ("app.py::main", _encode_vector([1.0, 0.0]), "hash", "local:test"),
        )
        conn.commit()
        conn.close()

        _print_embedding_status(db_path)

        out = capsys.readouterr().out
        assert "Embeddings: partial (1 vectors, 1 provider(s))" in out
        assert "Coverage: 1/2 embeddable nodes (1 missing)" in out
        assert "Provider: local:test (1)" in out


class TestCosineSimilarity:
    def test_identical_vectors(self):
        v = [1.0, 2.0, 3.0]
        assert abs(_cosine_similarity(v, v) - 1.0) < 1e-6

    def test_orthogonal_vectors(self):
        a = [1.0, 0.0]
        b = [0.0, 1.0]
        assert abs(_cosine_similarity(a, b)) < 1e-6

    def test_opposite_vectors(self):
        a = [1.0, 0.0]
        b = [-1.0, 0.0]
        assert abs(_cosine_similarity(a, b) - (-1.0)) < 1e-6

    def test_zero_vector(self):
        a = [0.0, 0.0]
        b = [1.0, 2.0]
        assert _cosine_similarity(a, b) == 0.0

    def test_dimension_mismatch(self):
        a = [1.0, 2.0, 3.0]
        b = [1.0, 2.0]
        assert _cosine_similarity(a, b) == 0.0


class TestNodeToText:
    def _make_node(self, **kwargs):
        defaults = dict(
            id=1,
            kind="Function",
            name="my_func",
            qualified_name="file.py::my_func",
            file_path="file.py",
            line_start=1,
            line_end=10,
            language="python",
            parent_name=None,
            params=None,
            return_type=None,
            is_test=False,
            file_hash=None,
            extra={},
            signature=None,
        )
        defaults.update(kwargs)
        return GraphNode(**defaults)

    def test_basic_function(self):
        node = self._make_node()
        text = _node_to_text(node, text_mode="metadata")
        assert "my_func" in text
        assert "file.py::my_func" in text
        assert "function" in text
        assert "python" in text

    def test_includes_file_path_terms_and_display_name(self):
        node = self._make_node(
            qualified_name="src/auth_service.py::AuthService.login",
            file_path="src/auth_service.py",
            extra={"display_name": "Auth Service Login"},
        )
        text = _node_to_text(node)
        assert "src auth_service.py" in text
        assert "Auth Service Login" in text

    def test_method_with_parent(self):
        node = self._make_node(parent_name="MyClass")
        text = _node_to_text(node)
        assert "in MyClass" in text

    def test_with_params_and_return_type(self):
        node = self._make_node(params="(x: int, y: str)", return_type="bool")
        text = _node_to_text(node, text_mode="metadata")
        assert "(x: int, y: str)" in text
        assert "returns bool" in text

    def test_includes_signature(self):
        node = self._make_node(signature="def fetch_user(session: Session, user_id: int) -> User")
        text = _node_to_text(node, text_mode="metadata")
        assert "Session" in text
        assert "user_id" in text
        assert "-> User" in text

    def test_material_mode_uses_name_and_adjacent_comment_sentences(self, tmp_path):
        source = tmp_path / "service.py"
        source.write_text(
            "# Retry transient failures.\n"
            "# Keep retries bounded.\n"
            "def handle_failure(retry_budget):\n"
            "    return retry_budget > 0\n",
            encoding="utf-8",
        )
        node = self._make_node(
            name="handle_failure",
            qualified_name="service.py::handle_failure",
            file_path="service.py",
            line_start=3,
            line_end=4,
            params="(retry_budget)",
            signature="def handle_failure(retry_budget)",
        )

        text = _node_to_text(node, source_root=tmp_path, text_mode="material")

        assert "service.py::handle_failure" in text
        assert "Retry transient failures" in text
        assert "Keep retries bounded" in text
        assert "def handle_failure" not in text
        assert "(retry_budget)" not in text

    def test_body_mode_includes_source_excerpt(self, tmp_path):
        source = tmp_path / "service.py"
        source.write_text(
            "def my_func():\n"
            "    retry_budget_exhausted = True\n"
            "    return retry_budget_exhausted\n",
            encoding="utf-8",
        )
        node = self._make_node(file_path="service.py", line_start=1, line_end=3)

        metadata_text = _node_to_text(node, source_root=tmp_path, text_mode="metadata")
        body_text = _node_to_text(node, source_root=tmp_path, text_mode="body")

        assert "retry_budget_exhausted" not in metadata_text
        assert "retry_budget_exhausted" in body_text

    def test_structured_mode_labels_metadata_and_source_excerpt(self, tmp_path):
        source = tmp_path / "service.py"
        source.write_text(
            "class RetryService:\n"
            "    def handle_failure(self, retry_budget: int) -> bool:\n"
            "        retry_budget_exhausted = retry_budget <= 0\n"
            "        return retry_budget_exhausted\n",
            encoding="utf-8",
        )
        node = self._make_node(
            name="handle_failure",
            qualified_name="service.py::RetryService.handle_failure",
            file_path="service.py",
            line_start=2,
            line_end=4,
            parent_name="RetryService",
            params="(self, retry_budget: int)",
            return_type="bool",
            signature="def handle_failure(self, retry_budget: int) -> bool",
        )

        text = _node_to_text(node, source_root=tmp_path, text_mode="structured")

        assert "kind: Function" in text
        assert "qualified: service.py::RetryService.handle_failure" in text
        assert "parent: RetryService" in text
        assert "params: (self, retry_budget: int)" in text
        assert "returns: bool" in text
        assert "source:" in text
        assert "retry_budget_exhausted" in text

    def test_narrative_mode_describes_static_code_facts(self, tmp_path):
        source = tmp_path / "service.py"
        source.write_text(
            "class RetryService:\n"
            "    def handle_failure(self, retry_budget: int) -> bool:\n"
            "        retry_budget_exhausted = retry_budget <= 0\n"
            "        if retry_budget_exhausted:\n"
            "            record_failure(retry_budget)\n"
            "        return retry_budget_exhausted\n",
            encoding="utf-8",
        )
        node = self._make_node(
            name="handle_failure",
            qualified_name="service.py::RetryService.handle_failure",
            file_path="service.py",
            line_start=2,
            line_end=6,
            parent_name="RetryService",
            params="(self, retry_budget: int)",
            return_type="bool",
            signature="def handle_failure(self, retry_budget: int) -> bool",
        )

        text = _node_to_text(node, source_root=tmp_path, text_mode="narrative")

        assert "This python function `handle_failure`" in text
        assert "It belongs to `RetryService`" in text
        assert "It accepts parameters (self, retry_budget: int)" in text
        assert "It calls `record_failure`" in text
        assert "It defines or updates `retry_budget_exhausted`" in text
        assert "It returns values related to retry" in text
        assert "It branches on retry" in text

    def test_narrative_mode_includes_graph_facts(self, tmp_path):
        source = tmp_path / "service.py"
        source.write_text(
            "def handle_failure(retry_budget):\n    return retry_budget <= 0\n",
            encoding="utf-8",
        )
        node = self._make_node(
            name="handle_failure",
            qualified_name="service.py::handle_failure",
            file_path="service.py",
            line_start=1,
            line_end=2,
            signature="def handle_failure(retry_budget)",
        )

        text = _node_to_text(
            node,
            source_root=tmp_path,
            text_mode="narrative",
            graph_facts={
                "CALLS": ["policy.py::record_failure"],
                "TESTED_BY": ["tests/test_service.py::test_handle_failure"],
                "called_by": ["api.py::submit_retry"],
            },
        )

        assert "The graph says it calls `record_failure`" in text
        assert "The graph says it is tested by `test_handle_failure`" in text
        assert "The graph says it is called by `submit_retry`" in text
        assert "Its graph relationships mention policy" in text

    def test_metadata_mode_includes_full_markdown_doc_section(self, tmp_path):
        source = tmp_path / "guide.md"
        source.write_text(
            "# Guide\n"
            "\n"
            "## Retry Budget\n"
            "Use backoff and jitter after transient upstream failures.\n"
            "\n"
            "### Details\n"
            "Keep retries bounded so dependencies are not overwhelmed.\n"
            "\n"
            "## Secret Handling\n"
            "Store credentials in a secret manager.\n",
            encoding="utf-8",
        )
        node = self._make_node(
            kind="DocSection",
            name="retry-budget",
            qualified_name="guide.md::retry-budget",
            file_path="guide.md",
            line_start=3,
            line_end=3,
            language="markdown",
            extra={"display_name": "Retry Budget"},
        )

        metadata_text = _node_to_text(node, source_root=tmp_path, text_mode="metadata")
        body_text = _node_to_text(node, source_root=tmp_path, text_mode="body")

        assert "transient upstream failures" in metadata_text
        assert "transient upstream failures" in body_text
        assert "dependencies are not overwhelmed" in metadata_text
        assert "dependencies are not overwhelmed" in body_text
        assert "Secret Handling" not in metadata_text
        assert "Secret Handling" not in body_text

    def test_metadata_mode_includes_markdown_doc_body_span(self, tmp_path):
        source = tmp_path / "guide.md"
        source.write_text(
            "# Guide\n"
            "\n"
            "## Retry Budget\n"
            "Use backoff and jitter after transient upstream failures.\n"
            "Keep retries bounded so dependencies are not overwhelmed.\n"
            "\n"
            "## Secret Handling\n"
            "Store credentials in a secret manager.\n",
            encoding="utf-8",
        )
        node = self._make_node(
            kind="DocBody",
            name="retry-budget--body-1",
            qualified_name="guide.md::retry-budget--body-1",
            file_path="guide.md",
            line_start=4,
            line_end=5,
            language="markdown",
            extra={
                "display_name": "Use backoff and jitter after transient upstream failures.",
                "parent_section": "guide.md::retry-budget",
            },
        )

        metadata_text = _node_to_text(node, source_root=tmp_path, text_mode="metadata")

        assert "transient upstream failures" in metadata_text
        assert "dependencies are not overwhelmed" in metadata_text
        assert "Secret Handling" not in metadata_text
        assert metadata_text.count("transient upstream failures") >= 2

    def test_file_node_no_kind(self):
        node = self._make_node(kind="File", name="file.py")
        text = _node_to_text(node)
        # File kind should not add "file" as a kind label
        assert "file.py" in text


class TestEmbeddingStore:
    def _make_node(self, name: str, node_id: int) -> GraphNode:
        return GraphNode(
            id=node_id,
            kind="Function",
            name=name,
            qualified_name=f"file.py::{name}",
            file_path="file.py",
            line_start=node_id,
            line_end=node_id,
            language="python",
            parent_name=None,
            params=None,
            return_type=None,
            is_test=False,
            file_hash=None,
            extra={},
            signature=None,
        )

    def test_store_initializes(self, tmp_path):
        db = tmp_path / "embeddings.db"
        with patch("dagayn.embeddings.get_provider", return_value=None):
            store = EmbeddingStore(db)
            assert store.count() == 0
            store.close()

    def test_count_empty(self, tmp_path):
        db = tmp_path / "embeddings.db"
        with patch("dagayn.embeddings.get_provider", return_value=None):
            store = EmbeddingStore(db)
            assert store.count() == 0
            assert store.count_provider() == 0
            store.close()

    def test_embed_nodes_returns_zero_when_unavailable(self, tmp_path):
        db = tmp_path / "embeddings.db"
        with patch("dagayn.embeddings.get_provider", return_value=None):
            store = EmbeddingStore(db)
            result = store.embed_nodes([])
            assert result == 0
            store.close()

    def test_search_returns_empty_when_unavailable(self, tmp_path):
        db = tmp_path / "embeddings.db"
        with patch("dagayn.embeddings.get_provider", return_value=None):
            store = EmbeddingStore(db)
            results = store.search("query")
            assert results == []
            store.close()

    def test_remove_node(self, tmp_path):
        db = tmp_path / "embeddings.db"
        with patch("dagayn.embeddings.get_provider", return_value=None):
            store = EmbeddingStore(db)
            # Should not raise even if node doesn't exist
            store.remove_node("nonexistent::func")
            store.close()

    def test_remove_orphans_deletes_only_current_provider_rows(self, tmp_path):
        db = tmp_path / "embeddings.db"

        class FakeProvider:
            name = "fake"
            preferred_batch_size = 1

            def embed(self, texts):
                return [[1.0] for _ in texts]

            def embed_query(self, text):
                return [1.0]

            @property
            def dimension(self):
                return 1

        with patch("dagayn.embeddings.get_provider", return_value=FakeProvider()):
            store = EmbeddingStore(db)
            store._conn.executemany(
                """INSERT INTO embeddings (qualified_name, vector, text_hash, provider)
                   VALUES (?, ?, ?, ?)""",
                [
                    ("file.py::live", _encode_vector([1.0]), "h1", "fake"),
                    ("file.py::orphan", _encode_vector([2.0]), "h2", "fake"),
                    ("file.py::other_orphan", _encode_vector([3.0]), "h3", "other"),
                ],
            )
            store._conn.commit()

            assert store.remove_orphans({"file.py::live"}) == 1

            rows = store._conn.execute(
                "SELECT qualified_name, provider FROM embeddings ORDER BY provider"
            ).fetchall()
            assert [(row["qualified_name"], row["provider"]) for row in rows] == [
                ("file.py::live", "fake"),
                ("file.py::other_orphan", "other"),
            ]
            store.close()

    def test_embed_all_nodes_removes_orphans_before_embedding(self, tmp_path):
        from dagayn.embeddings import embed_all_nodes

        db = tmp_path / "embeddings.db"
        live = self._make_node("live", 1)

        class FakeGraphStore:
            def get_all_nodes(self, exclude_files=True):
                assert exclude_files is True
                return [live]

        class FakeProvider:
            name = "fake"
            preferred_batch_size = 1

            def embed(self, texts):
                return [[1.0] for _ in texts]

            def embed_query(self, text):
                return [1.0]

            @property
            def dimension(self):
                return 1

        with patch("dagayn.embeddings.get_provider", return_value=FakeProvider()):
            store = EmbeddingStore(db)
            store._conn.execute(
                """INSERT INTO embeddings (qualified_name, vector, text_hash, provider)
                   VALUES (?, ?, ?, ?)""",
                ("file.py::orphan", _encode_vector([2.0]), "old", "fake"),
            )
            store._conn.commit()

            assert embed_all_nodes(FakeGraphStore(), store) == 1
            assert store.last_orphans_removed == 1

            names = [
                row["qualified_name"]
                for row in store._conn.execute("SELECT qualified_name FROM embeddings")
            ]
            assert names == ["file.py::live"]
            store.close()

    def test_embed_nodes_persists_each_successful_batch(self, tmp_path):
        db = tmp_path / "embeddings.db"

        class FakeProvider:
            name = "fake"
            preferred_batch_size = 1

            def __init__(self):
                self.calls = 0

            def embed(self, texts):
                self.calls += 1
                if self.calls == 2:
                    raise TimeoutError("read timed out")
                return [[float(i)] for i, _ in enumerate(texts)]

            def embed_query(self, text):
                return [1.0]

            @property
            def dimension(self):
                return 1

        provider = FakeProvider()
        nodes = [self._make_node(f"func_{i}", i + 1) for i in range(3)]
        with patch("dagayn.embeddings.get_provider", return_value=provider):
            store = EmbeddingStore(db)
            with pytest.raises(RuntimeError, match=r"Embedding batch 2/3 failed"):
                store.embed_nodes(nodes)
            assert store.count() == 1
            store.close()

    def test_embed_nodes_honors_body_text_mode(self, tmp_path):
        db = tmp_path / "embeddings.db"
        source = tmp_path / "file.py"
        source.write_text("def func_1():\n    source_only_token = True\n", encoding="utf-8")

        class CapturingProvider:
            name = "capture"
            preferred_batch_size = 1

            def __init__(self):
                self.texts = []

            def embed(self, texts):
                self.texts.extend(texts)
                return [[1.0] for _ in texts]

            def embed_query(self, text):
                return [1.0]

            @property
            def dimension(self):
                return 1

        provider = CapturingProvider()
        with patch("dagayn.embeddings.get_provider", return_value=provider):
            store = EmbeddingStore(db, text_mode="body", source_root=tmp_path)
            node = self._make_node("func_1", 1)
            node.line_end = 2
            assert store.embed_nodes([node]) == 1
            assert "source_only_token" in provider.texts[0]
            store.close()

    def test_embed_nodes_partitions_rows_by_text_mode(self, tmp_path):
        db = tmp_path / "embeddings.db"

        class CapturingProvider:
            name = "capture"
            preferred_batch_size = 1

            def __init__(self):
                self.texts = []

            def embed(self, texts):
                self.texts.extend(texts)
                return [[float(len(self.texts))] for _ in texts]

            def embed_query(self, text):
                return [1.0]

            @property
            def dimension(self):
                return 1

        provider = CapturingProvider()
        node = self._make_node("func_1", 1)
        with patch("dagayn.embeddings.get_provider", return_value=provider):
            material = EmbeddingStore(db, text_mode="material")
            assert material.embed_nodes([node]) == 1
            assert material.count_provider() == 1
            material.close()

            narrative = EmbeddingStore(db, text_mode="narrative")
            assert narrative.embed_nodes([node]) == 1
            assert narrative.count_provider() == 1

            rows = narrative._conn.execute(
                "SELECT qualified_name, provider FROM embeddings ORDER BY provider"
            ).fetchall()
            assert [(row["qualified_name"], row["provider"]) for row in rows] == [
                ("file.py::func_1", "capture#text=material"),
                ("file.py::func_1", "capture#text=narrative"),
            ]
            assert narrative.count() == 2
            narrative.close()

    def test_embed_nodes_isolates_failed_nodes_after_batch_failure(self, tmp_path):
        db = tmp_path / "embeddings.db"

        class FakeProvider:
            name = "fake"
            preferred_batch_size = 3

            def embed(self, texts):
                if len(texts) > 1:
                    raise TimeoutError("batch timed out")
                if "bad" in texts[0]:
                    raise TimeoutError("single node timed out")
                return [[1.0]]

            def embed_query(self, text):
                return [1.0]

            @property
            def dimension(self):
                return 1

        nodes = [
            self._make_node("good_a", 1),
            self._make_node("bad_node", 2),
            self._make_node("good_b", 3),
        ]
        with patch("dagayn.embeddings.get_provider", return_value=FakeProvider()):
            store = EmbeddingStore(db)
            with pytest.raises(RuntimeError) as exc_info:
                store.embed_nodes(nodes)
            message = str(exc_info.value)
            assert "failed as a batch" in message
            assert "file.py::bad_node" in message
            assert "single node timed out" in message
            assert store.count() == 2
            store.close()

    def test_embed_nodes_recovers_when_batch_fails_but_individual_nodes_succeed(self, tmp_path):
        db = tmp_path / "embeddings.db"

        class FakeProvider:
            name = "fake"
            preferred_batch_size = 2

            def embed(self, texts):
                if len(texts) > 1:
                    raise TimeoutError("batch timed out")
                return [[1.0]]

            def embed_query(self, text):
                return [1.0]

            @property
            def dimension(self):
                return 1

        nodes = [self._make_node("a", 1), self._make_node("b", 2)]
        with patch("dagayn.embeddings.get_provider", return_value=FakeProvider()):
            store = EmbeddingStore(db)
            assert store.embed_nodes(nodes) == 2
            assert store.count() == 2
            store.close()

    def test_embed_nodes_reports_vector_count_mismatch_with_batch_context(self, tmp_path):
        db = tmp_path / "embeddings.db"

        class FakeProvider:
            name = "fake"
            preferred_batch_size = 2

            def embed(self, texts):
                return [[1.0]]

            def embed_query(self, text):
                return [1.0]

            @property
            def dimension(self):
                return 1

        with patch("dagayn.embeddings.get_provider", return_value=FakeProvider()):
            store = EmbeddingStore(db)
            with pytest.raises(RuntimeError, match=r"batch 1/1 returned 1 vector"):
                store.embed_nodes([self._make_node("a", 1), self._make_node("b", 2)])
            assert store.count() == 0
            store.close()

    def test_search_reuses_numpy_matrix_cache(self, tmp_path, monkeypatch):
        import dagayn.embeddings as emb

        assert emb._NUMPY_AVAILABLE

        db = tmp_path / "embeddings.db"

        class FakeProvider:
            name = "fake"
            preferred_batch_size = 1

            def embed(self, texts):
                return [[1.0, 0.0] for _ in texts]

            def embed_query(self, text):
                return [1.0, 0.0]

            @property
            def dimension(self):
                return 2

        with patch("dagayn.embeddings.get_provider", return_value=FakeProvider()):
            store = EmbeddingStore(db)
            store._conn.executemany(
                """INSERT INTO embeddings (qualified_name, vector, text_hash, provider)
                   VALUES (?, ?, ?, ?)""",
                [
                    ("file.py::best", _encode_vector([1.0, 0.0]), "h1", "fake"),
                    ("file.py::other", _encode_vector([0.0, 1.0]), "h2", "fake"),
                ],
            )
            store._conn.commit()

            emb._np_vec_cache.clear()
            load_calls = 0
            original_load = emb._load_vec_matrix

            def counting_load(*args, **kwargs):
                nonlocal load_calls
                load_calls += 1
                return original_load(*args, **kwargs)

            monkeypatch.setattr(emb, "_load_vec_matrix", counting_load)

            assert store.search("query", limit=1)[0][0] == "file.py::best"
            assert store.search("query", limit=1)[0][0] == "file.py::best"
            assert load_calls == 1
            store.close()


class TestLocalEmbeddingProviderModelName:
    """Tests for configurable model name on LocalEmbeddingProvider."""

    def test_default_model_name(self):
        with patch.dict(os.environ, {}, clear=False):
            os.environ.pop("CRG_EMBEDDING_MODEL", None)
            provider = LocalEmbeddingProvider()
            assert provider._model_name == LOCAL_DEFAULT_MODEL
            assert provider.name == f"local:{LOCAL_DEFAULT_MODEL}"

    def test_explicit_model_name(self):
        with patch.dict(os.environ, {"CRG_EMBEDDING_MODEL": "should-be-ignored"}):
            provider = LocalEmbeddingProvider(model_name="custom/model")
            assert provider._model_name == "custom/model"
            assert provider.name == "local:custom/model"

    def test_env_var_fallback(self):
        with patch.dict(os.environ, {"CRG_EMBEDDING_MODEL": "BAAI/bge-small-en-v1.5"}):
            provider = LocalEmbeddingProvider()
            assert provider._model_name == "BAAI/bge-small-en-v1.5"
            assert provider.name == "local:BAAI/bge-small-en-v1.5"


class TestGetProviderModel:
    """Tests for model parameter in get_provider()."""

    @patch("dagayn.embeddings.LocalEmbeddingProvider")
    def test_local_passes_model(self, mock_cls):
        mock_cls.return_value = MagicMock()
        get_provider(provider=None, model="custom/model")
        mock_cls.assert_called_once_with(model_name="custom/model")

    @patch("dagayn.embeddings.LocalEmbeddingProvider")
    def test_local_default_passes_none(self, mock_cls):
        mock_cls.return_value = MagicMock()
        get_provider(provider=None, model=None)
        mock_cls.assert_called_once_with(model_name=None)


class TestCloudProviderWarning:
    """Tests for the stderr warning before cloud provider use (#174)."""

    def test_minimax_triggers_stderr_warning(self, capsys):
        """Using the MiniMax provider should print a warning to stderr
        unless CRG_ACCEPT_CLOUD_EMBEDDINGS=1 is set."""
        with patch.dict(os.environ, {"MINIMAX_API_KEY": "fake"}, clear=False):
            os.environ.pop("CRG_ACCEPT_CLOUD_EMBEDDINGS", None)
            with patch(
                "dagayn.embeddings.MiniMaxEmbeddingProvider",
            ) as mock_cls:
                mock_cls.return_value = MagicMock()
                get_provider(provider="minimax")
        captured = capsys.readouterr()
        assert "minimax" in captured.err.lower()
        assert "cloud" in captured.err.lower()
        assert "sent to an external API" in captured.err
        # Should NOT have written to stdout (would corrupt MCP stdio).
        assert captured.out == ""

    def test_google_triggers_stderr_warning(self, capsys):
        with patch.dict(os.environ, {"GOOGLE_API_KEY": "fake"}, clear=False):
            os.environ.pop("CRG_ACCEPT_CLOUD_EMBEDDINGS", None)
            with patch(
                "dagayn.embeddings.GoogleEmbeddingProvider",
            ) as mock_cls:
                mock_cls.return_value = MagicMock()
                get_provider(provider="google")
        captured = capsys.readouterr()
        assert "google" in captured.err.lower()
        assert captured.out == ""

    def test_accept_env_var_suppresses_warning(self, capsys):
        """Setting CRG_ACCEPT_CLOUD_EMBEDDINGS=1 silences the warning."""
        with patch.dict(
            os.environ,
            {
                "MINIMAX_API_KEY": "fake",
                "CRG_ACCEPT_CLOUD_EMBEDDINGS": "1",
            },
            clear=False,
        ):
            with patch(
                "dagayn.embeddings.MiniMaxEmbeddingProvider",
            ) as mock_cls:
                mock_cls.return_value = MagicMock()
                get_provider(provider="minimax")
        captured = capsys.readouterr()
        assert captured.err == ""
        assert captured.out == ""

    def test_local_provider_never_warns(self, capsys):
        """Local (offline) provider must not trigger the cloud warning."""
        with patch(
            "dagayn.embeddings.LocalEmbeddingProvider",
        ) as mock_cls:
            mock_cls.return_value = MagicMock()
            get_provider(provider=None)
        captured = capsys.readouterr()
        assert "cloud" not in captured.err.lower()


class TestEmbeddingStoreModelPassthrough:
    """Tests that EmbeddingStore passes model to get_provider."""

    def test_model_forwarded_to_get_provider(self, tmp_path):
        db = tmp_path / "embeddings.db"
        with patch("dagayn.embeddings.get_provider", return_value=None) as mock_gp:
            EmbeddingStore(db, model="custom/model").close()
            mock_gp.assert_called_once_with(None, model="custom/model")

    def test_provider_and_model_forwarded(self, tmp_path):
        db = tmp_path / "embeddings.db"
        with patch("dagayn.embeddings.get_provider", return_value=None) as mock_gp:
            EmbeddingStore(db, provider="local", model="custom/model").close()
            mock_gp.assert_called_once_with("local", model="custom/model")

    def test_provider_instance_skips_get_provider(self, tmp_path):
        db = tmp_path / "embeddings.db"
        provider = MagicMock()
        provider.name = "fake"
        with patch("dagayn.embeddings.get_provider", return_value=None) as mock_gp:
            store = EmbeddingStore(db, provider_instance=provider)
            assert store.provider is provider
            mock_gp.assert_not_called()
            store.close()


class TestPersistedProviderResolution:
    def test_recreates_localhost_openai_provider(self):
        name = "openai:qwen@http://127.0.0.1:18080/v1"
        provider = provider_from_persisted_name(name)
        assert provider is not None
        assert provider.name == name

    def test_refuses_non_localhost_openai_provider(self):
        assert provider_from_persisted_name("openai:qwen@https://api.example.com/v1") is None

    def test_refuses_non_openai_provider(self):
        assert provider_from_persisted_name("local:all-MiniLM-L6-v2") is None


class TestMiniMaxEmbeddingProvider:
    """Unit tests for MiniMaxEmbeddingProvider."""

    def test_name(self):
        provider = MiniMaxEmbeddingProvider(api_key="test-key")
        assert provider.name == "minimax:embo-01"

    def test_dimension(self):
        provider = MiniMaxEmbeddingProvider(api_key="test-key")
        assert provider.dimension == 1536

    def test_embed_calls_api_with_db_type(self):
        provider = MiniMaxEmbeddingProvider(api_key="test-key")
        mock_vectors = [[0.1] * 1536, [0.2] * 1536]
        mock_response = json.dumps(
            {
                "vectors": mock_vectors,
                "total_tokens": 10,
                "base_resp": {"status_code": 0, "status_msg": "success"},
            }
        ).encode("utf-8")

        mock_resp_obj = MagicMock()
        mock_resp_obj.read.return_value = mock_response
        mock_resp_obj.__enter__ = MagicMock(return_value=mock_resp_obj)
        mock_resp_obj.__exit__ = MagicMock(return_value=False)

        with patch("urllib.request.urlopen", return_value=mock_resp_obj) as mock_urlopen:
            result = provider.embed(["hello", "world"])

        assert len(result) == 2
        assert len(result[0]) == 1536
        call_args = mock_urlopen.call_args
        req = call_args[0][0]
        payload = json.loads(req.data.decode("utf-8"))
        assert payload["type"] == "db"
        assert payload["model"] == "embo-01"

    def test_embed_query_calls_api_with_query_type(self):
        provider = MiniMaxEmbeddingProvider(api_key="test-key")
        mock_vectors = [[0.5] * 1536]
        mock_response = json.dumps(
            {
                "vectors": mock_vectors,
                "total_tokens": 5,
                "base_resp": {"status_code": 0, "status_msg": "success"},
            }
        ).encode("utf-8")

        mock_resp_obj = MagicMock()
        mock_resp_obj.read.return_value = mock_response
        mock_resp_obj.__enter__ = MagicMock(return_value=mock_resp_obj)
        mock_resp_obj.__exit__ = MagicMock(return_value=False)

        with patch("urllib.request.urlopen", return_value=mock_resp_obj) as mock_urlopen:
            result = provider.embed_query("search term")

        assert len(result) == 1536
        call_args = mock_urlopen.call_args
        req = call_args[0][0]
        payload = json.loads(req.data.decode("utf-8"))
        assert payload["type"] == "query"

    def test_embed_api_error_raises(self):
        provider = MiniMaxEmbeddingProvider(api_key="test-key")
        mock_response = json.dumps(
            {
                "vectors": [],
                "base_resp": {"status_code": 1001, "status_msg": "invalid api key"},
            }
        ).encode("utf-8")

        mock_resp_obj = MagicMock()
        mock_resp_obj.read.return_value = mock_response
        mock_resp_obj.__enter__ = MagicMock(return_value=mock_resp_obj)
        mock_resp_obj.__exit__ = MagicMock(return_value=False)

        with patch("urllib.request.urlopen", return_value=mock_resp_obj):
            with pytest.raises(RuntimeError, match="invalid api key"):
                provider.embed_query("test")


class TestGetProviderMiniMax:
    """Tests for get_provider() with MiniMax."""

    def test_get_provider_minimax_with_key(self):
        with patch.dict("os.environ", {"MINIMAX_API_KEY": "test-key"}):
            provider = get_provider("minimax")
        assert isinstance(provider, MiniMaxEmbeddingProvider)
        assert provider.name == "minimax:embo-01"

    def test_get_provider_minimax_without_key_raises(self):
        with patch.dict("os.environ", {}, clear=True):
            with pytest.raises(ValueError, match="MINIMAX_API_KEY"):
                get_provider("minimax")


class TestEmbeddingStoreContextManager:
    """Regression tests for #260: EmbeddingStore must support the context
    manager protocol so connections are cleaned up on exception."""

    def test_supports_context_manager(self, tmp_path):
        db = tmp_path / "embed_ctx.db"
        with EmbeddingStore(db) as store:
            assert store is not None
            assert store.db_path == db
        # After exiting, connection should be closed.
        # (Attempting another query would fail, but we don't test that
        # because close() doesn't invalidate the object — it just
        # closes the underlying sqlite3 connection.)

    def test_context_manager_closes_on_exception(self, tmp_path):
        db = tmp_path / "embed_err.db"
        try:
            with EmbeddingStore(db) as store:
                assert store.db_path == db
                raise RuntimeError("simulated crash")
        except RuntimeError:
            pass
        # The connection was closed by __exit__ even though an exception
        # was raised.  This is the whole point of #260 — without the
        # context manager, the connection would leak.


def _make_openai_response(vectors: list[list[float]]) -> MagicMock:
    body = json.dumps(
        {
            "data": [{"embedding": v, "index": i} for i, v in enumerate(vectors)],
            "model": "text-embedding-3-small",
            "object": "list",
            "usage": {"prompt_tokens": 5, "total_tokens": 5},
        }
    ).encode("utf-8")
    mock = MagicMock()
    mock.read.return_value = body
    mock.__enter__ = MagicMock(return_value=mock)
    mock.__exit__ = MagicMock(return_value=False)
    return mock


class TestIsLocalhostUrl:
    """Ensure localhost detection is robust against subdomain tricks."""

    def test_plain_localhost(self):
        assert _is_localhost_url("http://localhost:3000/v1")

    def test_127_loopback(self):
        assert _is_localhost_url("http://127.0.0.1:3000/v1")

    def test_0000_loopback(self):
        assert _is_localhost_url("http://0.0.0.0:8080/v1")

    def test_ipv6_loopback(self):
        assert _is_localhost_url("http://[::1]:3000/v1")

    def test_real_cloud_host(self):
        assert not _is_localhost_url("https://api.openai.com/v1")

    def test_subdomain_spoof_not_localhost(self):
        # Architect flagged: plain string match would mis-classify this.
        assert not _is_localhost_url("https://my-openai.127.0.0.1.nip.io/v1")

    def test_invalid_url(self):
        assert not _is_localhost_url("not a url")


class TestOpenAIEmbeddingProvider:
    def test_name_includes_model(self):
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="text-embedding-3-small",
        )
        assert p.name == "openai:text-embedding-3-small@http://localhost:3000/v1"

    def test_default_dimension_before_call(self):
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        assert p.dimension == 1536  # fallback until first response

    def test_dimension_captured_from_response(self):
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        with patch(
            "urllib.request.urlopen",
            return_value=_make_openai_response([[0.1] * 768]),
        ):
            vec = p.embed_query("hello")
        assert len(vec) == 768
        assert p.dimension == 768

    def test_embed_calls_api_with_correct_payload(self):
        p = OpenAIEmbeddingProvider(
            api_key="secret-key",
            base_url="http://127.0.0.1:3000/v1",
            model="text-embedding-3-small",
        )
        with patch(
            "urllib.request.urlopen",
            return_value=_make_openai_response([[0.1] * 1536, [0.2] * 1536]),
        ) as mock_urlopen:
            result = p.embed(["hello", "world"])

        assert len(result) == 2
        assert len(result[0]) == 1536

        req = mock_urlopen.call_args[0][0]
        payload = json.loads(req.data.decode("utf-8"))
        assert payload["model"] == "text-embedding-3-small"
        assert payload["input"] == ["hello", "world"]
        assert "dimensions" not in payload  # not pinned by default
        assert req.headers["Authorization"] == "Bearer secret-key"
        assert req.headers["Content-type"] == "application/json"
        assert req.full_url == "http://127.0.0.1:3000/v1/embeddings"

    def test_explicit_dimension_forwarded_in_payload(self):
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="text-embedding-3-large",
            dimension=256,
        )
        with patch(
            "urllib.request.urlopen",
            return_value=_make_openai_response([[0.1] * 256]),
        ) as mock_urlopen:
            p.embed_query("x")
        payload = json.loads(mock_urlopen.call_args[0][0].data.decode("utf-8"))
        assert payload["dimensions"] == 256

    def test_explicit_max_length_forwarded_and_partitions_provider_identity(self):
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="local-embedding",
            max_length=2048,
        )
        with patch(
            "urllib.request.urlopen",
            return_value=_make_openai_response([[0.1] * 1024]),
        ) as mock_urlopen:
            p.embed_query("x")
        payload = json.loads(mock_urlopen.call_args[0][0].data.decode("utf-8"))
        assert payload["max_length"] == 2048
        assert p.name == "openai:local-embedding@http://localhost:3000/v1#max_length=2048"

    def test_from_persisted_name_restores_local_max_length_suffix(self):
        p = OpenAIEmbeddingProvider.from_persisted_name(
            "openai:local-embedding@http://127.0.0.1:3000/v1#max_length=2048"
        )

        assert p is not None
        assert p.name == "openai:local-embedding@http://127.0.0.1:3000/v1#max_length=2048"

    def test_base_url_trailing_slash_stripped(self):
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1/",
            model="m",
        )
        with patch(
            "urllib.request.urlopen",
            return_value=_make_openai_response([[0.1] * 10]),
        ) as mock_urlopen:
            p.embed_query("x")
        req = mock_urlopen.call_args[0][0]
        assert req.full_url == "http://localhost:3000/v1/embeddings"

    def test_embed_api_error_raises(self):
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        err_body = json.dumps(
            {
                "error": {"message": "invalid api key", "type": "invalid_request_error"},
            }
        ).encode("utf-8")
        mock = MagicMock()
        mock.read.return_value = err_body
        mock.__enter__ = MagicMock(return_value=mock)
        mock.__exit__ = MagicMock(return_value=False)
        with patch("urllib.request.urlopen", return_value=mock):
            with pytest.raises(RuntimeError, match="invalid api key"):
                p.embed_query("x")

    def test_embed_empty_data_raises(self):
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        body = json.dumps({"data": []}).encode("utf-8")
        mock = MagicMock()
        mock.read.return_value = body
        mock.__enter__ = MagicMock(return_value=mock)
        mock.__exit__ = MagicMock(return_value=False)
        with patch("urllib.request.urlopen", return_value=mock):
            with pytest.raises(RuntimeError, match="empty data"):
                p.embed_query("x")

    def test_batching_splits_into_100_per_request(self):
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        texts = [f"text-{i}" for i in range(250)]
        call_count = {"n": 0}

        def _mk_response(*_args, **_kwargs):
            call_count["n"] += 1
            # match payload size
            req = _args[0]
            body = json.loads(req.data.decode("utf-8"))
            n = len(body["input"])
            return _make_openai_response([[0.1] * 5 for _ in range(n)])

        with patch("urllib.request.urlopen", side_effect=_mk_response):
            out = p.embed(texts)
        assert len(out) == 250
        assert call_count["n"] == 3  # 100 + 100 + 50

    def test_custom_batch_size_respected(self):
        """new-api gateways (e.g. text-embedding-v4) cap batch at 10 —
        user must be able to lower the batch size to avoid 400 errors."""
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
            batch_size=10,
        )
        texts = [f"t-{i}" for i in range(25)]
        call_count = {"n": 0}

        def _mk_response(*_args, **_kwargs):
            call_count["n"] += 1
            req = _args[0]
            body = json.loads(req.data.decode("utf-8"))
            assert len(body["input"]) <= 10  # never exceed configured size
            return _make_openai_response([[0.1] * 5 for _ in body["input"]])

        with patch("urllib.request.urlopen", side_effect=_mk_response):
            out = p.embed(texts)
        assert len(out) == 25
        assert call_count["n"] == 3  # 10 + 10 + 5

    def test_empty_input_returns_empty(self):
        """embed([]) must short-circuit without hitting the API."""
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        with patch("urllib.request.urlopen") as mock_urlopen:
            assert p.embed([]) == []
            mock_urlopen.assert_not_called()

    def test_endpoint_isolation_in_name(self):
        """Two providers with the same model but different base URLs MUST
        produce different provider.name values, otherwise the embeddings
        store silently reuses vectors from a different backend's vector space.
        (Codex review HIGH finding.)"""
        p1 = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="https://api.openai.com/v1",
            model="text-embedding-3-small",
        )
        p2 = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="https://openrouter.ai/api/v1",
            model="text-embedding-3-small",
        )
        p3 = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://127.0.0.1:3000/v1",
            model="text-embedding-3-small",
        )
        assert p1.name != p2.name != p3.name
        assert p1.name == "openai:text-embedding-3-small@https://api.openai.com/v1"
        assert p2.name == "openai:text-embedding-3-small@https://openrouter.ai/api/v1"
        assert p3.name == "openai:text-embedding-3-small@http://127.0.0.1:3000/v1"

    def test_trailing_slash_does_not_change_identity(self):
        """A trailing slash on base_url must not cause a re-embed."""
        p1 = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        p2 = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1/",
            model="m",
        )
        assert p1.name == p2.name

    def test_path_routed_gateways_get_distinct_identity(self):
        """Path-routed gateways (same host, different URL path) front
        different backends and must NOT share cached vectors.
        (Codex round-2 HIGH finding.)"""
        p1 = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="https://gw.example.com/openai/v1",
            model="m",
        )
        p2 = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="https://gw.example.com/vendor-b/v1",
            model="m",
        )
        assert p1.name != p2.name
        assert p1.name == "openai:m@https://gw.example.com/openai/v1"
        assert p2.name == "openai:m@https://gw.example.com/vendor-b/v1"

    def test_default_port_is_stripped_from_identity(self):
        """`https://host/v1` and `https://host:443/v1` must map to the
        same identity; stripping is necessary so the user can't force
        a pointless re-embed by spelling the port differently.
        (Codex round-2 MED finding.)"""
        p1 = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="https://api.openai.com/v1",
            model="m",
        )
        p2 = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="https://api.openai.com:443/v1",
            model="m",
        )
        p3 = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://example.com:80/v1",
            model="m",
        )
        p4 = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://example.com/v1",
            model="m",
        )
        assert p1.name == p2.name
        assert p3.name == p4.name
        # Non-default port still affects identity (normal case).
        p5 = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="https://api.openai.com:8443/v1",
            model="m",
        )
        assert p5.name != p1.name

    def test_userinfo_is_stripped_from_identity(self):
        """Credentials embedded in the URL must NOT appear in provider.name
        (which gets persisted into the embeddings table). This is an
        at-rest credential-leak defense. (Codex round-2 MED finding.)"""
        p_plain = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="https://api.example.com/v1",
            model="m",
        )
        p_auth = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="https://user:secret@api.example.com/v1",
            model="m",
        )
        # 1. Same identity — userinfo stripped.
        assert p_plain.name == p_auth.name
        # 2. The secret never appears in the identity string.
        assert "secret" not in p_auth.name
        assert "user" not in p_auth.name

    def test_ipv6_literal_in_identity(self):
        """IPv6 hostnames must round-trip cleanly, with brackets restored
        when a non-default port is attached."""
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://[::1]:3000/v1",
            model="m",
        )
        assert p.name == "openai:m@http://[::1]:3000/v1"

    def test_response_with_missing_index_raises(self):
        """Length-only checks let duplicate/missing indices through. We
        require a strict 0..N-1 permutation. (Codex round-2 MED finding.)"""
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        bad = json.dumps(
            {
                "data": [
                    {"embedding": [1.0], "index": 0},
                    {"embedding": [2.0], "index": 0},  # duplicate 0, missing 1
                ],
            }
        ).encode("utf-8")
        mock = MagicMock()
        mock.read.return_value = bad
        mock.__enter__ = MagicMock(return_value=mock)
        mock.__exit__ = MagicMock(return_value=False)
        with patch("urllib.request.urlopen", return_value=mock):
            with pytest.raises(RuntimeError, match="malformed indices"):
                p.embed(["a", "b"])

    def test_response_with_out_of_range_index_raises(self):
        """Index >= N is invalid even if count matches."""
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        bad = json.dumps(
            {
                "data": [
                    {"embedding": [1.0], "index": 0},
                    {"embedding": [2.0], "index": 5},  # out-of-range
                ],
            }
        ).encode("utf-8")
        mock = MagicMock()
        mock.read.return_value = bad
        mock.__enter__ = MagicMock(return_value=mock)
        mock.__exit__ = MagicMock(return_value=False)
        with patch("urllib.request.urlopen", return_value=mock):
            with pytest.raises(RuntimeError, match="malformed indices"):
                p.embed(["a", "b"])

    def test_response_without_index_field_falls_back_to_server_order(self):
        """Some OpenAI-compatible gateways omit `index` entirely. The
        length check is the only safety net available — we must still
        succeed on length match and fail on mismatch."""
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        no_idx = json.dumps(
            {
                "data": [
                    {"embedding": [1.0]},
                    {"embedding": [2.0]},
                ],
            }
        ).encode("utf-8")
        mock = MagicMock()
        mock.read.return_value = no_idx
        mock.__enter__ = MagicMock(return_value=mock)
        mock.__exit__ = MagicMock(return_value=False)
        with patch("urllib.request.urlopen", return_value=mock):
            result = p.embed(["a", "b"])
        # Trust server order when index is absent.
        assert result == [[1.0], [2.0]]

    def test_scheme_change_produces_distinct_identity(self):
        """http and https to the same host/path front different endpoints
        in practice (dev vs prod gateway, pre/post TLS migration). They
        must NOT share cached vectors. (Codex round-3 HIGH finding.)"""
        p_http = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://gw.example.com/v1",
            model="m",
        )
        p_https = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="https://gw.example.com/v1",
            model="m",
        )
        assert p_http.name != p_https.name
        # http default port 80 and https default port 443 are both stripped
        # from the host, but scheme is preserved in the identity.
        assert p_http.name == "openai:m@http://gw.example.com/v1"
        assert p_https.name == "openai:m@https://gw.example.com/v1"

    def test_mixed_indexed_unindexed_response_raises(self):
        """Some items with ``index``, others without: must refuse rather
        than silently zip in server order (which would misplace the
        indexed items). (Codex round-3 HIGH finding.)"""
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        mixed = json.dumps(
            {
                "data": [
                    {"embedding": [1.0], "index": 1},  # claims to be for input[1]
                    {"embedding": [2.0]},  # no index
                ],
            }
        ).encode("utf-8")
        mock = MagicMock()
        mock.read.return_value = mixed
        mock.__enter__ = MagicMock(return_value=mock)
        mock.__exit__ = MagicMock(return_value=False)
        with patch("urllib.request.urlopen", return_value=mock):
            with pytest.raises(RuntimeError, match="mixed indexed/unindexed"):
                p.embed(["a", "b"])

    def test_string_index_treated_as_mixed(self):
        """Some OpenAI-compatible gateways serialize index as a string.
        Our permutation check requires ints; string index must fall to
        the mixed-case refusal, not silently slip through."""
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        bad = json.dumps(
            {
                "data": [
                    {"embedding": [1.0], "index": "0"},  # string, not int
                    {"embedding": [2.0], "index": "1"},
                ],
            }
        ).encode("utf-8")
        mock = MagicMock()
        mock.read.return_value = bad
        mock.__enter__ = MagicMock(return_value=mock)
        mock.__exit__ = MagicMock(return_value=False)
        with patch("urllib.request.urlopen", return_value=mock):
            with pytest.raises(RuntimeError, match="mixed indexed/unindexed"):
                p.embed(["a", "b"])

    def test_retry_on_remote_disconnected(self, monkeypatch):
        """http.client.RemoteDisconnected is a common transient failure
        when reverse proxies drop idle connections. Must retry.
        (Codex round-2 LOW finding.)"""
        import http.client

        monkeypatch.setattr(time, "sleep", lambda s: None)

        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        call_count = {"n": 0}

        def _mock_urlopen(*args, **kwargs):
            call_count["n"] += 1
            if call_count["n"] == 1:
                raise http.client.RemoteDisconnected("edge proxy dropped connection")
            return _make_openai_response([[0.1] * 5])

        with patch("urllib.request.urlopen", side_effect=_mock_urlopen):
            p.embed_query("x")
        assert call_count["n"] == 2

    def test_response_length_mismatch_raises(self):
        """Gateway returns fewer embeddings than inputs: refuse to proceed
        rather than silently zip misaligned vectors onto the wrong nodes.
        (Codex review MED finding.)"""
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        with patch(
            "urllib.request.urlopen",
            return_value=_make_openai_response([[0.1] * 5]),  # 1 vec
        ):
            with pytest.raises(RuntimeError, match="refusing to misalign"):
                p.embed(["a", "b", "c"])  # 3 inputs

    def test_reordered_response_is_sorted_by_index(self):
        """Gateway returns data out of order: restore input order via
        the `index` field, so vec[i] always corresponds to input[i].
        (Codex review MED finding.)"""
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        # Return data in order 2, 0, 1 (i.e. reversed-ish).
        reordered = json.dumps(
            {
                "data": [
                    {"embedding": [3.0], "index": 2},
                    {"embedding": [1.0], "index": 0},
                    {"embedding": [2.0], "index": 1},
                ],
            }
        ).encode("utf-8")
        mock = MagicMock()
        mock.read.return_value = reordered
        mock.__enter__ = MagicMock(return_value=mock)
        mock.__exit__ = MagicMock(return_value=False)
        with patch("urllib.request.urlopen", return_value=mock):
            result = p.embed(["a", "b", "c"])
        # Must be [[1.0], [2.0], [3.0]] after sorting by index.
        assert result == [[1.0], [2.0], [3.0]]

    def test_retry_on_http_429(self, monkeypatch):
        """HTTP 429 must trigger retry with backoff (not bail immediately).
        (Codex review MED finding — prior substring match missed the fact
        that error bodies may not contain '429'.)"""
        import urllib.error

        monkeypatch.setattr(time, "sleep", lambda s: None)  # instant retries

        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        call_count = {"n": 0}
        good_response = _make_openai_response([[0.1] * 5])
        import io

        def _mock_urlopen(*args, **kwargs):
            call_count["n"] += 1
            if call_count["n"] == 1:
                raise urllib.error.HTTPError(
                    url="http://localhost:3000/v1/embeddings",
                    code=429,
                    msg="Too Many Requests",
                    hdrs=None,
                    fp=io.BytesIO(b'{"error": "rate limited"}'),
                )
            return good_response

        with patch("urllib.request.urlopen", side_effect=_mock_urlopen):
            out = p.embed_query("x")
        assert len(out) == 5
        assert call_count["n"] == 2  # 1 fail + 1 success

    def test_retry_on_socket_timeout(self, monkeypatch):
        """socket.timeout (read timeout) must be classified retryable —
        previously these surfaced as str(exc) without '429/500/503' so
        retry never fired. (Codex review MED finding.)"""
        import socket

        monkeypatch.setattr(time, "sleep", lambda s: None)

        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        call_count = {"n": 0}
        good_response = _make_openai_response([[0.1] * 5])

        def _mock_urlopen(*args, **kwargs):
            call_count["n"] += 1
            if call_count["n"] <= 2:
                raise socket.timeout("read timed out")
            return good_response

        with patch("urllib.request.urlopen", side_effect=_mock_urlopen):
            out = p.embed_query("x")
        assert len(out) == 5
        assert call_count["n"] == 3  # 2 fails + 1 success

    def test_retry_on_url_error(self, monkeypatch):
        """URLError (connection refused, DNS failure) must retry."""
        import urllib.error

        monkeypatch.setattr(time, "sleep", lambda s: None)

        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        call_count = {"n": 0}

        def _mock_urlopen(*args, **kwargs):
            call_count["n"] += 1
            if call_count["n"] == 1:
                raise urllib.error.URLError("connection refused")
            return _make_openai_response([[0.1] * 5])

        with patch("urllib.request.urlopen", side_effect=_mock_urlopen):
            p.embed_query("x")
        assert call_count["n"] == 2

    def test_no_retry_on_http_400(self, monkeypatch):
        """HTTP 400 = caller bug (bad payload, unsupported model). Must fail
        fast rather than waste time on 3 retries."""
        import io
        import urllib.error

        monkeypatch.setattr(time, "sleep", lambda s: None)

        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        call_count = {"n": 0}

        def _mock_urlopen(*args, **kwargs):
            call_count["n"] += 1
            raise urllib.error.HTTPError(
                url="http://localhost:3000/v1/embeddings",
                code=400,
                msg="Bad Request",
                hdrs=None,
                fp=io.BytesIO(b'{"error": {"message": "invalid model"}}'),
            )

        with patch("urllib.request.urlopen", side_effect=_mock_urlopen):
            with pytest.raises(RuntimeError, match="invalid model"):
                p.embed_query("x")
        assert call_count["n"] == 1  # no retry on 4xx non-429

    def test_http_error_body_is_surfaced(self):
        """If the gateway returns 400 with a JSON error body, the RuntimeError
        must include the real reason, not just 'HTTP Error 400: Bad Request'."""
        import urllib.error

        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
        )
        body = json.dumps(
            {
                "error": {"message": "batch size is invalid, should not exceed 10."},
            }
        ).encode("utf-8")
        # HTTPError's .read() returns bytes from its fp
        import io

        err = urllib.error.HTTPError(
            url="http://localhost:3000/v1/embeddings",
            code=400,
            msg="Bad Request",
            hdrs=None,
            fp=io.BytesIO(body),
        )
        with patch("urllib.request.urlopen", side_effect=err):
            with pytest.raises(RuntimeError, match="batch size is invalid"):
                p.embed_query("x")


class TestGetProviderOpenAI:
    _MIN_ENV = {
        "CRG_OPENAI_API_KEY": "sk-test",
        "CRG_OPENAI_BASE_URL": "http://127.0.0.1:3000/v1",
        "CRG_OPENAI_MODEL": "text-embedding-3-small",
    }

    def test_with_all_env_vars(self):
        with patch.dict("os.environ", self._MIN_ENV, clear=True):
            p = get_provider("openai")
        assert isinstance(p, OpenAIEmbeddingProvider)
        assert p.name == "openai:text-embedding-3-small@http://127.0.0.1:3000/v1"

    def test_missing_api_key_raises(self):
        env = {k: v for k, v in self._MIN_ENV.items() if k != "CRG_OPENAI_API_KEY"}
        with patch.dict("os.environ", env, clear=True):
            with pytest.raises(ValueError, match="CRG_OPENAI_API_KEY"):
                get_provider("openai")

    def test_missing_base_url_raises(self):
        env = {k: v for k, v in self._MIN_ENV.items() if k != "CRG_OPENAI_BASE_URL"}
        with patch.dict("os.environ", env, clear=True):
            with pytest.raises(ValueError, match="CRG_OPENAI_BASE_URL"):
                get_provider("openai")

    def test_missing_model_raises(self):
        env = {k: v for k, v in self._MIN_ENV.items() if k != "CRG_OPENAI_MODEL"}
        with patch.dict("os.environ", env, clear=True):
            with pytest.raises(ValueError, match="CRG_OPENAI_MODEL"):
                get_provider("openai")

    def test_model_arg_overrides_env(self):
        with patch.dict("os.environ", self._MIN_ENV, clear=True):
            p = get_provider("openai", model="text-embedding-3-large")
        assert p.name == "openai:text-embedding-3-large@http://127.0.0.1:3000/v1"

    def test_dimension_env_forwarded(self):
        env = {**self._MIN_ENV, "CRG_OPENAI_DIMENSION": "256"}
        with patch.dict("os.environ", env, clear=True):
            p = get_provider("openai")
        assert p._dimension == 256

    def test_max_length_env_forwarded(self):
        env = {**self._MIN_ENV, "CRG_OPENAI_MAX_LENGTH": "2048"}
        with patch.dict("os.environ", env, clear=True):
            p = get_provider("openai")
        assert p.name == "openai:text-embedding-3-small@http://127.0.0.1:3000/v1#max_length=2048"

    def test_localhost_suppresses_egress_warning(self, capsys):
        with patch.dict("os.environ", self._MIN_ENV, clear=True):
            get_provider("openai")
        captured = capsys.readouterr()
        # localhost must never trigger the cloud-egress warning
        assert captured.err == ""
        assert captured.out == ""

    def test_cloud_base_url_triggers_egress_warning(self, capsys):
        env = {**self._MIN_ENV, "CRG_OPENAI_BASE_URL": "https://api.openai.com/v1"}
        with patch.dict("os.environ", env, clear=True):
            # drop accept flag to ensure warning fires
            os.environ.pop("CRG_ACCEPT_CLOUD_EMBEDDINGS", None)
            get_provider("openai")
        captured = capsys.readouterr()
        assert "openai" in captured.err.lower()
        assert "cloud" in captured.err.lower()
        assert captured.out == ""  # MCP stdio safety

    def test_subdomain_spoof_triggers_warning(self, capsys):
        """my-openai.127.0.0.1.nip.io must NOT be treated as localhost."""
        env = {
            **self._MIN_ENV,
            "CRG_OPENAI_BASE_URL": "https://my-openai.127.0.0.1.nip.io/v1",
        }
        with patch.dict("os.environ", env, clear=True):
            get_provider("openai")
        captured = capsys.readouterr()
        assert "cloud" in captured.err.lower()
