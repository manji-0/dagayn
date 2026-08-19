"""Tests for the embeddings module."""

import json
import os
import sqlite3
import time
from email.message import Message
from unittest.mock import MagicMock, patch

import pytest

from dagayn.cli.commands.build import _print_embedding_status
from dagayn.embeddings import (
    EmbeddingProvider,
    EmbeddingStore,
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
    resolve_active_embedding_provider,
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
            "active_provider": "local:test",
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
            [
                ("Function", "app.py::main"),
                ("Class", "app.py::Widget"),
                ("Function", "app.py::helper"),
            ],
        )
        conn.executemany(
            "INSERT INTO embeddings (qualified_name, vector, text_hash, provider) "
            "VALUES (?, ?, ?, ?)",
            [
                ("app.py::main", _encode_vector([1.0, 0.0]), "hash", "local:test"),
                ("app.py::helper", _encode_vector([0.5, 0.5]), "hash2", "local:test"),
                ("old.py::gone", _encode_vector([0.0, 1.0]), "hash3", "openai:test"),
            ],
        )
        conn.commit()
        conn.close()

        status = get_embedding_status(db_path)

        assert status["status"] == "partial"
        assert status["total_embeddings"] == 3
        assert status["provider_counts"] == {"local:test": 2, "openai:test": 1}
        assert status["active_provider"] == "local:test"
        assert status["embeddable_nodes"] == 3
        assert status["indexed_embeddings"] == 2
        assert status["missing_embeddings"] == 1
        assert status["orphan_embeddings"] == 0

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

    def test_reports_stale_when_active_provider_has_orphans(self, tmp_path):
        db_path = tmp_path / "graph.db"
        conn = self._make_db(db_path)
        conn.executemany(
            "INSERT INTO nodes (kind, qualified_name) VALUES (?, ?)",
            [("Function", "app.py::main")],
        )
        conn.executemany(
            "INSERT INTO embeddings (qualified_name, vector, text_hash, provider) "
            "VALUES (?, ?, ?, ?)",
            [
                ("app.py::main", _encode_vector([1.0, 0.0]), "hash", "local:test"),
                ("old.py::gone", _encode_vector([0.0, 1.0]), "hash", "local:test"),
            ],
        )
        conn.commit()
        conn.close()

        status = get_embedding_status(db_path)

        assert status["status"] == "stale"
        assert status["active_provider"] == "local:test"
        assert status["orphan_embeddings"] == 1

    def test_prefers_metadata_active_provider_for_coverage(self, tmp_path):
        db_path = tmp_path / "graph.db"
        conn = self._make_db(db_path)
        conn.execute("CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
        conn.executemany(
            "INSERT INTO nodes (kind, qualified_name) VALUES (?, ?)",
            [("Function", "app.py::main"), ("Function", "app.py::helper")],
        )
        conn.executemany(
            "INSERT INTO embeddings (qualified_name, vector, text_hash, provider) "
            "VALUES (?, ?, ?, ?)",
            [
                ("app.py::main", _encode_vector([1.0, 0.0]), "hash", "openai:test"),
                ("app.py::helper", _encode_vector([0.0, 1.0]), "hash2", "openai:test"),
                ("app.py::other", _encode_vector([1.0, 0.0]), "hash3", "local:test"),
            ],
        )
        conn.execute(
            "INSERT INTO metadata (key, value) VALUES (?, ?)",
            ("embedding_provider", "openai:test"),
        )
        conn.commit()
        conn.close()

        status = get_embedding_status(db_path)

        assert status["active_provider"] == "openai:test"
        assert status["status"] == "complete"
        assert status["missing_embeddings"] == 0

    def test_resolve_active_provider_picks_highest_row_count(self):
        counts = {
            "local:small#text=material": 3,
            "openai:big#text=material": 9718,
            "legacy:abandoned": 569,
        }

        resolved = resolve_active_embedding_provider(
            counts,
            text_mode="material",
        )

        assert resolved == "openai:big#text=material"

    def test_resolve_active_provider_prefers_metadata_over_row_count(self):
        counts = {"openai:old": 1000, "openai:new": 30}

        resolved = resolve_active_embedding_provider(
            counts,
            preferred_provider="openai:new",
        )

        assert resolved == "openai:new"

    def test_resolve_active_provider_keeps_preferred_with_zero_rows(self):
        counts = {"openai:old": 1000}

        resolved = resolve_active_embedding_provider(
            counts,
            preferred_provider="openai:new",
        )

        assert resolved == "openai:new"

    def test_resolve_active_provider_matches_dim_suffix_preferred(self):
        counts = {"openai:new#dim=1024": 30, "openai:old": 1000}

        resolved = resolve_active_embedding_provider(
            counts,
            preferred_provider="openai:new",
        )

        assert resolved == "openai:new#dim=1024"


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

    def test_material_mode_tolerates_stale_line_past_eof(self, tmp_path):
        """Stale nodes past EOF must not IndexError during comment walk.

        Regression: idx = start - 1 was not clamped to len(lines), so
        ``dagayn update --local-embedding`` aborted mid-embedding and never
        committed the update that would have healed the stale ranges.
        """
        source = tmp_path / "service.py"
        source.write_text(
            "# Still useful.\ndef tiny():\n    return 1\n",
            encoding="utf-8",
        )
        node = self._make_node(
            name="tiny",
            qualified_name="service.py::tiny",
            file_path="service.py",
            line_start=99,
            line_end=120,
        )

        text = _node_to_text(node, source_root=tmp_path, text_mode="material")

        assert "service.py::tiny" in text
        # No source span / adjacent comments when the range is entirely past EOF;
        # the important guarantee is that materialization completes.
        assert isinstance(text, str) and text

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

    def test_remove_orphans_can_sweep_all_providers(self, tmp_path):
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

            assert store.remove_orphans({"file.py::live"}, all_providers=True) == 2

            rows = store._conn.execute(
                "SELECT qualified_name, provider FROM embeddings ORDER BY provider"
            ).fetchall()
            assert [(row["qualified_name"], row["provider"]) for row in rows] == [
                ("file.py::live", "fake"),
            ]
            store.close()

    def test_remove_inactive_provider_partitions_keeps_active_identity(self, tmp_path):
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
                    ("file.py::same", _encode_vector([1.5]), "h1b", "fake#dim=1"),
                    ("file.py::retired", _encode_vector([2.0]), "h2", "other"),
                ],
            )
            store._conn.commit()

            assert store.remove_inactive_provider_partitions() == 1

            rows = store._conn.execute(
                "SELECT qualified_name, provider FROM embeddings ORDER BY qualified_name"
            ).fetchall()
            assert [(row["qualified_name"], row["provider"]) for row in rows] == [
                ("file.py::live", "fake"),
                ("file.py::same", "fake#dim=1"),
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

    @staticmethod
    def _slow_provider(seconds):
        class SlowProvider:
            name = "fake"
            preferred_batch_size = 1

            def __init__(self):
                self.calls = 0

            def embed(self, texts):
                self.calls += 1
                time.sleep(seconds)
                return [[float(i)] for i, _ in enumerate(texts)]

            def embed_query(self, text):
                return [1.0]

            @property
            def dimension(self):
                return 1

        return SlowProvider()

    def test_embed_nodes_without_slice_budget_embeds_everything(self, tmp_path):
        provider = self._slow_provider(0.0)
        nodes = [self._make_node(f"func_{i}", i + 1) for i in range(4)]
        with patch("dagayn.embeddings.get_provider", return_value=provider):
            store = EmbeddingStore(tmp_path / "embeddings.db")
            assert store.embed_nodes(nodes) == 4
            assert store.last_remaining == 0
            store.close()

    def test_slice_budget_stops_early_and_reports_remaining(self, tmp_path):
        provider = self._slow_provider(0.05)
        nodes = [self._make_node(f"func_{i}", i + 1) for i in range(5)]
        with patch("dagayn.embeddings.get_provider", return_value=provider):
            store = EmbeddingStore(tmp_path / "embeddings.db")
            embedded = store.embed_nodes(nodes, slice_seconds=0.04)
            # One batch always runs; the budget is already spent after it.
            assert embedded == 1
            assert store.last_remaining == 4
            assert store.count() == 1
            store.close()

    def test_slice_budget_of_zero_still_makes_progress(self, tmp_path):
        provider = self._slow_provider(0.0)
        nodes = [self._make_node(f"func_{i}", i + 1) for i in range(3)]
        with patch("dagayn.embeddings.get_provider", return_value=provider):
            store = EmbeddingStore(tmp_path / "embeddings.db")
            assert store.embed_nodes(nodes, slice_seconds=0.0) == 1
            assert store.last_remaining == 2
            store.close()

    def test_successive_slices_finish_the_corpus(self, tmp_path):
        provider = self._slow_provider(0.0)
        nodes = [self._make_node(f"func_{i}", i + 1) for i in range(4)]
        with patch("dagayn.embeddings.get_provider", return_value=provider):
            store = EmbeddingStore(tmp_path / "embeddings.db")
            slices = 0
            while True:
                store.embed_nodes(nodes, slice_seconds=0.0)
                slices += 1
                if store.last_remaining == 0:
                    break
                assert slices < 10, "slicing did not converge"
            assert slices == 4
            assert store.count() == 4
            # A finished run must not look like it has leftovers.
            assert store.embed_nodes(nodes, slice_seconds=0.0) == 0
            assert store.last_remaining == 0
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

    def test_search_uses_native_backend_when_configured(self, tmp_path, monkeypatch):
        import dagayn.embeddings as emb

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

        calls = []

        def fake_native_search(db_path, provider_name, query_vec, limit):
            calls.append((db_path, provider_name, query_vec, limit))
            return [("file.py::best", 1.0)]

        monkeypatch.delenv("DAGAYN_EMBEDDING_SEARCH_BACKEND", raising=False)
        monkeypatch.setattr(emb, "_native_embedding_search", fake_native_search)

        with patch("dagayn.embeddings.get_provider", return_value=FakeProvider()):
            store = EmbeddingStore(db)
            assert store.search("query", limit=1) == [("file.py::best", 1.0)]
            store.close()

        assert len(calls) == 1
        assert calls[0][1:] == ("fake#text=material", [1.0, 0.0], 1)

    def test_prewarm_search_uses_native_cache(self, tmp_path, monkeypatch):
        import dagayn.embeddings as emb

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

        calls = []

        def fake_prewarm(db_path, provider_name):
            calls.append((db_path, provider_name))
            return 2

        monkeypatch.setattr(emb, "_native_embedding_search_prewarm", fake_prewarm)

        with patch("dagayn.embeddings.get_provider", return_value=FakeProvider()):
            store = EmbeddingStore(db)
            assert store.prewarm_search() == 2
            store.close()

        assert len(calls) == 1
        assert calls[0][1] == "fake#text=material"

    def test_search_auto_falls_back_when_native_unavailable(self, tmp_path, monkeypatch):
        import dagayn.embeddings as emb

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

        def failing_native_search(*args, **kwargs):
            raise AttributeError("old extension")

        monkeypatch.setenv("DAGAYN_EMBEDDING_SEARCH_BACKEND", "auto")
        monkeypatch.setattr(emb, "_native_embedding_search", failing_native_search)

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

            assert store.search("query", limit=1)[0][0] == "file.py::best"
            store.close()

    def test_search_python_backend_uses_pure_python_without_numpy(self, tmp_path, monkeypatch):
        import dagayn.embeddings as emb
        import dagayn.embeddings_store as emb_store

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

        monkeypatch.setenv("DAGAYN_EMBEDDING_SEARCH_BACKEND", "python")
        monkeypatch.setattr(emb_store, "_NUMPY_AVAILABLE", False)
        monkeypatch.setattr(emb, "_NUMPY_AVAILABLE", False)

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
            results = store.search("query", limit=2)
            store.close()

        assert results[0][0] == "file.py::best"
        assert results[0][1] == pytest.approx(1.0, abs=1e-5)
        assert results[1][0] == "file.py::other"
        assert results[1][1] == pytest.approx(0.0, abs=1e-5)

    def test_search_numpy_matmul_parity_with_python_loop(self, tmp_path, monkeypatch):
        import dagayn.embeddings as emb
        import dagayn.embeddings_store as emb_store

        if not emb._NUMPY_AVAILABLE:
            pytest.skip("numpy fast path is optional")

        db = tmp_path / "embeddings.db"
        vectors = [
            ("n::a", [1.0, 0.0, 0.0]),
            ("n::b", [0.8, 0.6, 0.0]),
            ("n::c", [0.0, 1.0, 0.0]),
            ("n::d", [-1.0, 0.0, 0.0]),
            ("n::e", [0.1, 0.2, 0.3]),
        ]
        query = [0.9, 0.1, 0.0]

        class FakeProvider:
            name = "fake"
            preferred_batch_size = 1

            def embed(self, texts):
                return [[1.0, 0.0, 0.0] for _ in texts]

            def embed_query(self, text):
                return list(query)

            @property
            def dimension(self):
                return 3

        monkeypatch.setenv("DAGAYN_EMBEDDING_SEARCH_BACKEND", "python")

        with patch("dagayn.embeddings.get_provider", return_value=FakeProvider()):
            store = EmbeddingStore(db)
            store._conn.executemany(
                """INSERT INTO embeddings (qualified_name, vector, text_hash, provider)
                   VALUES (?, ?, ?, ?)""",
                [(qn, _encode_vector(vec), f"h{i}", "fake") for i, (qn, vec) in enumerate(vectors)],
            )
            store._conn.commit()

            emb._np_vec_cache.clear()
            numpy_results = store.search("query", limit=5)
            # Also exercise argpartition (limit < n); limit == n takes the argsort path.
            numpy_top2 = store.search("query", limit=2)

            monkeypatch.setattr(emb_store, "_NUMPY_AVAILABLE", False)
            monkeypatch.setattr(emb, "_NUMPY_AVAILABLE", False)
            python_results = store.search("query", limit=5)
            python_top2 = store.search("query", limit=2)
            store.close()

        assert [qn for qn, _ in numpy_results] == [qn for qn, _ in python_results]
        for (_, numpy_score), (_, python_score) in zip(numpy_results, python_results):
            assert numpy_score == pytest.approx(python_score, abs=1e-5)
        assert [qn for qn, _ in numpy_top2] == [qn for qn, _ in python_top2]
        for (_, numpy_score), (_, python_score) in zip(numpy_top2, python_top2):
            assert numpy_score == pytest.approx(python_score, abs=1e-5)

    def test_numpy_argpartition_topk_zero_based_kth(self):
        """argpartition kth is 0-based; kth=k can drop a true top-k member."""
        np = pytest.importorskip("numpy")

        # Distinct scores: true top-3 are indices 1, 3, 5. Index 7 (0.50) is the
        # (k+1)-th neighbor that an off-by-one kth must not let displace 0.90.
        sims = np.array([0.10, 0.99, 0.20, 0.95, 0.30, 0.90, 0.40, 0.50], dtype=np.float32)
        k = 3
        expected = set(np.argsort(-sims)[:k].tolist())
        assert expected == {1, 3, 5}

        def select(kth: int) -> set[int]:
            top_idx = np.argpartition(-sims, kth)[:k]
            top_idx = top_idx[np.argsort(-sims[top_idx])]
            return {int(i) for i in top_idx}

        # Production form: 0-based kth=k-1 with [:k] keeps exactly the true top-k.
        assert select(k - 1) == expected

        # Old kth=k bounds on the (k+1)-th largest. The pivot-inclusive window
        # [:k+1] admits that worse neighbor; truncating back to k without a
        # score re-rank can keep the pivot and drop a true top-k member.
        part = np.argpartition(-sims, k)
        pivot = int(part[k])
        assert float(sims[pivot]) == pytest.approx(float(sorted(sims, reverse=True)[k]))
        assert pivot not in expected
        old_window = {int(i) for i in part[: k + 1]}
        assert expected < old_window
        left = [int(i) for i in part[:k]]
        buggy = set(left[: k - 1] + [pivot])
        assert expected - buggy, "old kth=k truncation must omit a true top-k index"
        assert buggy != expected
        assert float(min(sims[i] for i in select(k - 1))) > float(sims[pivot])

    def test_numpy_matmul_topk_keeps_boundary_member(self, tmp_path):
        """Numpy path with limit < n must not drop the true k-th match."""
        import sqlite3

        import dagayn.embeddings as emb
        import dagayn.embeddings_store as emb_store

        if not emb._NUMPY_AVAILABLE:
            pytest.skip("numpy fast path is optional")

        # Query [1,0]; cosine equals the first component for these rows.
        # Ranked: best, second, third, fourth(near-miss). limit=3 must keep third.
        rows = [
            ("n::best", [1.00, 0.0]),
            ("n::second", [0.90, 0.0]),
            ("n::third", [0.80, 0.0]),
            ("n::fourth", [0.70, 0.0]),
            ("n::noise", [0.10, 1.0]),
        ]
        query = [1.0, 0.0]
        db = tmp_path / "topk.db"
        provider = "fake"
        conn = sqlite3.connect(str(db))
        conn.row_factory = sqlite3.Row
        conn.executescript(
            """
            CREATE TABLE embeddings (
                qualified_name TEXT NOT NULL,
                vector BLOB NOT NULL,
                text_hash TEXT NOT NULL,
                provider TEXT NOT NULL,
                PRIMARY KEY (qualified_name, provider)
            );
            """
        )
        conn.executemany(
            "INSERT INTO embeddings (qualified_name, vector, text_hash, provider) "
            "VALUES (?, ?, ?, ?)",
            [(qn, _encode_vector(vec), f"h{i}", provider) for i, (qn, vec) in enumerate(rows)],
        )
        conn.commit()

        emb._np_vec_cache.clear()
        numpy_hits = emb_store._numpy_matmul_search(db, conn, provider, query, limit=3)
        python_hits = emb_store._python_loop_search(conn, provider, query, limit=3)
        conn.close()

        assert [qn for qn, _ in numpy_hits] == ["n::best", "n::second", "n::third"]
        assert [qn for qn, _ in numpy_hits] == [qn for qn, _ in python_hits]
        assert "n::fourth" not in {qn for qn, _ in numpy_hits}

    def test_search_reuses_numpy_matrix_cache(self, tmp_path, monkeypatch):
        import dagayn.embeddings as emb

        if not emb._NUMPY_AVAILABLE:
            pytest.skip("numpy fast path is optional")

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

        monkeypatch.setenv("DAGAYN_EMBEDDING_SEARCH_BACKEND", "python")

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

    def test_numpy_matmul_microbenchmark_beats_python_loop(self, tmp_path):
        """Document that the numpy matmul path is faster than the pure-Python loop."""
        import sqlite3
        import time

        import dagayn.embeddings as emb
        import dagayn.embeddings_store as emb_store

        if not emb._NUMPY_AVAILABLE:
            pytest.skip("numpy fast path is optional")

        rows = 4000
        dim = 96
        db = tmp_path / "bench.db"
        provider = "bench"
        query = [1.0] + [0.0] * (dim - 1)

        conn = sqlite3.connect(str(db))
        conn.row_factory = sqlite3.Row
        conn.executescript(
            """
            CREATE TABLE embeddings (
                qualified_name TEXT NOT NULL,
                vector BLOB NOT NULL,
                text_hash TEXT NOT NULL,
                provider TEXT NOT NULL,
                PRIMARY KEY (qualified_name, provider)
            );
            """
        )
        records = []
        for i in range(rows):
            vec = [0.0] * dim
            vec[i % dim] = 1.0
            records.append((f"node::{i}", _encode_vector(vec), "h", provider))
        conn.executemany(
            "INSERT INTO embeddings (qualified_name, vector, text_hash, provider) "
            "VALUES (?, ?, ?, ?)",
            records,
        )
        conn.commit()

        emb._np_vec_cache.clear()
        # Warm caches / decode once for a fair comparison of the scan itself.
        _ = emb_store._python_loop_search(conn, provider, query, limit=10)
        _ = emb_store._numpy_matmul_search(db, conn, provider, query, limit=10)

        iterations = 8
        start = time.perf_counter()
        for _ in range(iterations):
            emb_store._python_loop_search(conn, provider, query, limit=10)
        python_ms = (time.perf_counter() - start) * 1000 / iterations

        start = time.perf_counter()
        for _ in range(iterations):
            emb_store._numpy_matmul_search(db, conn, provider, query, limit=10)
        numpy_ms = (time.perf_counter() - start) * 1000 / iterations
        conn.close()

        # Soft assertion: matmul should win on this synthetic size; if the
        # environment is too noisy, still record timings in the assertion message.
        assert numpy_ms < python_ms, (
            f"expected numpy matmul ({numpy_ms:.3f}ms) faster than "
            f"pure-Python ({python_ms:.3f}ms) for {rows}x{dim}"
        )


class TestGetProviderModel:
    """Tests for model parameter in get_provider()."""

    def test_default_returns_none_without_openai_compatible_env(self):
        with patch.dict(os.environ, {}, clear=True):
            assert get_provider(provider=None, model="custom/model") is None

    def test_default_uses_openai_compatible_env_when_configured(self):
        env = {
            "CRG_OPENAI_API_KEY": "dagayn-local",
            "CRG_OPENAI_BASE_URL": "http://127.0.0.1:18080/v1",
            "CRG_OPENAI_MODEL": "bge-m3-gguf-q8_0",
        }
        with patch.dict(os.environ, env, clear=True):
            provider = get_provider(provider=None)

        assert isinstance(provider, OpenAIEmbeddingProvider)
        assert provider.name == "openai:bge-m3-gguf-q8_0@http://127.0.0.1:18080/v1"

    def test_removed_local_provider_returns_none(self, caplog):
        with patch.dict(os.environ, {}, clear=True):
            assert get_provider(provider="local", model="BAAI/bge-m3") is None
        assert "sentence-transformers embeddings were removed" in caplog.text


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

    def test_no_provider_never_warns(self, capsys):
        """No configured provider must not trigger the cloud warning."""
        with patch.dict(os.environ, {}, clear=True):
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
            EmbeddingStore(db, provider="openai", model="custom/model").close()
            mock_gp.assert_called_once_with("openai", model="custom/model")

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
        name = "openai:qwen@http://127.0.0.1:18080/v1#dim=1536"
        provider = provider_from_persisted_name(name)
        assert provider is not None
        assert provider.name == name

    def test_recreates_legacy_localhost_openai_provider_without_dim_suffix(self):
        legacy = "openai:qwen@http://127.0.0.1:18080/v1"
        provider = provider_from_persisted_name(legacy)
        assert provider is not None
        assert provider.name == legacy

    def test_legacy_provider_rows_remain_searchable_after_dim_suffix_upgrade(self, tmp_path):
        legacy = "openai:qwen@http://127.0.0.1:18080/v1"
        provider = provider_from_persisted_name(legacy)
        assert provider is not None

        db_path = tmp_path / "graph.db"
        store = EmbeddingStore(db_path, provider_instance=provider)
        try:
            vector = _encode_vector([1.0, 0.0])
            store._conn.execute(
                "INSERT INTO embeddings (qualified_name, provider, vector, text_hash) "
                "VALUES (?, ?, ?, ?)",
                ("app.py::entry", legacy, vector, "hash"),
            )
            store._conn.commit()

            assert store._provider_key_for_lookup() == legacy
            assert store.count_provider() == 1
            assert store.count_provider(dimension=2) == 1
        finally:
            store.close()

    def test_embed_nodes_persists_probed_dimension_in_provider_key(self, tmp_path):
        provider = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://127.0.0.1:3000/v1",
            model="bge-m3-gguf-q8_0",
        )
        node = GraphNode(
            id=1,
            kind="Function",
            name="entry",
            qualified_name="app.py::entry",
            file_path="app.py",
            line_start=1,
            line_end=1,
            language="python",
            parent_name=None,
            params=None,
            return_type=None,
            is_test=False,
            file_hash=None,
            extra={},
            signature=None,
        )
        with patch.object(
            provider,
            "embed",
            return_value=[[0.1] * 1024],
        ):
            store = EmbeddingStore(tmp_path / "graph.db", provider_instance=provider)
            try:
                assert store.embed_nodes([node]) == 1
                row = store._conn.execute(
                    "SELECT provider, length(vector) FROM embeddings WHERE qualified_name = ?",
                    (node.qualified_name,),
                ).fetchone()
                assert "#dim=1024" in row["provider"]
                assert row["length(vector)"] == 1024 * 4
            finally:
                store.close()

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

    def test_name_includes_dimension_after_probe(self):
        p = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="text-embedding-3-small",
        )
        with patch(
            "urllib.request.urlopen",
            return_value=_make_openai_response([[0.1] * 768]),
        ):
            p.embed_query("hello")
        assert p.name == "openai:text-embedding-3-small@http://localhost:3000/v1#dim=768"

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
        assert p.name == "openai:local-embedding@http://localhost:3000/v1#max_length=2048#dim=1024"

    def test_from_persisted_name_restores_local_max_length_suffix(self):
        p = OpenAIEmbeddingProvider.from_persisted_name(
            "openai:local-embedding@http://127.0.0.1:3000/v1#max_length=2048#dim=1536"
        )

        assert p is not None
        assert p.name == "openai:local-embedding@http://127.0.0.1:3000/v1#max_length=2048#dim=1536"

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
                    hdrs=Message(),
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
                hdrs=Message(),
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
            hdrs=Message(),
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
        assert p is not None
        assert p.name == "openai:text-embedding-3-large@http://127.0.0.1:3000/v1"

    def test_dimension_env_forwarded(self):
        env = {**self._MIN_ENV, "CRG_OPENAI_DIMENSION": "256"}
        with patch.dict("os.environ", env, clear=True):
            p = get_provider("openai")
        assert p is not None
        assert isinstance(p, OpenAIEmbeddingProvider)
        assert p._dimension == 256
        assert p.name == "openai:text-embedding-3-small@http://127.0.0.1:3000/v1#dim=256"

    def test_max_length_env_forwarded(self):
        env = {**self._MIN_ENV, "CRG_OPENAI_MAX_LENGTH": "2048"}
        with patch.dict("os.environ", env, clear=True):
            p = get_provider("openai")
        assert p is not None
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


class TestVectorDimensionIdentity:
    """Regression tests for issue #45: dimension must partition provider identity."""

    def test_dimension_change_forces_reembed(self, tmp_path):
        db = tmp_path / "embeddings.db"

        class DimProvider(EmbeddingProvider):
            def __init__(self, dim: int) -> None:
                self._dim = dim

            @property
            def name(self) -> str:
                return f"fake#dim={self._dim}"

            @property
            def dimension(self) -> int:
                return self._dim

            @property
            def preferred_batch_size(self) -> int:
                return 8

            def embed(self, texts: list[str]) -> list[list[float]]:
                return [[float(i + 1)] * self._dim for i, _ in enumerate(texts)]

            def embed_query(self, text: str) -> list[float]:
                return [1.0] * self._dim

        node = GraphNode(
            id=1,
            kind="Function",
            name="a",
            qualified_name="file.py::a",
            file_path="file.py",
            line_start=1,
            line_end=1,
            language="python",
            parent_name=None,
            params=None,
            return_type=None,
            is_test=False,
            file_hash=None,
            extra={},
            signature=None,
        )
        dim4 = DimProvider(4)
        with patch("dagayn.embeddings.get_provider", return_value=dim4):
            store = EmbeddingStore(db, provider_instance=dim4)
            embedded = store.embed_nodes([node])
            assert embedded == 1
            store.close()

        dim8 = DimProvider(8)
        with patch("dagayn.embeddings.get_provider", return_value=dim8):
            store = EmbeddingStore(db, provider_instance=dim8)
            reembedded = store.embed_nodes([node])
            assert reembedded == 1
            rows = store._conn.execute(
                "SELECT provider, length(vector) FROM embeddings WHERE qualified_name = ?",
                (node.qualified_name,),
            ).fetchall()
            providers = {row["provider"]: row["length(vector)"] for row in rows}
            assert providers["fake#dim=8#text=material"] == 32
            assert providers["fake#dim=4#text=material"] == 16
            store.close()

        conn = sqlite3.connect(str(db))
        assert conn.execute("SELECT COUNT(*) FROM embeddings").fetchone()[0] == 2
        conn.close()

    def test_dimension_mismatch_reported_in_embedding_health(self, tmp_path, monkeypatch):
        from dagayn.graph import GraphStore
        from dagayn.parser import NodeInfo
        from dagayn.search import _embedding_search_with_health

        db = tmp_path / "graph.db"
        store = GraphStore(db)
        store.upsert_node(
            NodeInfo(
                kind="Function",
                name="a",
                file_path="file.py",
                line_start=1,
                line_end=1,
                language="python",
            )
        )
        store._conn.execute(
            """
            CREATE TABLE IF NOT EXISTS embeddings (
                qualified_name TEXT NOT NULL,
                vector BLOB NOT NULL,
                text_hash TEXT NOT NULL,
                provider TEXT NOT NULL,
                PRIMARY KEY (qualified_name, provider)
            )
            """
        )
        store._conn.execute(
            "INSERT INTO embeddings (qualified_name, vector, text_hash, provider) "
            "VALUES (?, ?, ?, ?)",
            (
                "file.py::a",
                _encode_vector([1.0, 0.0, 0.0, 0.0]),
                "hash",
                "fake#dim=8#text=material",
            ),
        )
        store._conn.commit()

        class Dim8Provider:
            name = "fake#dim=8"
            preferred_batch_size = 1

            @property
            def dimension(self) -> int:
                return 8

            def embed(self, texts):
                return [[1.0] * 8 for _ in texts]

            def embed_query(self, text):
                return [1.0] * 8

        monkeypatch.setenv("DAGAYN_EMBEDDING_SEARCH_BACKEND", "python")
        with patch("dagayn.embeddings.get_provider", return_value=Dim8Provider()):
            results, health = _embedding_search_with_health(store, "alpha", limit=5)
            store.close()

        assert results == []
        assert health["status"] == "dimension_mismatch"
        assert health["matching_vector_count"] == 0
        assert health["query_dimension"] == 8
        assert health["stored_dimension"] == 4

    def test_search_backends_agree_on_mixed_dimensions(self, tmp_path, monkeypatch):
        import dagayn.embeddings as emb
        import dagayn.embeddings_store as emb_store

        rows = [
            ("file.py::a", [1.0, 0.0, 0.0, 0.0]),
            ("file.py::b", [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        ]
        query = [1.0, 0.0, 0.0, 0.0]
        db = tmp_path / "mixed.db"
        provider = "fake"
        conn = sqlite3.connect(str(db))
        conn.row_factory = sqlite3.Row
        conn.executescript(
            """
            CREATE TABLE embeddings (
                qualified_name TEXT NOT NULL,
                vector BLOB NOT NULL,
                text_hash TEXT NOT NULL,
                provider TEXT NOT NULL,
                PRIMARY KEY (qualified_name, provider)
            );
            """
        )
        conn.executemany(
            "INSERT INTO embeddings (qualified_name, vector, text_hash, provider) "
            "VALUES (?, ?, ?, ?)",
            [(qn, _encode_vector(vec), f"h{i}", provider) for i, (qn, vec) in enumerate(rows)],
        )
        conn.commit()

        python_hits = emb_store._python_loop_search(conn, provider, query, limit=5)
        if emb._NUMPY_AVAILABLE:
            emb._np_vec_cache.clear()
            numpy_hits = emb_store._numpy_matmul_search(db, conn, provider, query, limit=5)
            assert [qn for qn, _ in numpy_hits] == [qn for qn, _ in python_hits]
        try:
            from dagayn import _core

            rust_hits = [
                (name, score) for name, score in _core.embedding_search(db, provider, query, 5)
            ]
            assert [qn for qn, _ in rust_hits] == [qn for qn, _ in python_hits]
        except (ImportError, AttributeError):
            pytest.skip("native embedding search extension unavailable")  # ty: ignore[too-many-positional-arguments]
        conn.close()

        assert python_hits == [("file.py::a", pytest.approx(1.0))]
        assert "file.py::b" not in {qn for qn, _ in python_hits}

    def test_openai_dimension_change_partitions_provider_name(self):
        p4 = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
            dimension=4,
        )
        p8 = OpenAIEmbeddingProvider(
            api_key="k",
            base_url="http://localhost:3000/v1",
            model="m",
            dimension=8,
        )
        assert p4.name != p8.name
        assert p4.name.endswith("#dim=4")
        assert p8.name.endswith("#dim=8")


class TestProviderKeyReuse:
    """A dim-less lookup identity must still find dim-suffixed rows.

    The dimension is unknown until the provider's first response, so lookups
    used the dim-less key while rows were written with `#dim=N`. Nothing
    matched, so every build re-embedded the whole corpus while `INSERT OR
    REPLACE` kept the row count stable and hid it.
    """

    class _LateDimProvider(EmbeddingProvider):
        """Mirrors OpenAIEmbeddingProvider: dimension learned from the response."""

        preferred_batch_size = 100

        def __init__(self, model: str = "qwen", dim: int = 8) -> None:
            self._model = model
            self._dim = dim
            self._dimension: int | None = None
            self.calls = 0

        @property
        def dimension(self) -> int:
            return self._dimension or self._dim

        @property
        def name(self) -> str:
            suffix = f"#dim={self._dimension}" if self._dimension else ""
            return f"openai:{self._model}@http://127.0.0.1:18080/v1{suffix}"

        def embed(self, texts: list[str]) -> list[list[float]]:
            self.calls += len(texts)
            return [[0.5] * self._dim for _ in texts]

        def embed_query(self, text: str) -> list[float]:
            return [0.5] * self._dim

    def _node(self, name: str, line: int) -> GraphNode:
        return GraphNode(
            id=line,
            kind="Function",
            name=name,
            qualified_name=f"file.py::{name}",
            file_path="file.py",
            line_start=line,
            line_end=line,
            language="python",
            parent_name=None,
            params=None,
            return_type=None,
            is_test=False,
            file_hash=None,
            extra={},
            signature=None,
        )

    def test_second_process_does_not_re_embed(self, tmp_path):
        db = tmp_path / "embeddings.db"
        nodes = [self._node(f"fn{i}", i + 1) for i in range(5)]

        first = self._LateDimProvider()
        store = EmbeddingStore(db, provider_instance=first)
        assert store.embed_nodes(nodes) == 5
        store.close()

        # A fresh process: the provider has not seen a response yet.
        second = self._LateDimProvider()
        store = EmbeddingStore(db, provider_instance=second)
        try:
            assert store.embed_nodes(nodes) == 0
            assert second.calls == 0, "re-embedded an unchanged corpus"
        finally:
            store.close()

    def test_re_spelled_model_reuses_the_existing_partition(self, tmp_path):
        db = tmp_path / "embeddings.db"
        nodes = [self._node("fn0", 1)]

        store = EmbeddingStore(db, provider_instance=self._LateDimProvider(model="qwen"))
        assert store.embed_nodes(nodes) == 1
        store.close()

        restyled = self._LateDimProvider(model="Qwen")
        store = EmbeddingStore(db, provider_instance=restyled)
        try:
            assert store.embed_nodes(nodes) == 0
            assert restyled.calls == 0
            partitions = [
                row[0] for row in store._conn.execute("SELECT DISTINCT provider FROM embeddings")
            ]
            assert len(partitions) == 1, partitions
        finally:
            store.close()

    def test_active_provider_metadata_names_a_partition_with_rows(self, tmp_path):
        from dagayn.embeddings_store import embed_all_nodes
        from dagayn.graph import GraphStore
        from dagayn.parser import NodeInfo

        # One file for both, as production does: the pointer lives in the
        # graph's ``metadata`` table.
        db = tmp_path / "graph.db"
        graph = GraphStore(db)
        graph.upsert_node(
            NodeInfo(
                kind="Function",
                name="fn0",
                file_path="file.py",
                line_start=1,
                line_end=2,
                language="python",
            )
        )
        graph.commit()

        store = EmbeddingStore(db, provider_instance=self._LateDimProvider())
        try:
            assert embed_all_nodes(graph, store) == 1
            pointer = store._conn.execute(
                "SELECT value FROM metadata WHERE key = 'embedding_provider'"
            ).fetchone()
            assert pointer is not None, "no active-provider pointer written"
            rows = store._conn.execute(
                "SELECT COUNT(*) FROM embeddings WHERE provider = ?", (pointer[0],)
            ).fetchone()[0]
            assert rows > 0, f"pointer {pointer[0]!r} names a partition with no rows"
        finally:
            store.close()
            graph.close()


class TestBatchFailureFanOut:
    def test_rate_limit_aborts_without_per_node_retries(self, tmp_path):
        """429 says nothing about the inputs; isolating multiplies the damage."""

        class RateLimited(EmbeddingProvider):
            name = "fake"
            preferred_batch_size = 3

            def __init__(self) -> None:
                self.calls = 0

            def embed(self, texts):
                self.calls += 1
                raise RuntimeError("OpenAI API HTTP 429: rate limit exceeded")

            def embed_query(self, text):
                return [1.0]

            @property
            def dimension(self):
                return 1

        provider = RateLimited()
        nodes = [
            GraphNode(
                id=i,
                kind="Function",
                name=f"fn{i}",
                qualified_name=f"file.py::fn{i}",
                file_path="file.py",
                line_start=i,
                line_end=i,
                language="python",
                parent_name=None,
                params=None,
                return_type=None,
                is_test=False,
                file_hash=None,
                extra={},
                signature=None,
            )
            for i in range(1, 4)
        ]
        store = EmbeddingStore(tmp_path / "e.db", provider_instance=provider)
        try:
            with pytest.raises(RuntimeError, match="429"):
                store.embed_nodes(nodes)
            assert provider.calls == 1, f"fanned out into {provider.calls} calls"
        finally:
            store.close()
