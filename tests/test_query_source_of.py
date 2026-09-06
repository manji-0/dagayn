"""Tests for query_graph pattern=source_of and locator-preserving search."""

from __future__ import annotations

import hashlib
from pathlib import Path

from dagayn.graph import GraphStore
from dagayn.parser import NodeInfo
from dagayn.tools.node_source import SOURCE_OF_MAX_CHARS
from dagayn.tools.query import query_graph, semantic_search_nodes
from dagayn.tools.query_graph_dispatch import _PATTERN_HANDLERS
from dagayn.tools.query_graph_support import QUERY_PATTERNS


def _repo(tmp_path: Path) -> Path:
    root = tmp_path.resolve()
    root.mkdir(parents=True, exist_ok=True)
    (root / ".git").mkdir(exist_ok=True)
    (root / ".dagayn").mkdir(exist_ok=True)
    return root


def _digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _seed_function(
    store: GraphStore,
    *,
    rel_path: str,
    name: str,
    line_start: int,
    line_end: int,
    file_hash: str,
    kind: str = "Function",
    language: str = "python",
) -> None:
    store.upsert_node(
        NodeInfo(
            kind="File",
            name=rel_path,
            file_path=rel_path,
            line_start=1,
            line_end=line_end,
            language=language,
        ),
        file_hash=file_hash,
    )
    store.upsert_node(
        NodeInfo(
            kind=kind,
            name=name,
            file_path=rel_path,
            line_start=line_start,
            line_end=line_end,
            language=language,
        ),
        file_hash=file_hash,
    )
    store.commit()


class TestQueryGraphSourceOf:
    def test_handlers_cover_every_pattern(self) -> None:
        assert set(QUERY_PATTERNS) == set(_PATTERN_HANDLERS)

    def test_returns_live_function_span(self, tmp_path: Path) -> None:
        root = _repo(tmp_path)
        app = root / "app.py"
        app.write_text("def greet():\n    return 'hi'\n\ndef other():\n    return 1\n")
        store = GraphStore(root / ".dagayn" / "graph.db")
        _seed_function(
            store,
            rel_path="app.py",
            name="greet",
            line_start=1,
            line_end=2,
            file_hash=_digest(app),
        )
        store.close()

        result = query_graph(
            pattern="source_of",
            target="app.py::greet",
            repo_root=str(root),
        )

        assert result["status"] == "ok"
        assert result["result_count"] == 1
        item = result["results"][0]
        assert item["source"] == "def greet():\n    return 'hi'"
        assert "1:" not in item["source"]
        assert item["truncated"] is False
        assert item["source_stale"] is False
        assert item["read_error"] is None
        assert item["line_start"] == 1
        assert item["line_end"] == 2
        assert result["source_coverage"]["truncated"] is False
        assert "callers_of" in result["next_action"]["suggestion"]

    def test_minimal_keeps_source_and_locators(self, tmp_path: Path) -> None:
        root = _repo(tmp_path)
        app = root / "app.py"
        app.write_text("def greet():\n    return 'hi'\n")
        store = GraphStore(root / ".dagayn" / "graph.db")
        _seed_function(
            store,
            rel_path="app.py",
            name="greet",
            line_start=1,
            line_end=2,
            file_hash=_digest(app),
        )
        store.close()

        result = query_graph(
            pattern="source_of",
            target="app.py::greet",
            repo_root=str(root),
            detail_level="minimal",
        )

        item = result["results"][0]
        assert item["qualified_name"] == "app.py::greet"
        assert item["line_start"] == 1
        assert item["line_end"] == 2
        assert item["source"] == "def greet():\n    return 'hi'"
        assert item["truncated"] is False

    def test_doc_section_walks_to_next_heading(self, tmp_path: Path) -> None:
        root = _repo(tmp_path)
        doc = root / "notes.md"
        doc.write_text("# Intro\n\nhello\n\n# Next\n\nbye\n")
        store = GraphStore(root / ".dagayn" / "graph.db")
        digest = _digest(doc)
        store.upsert_node(
            NodeInfo(
                kind="DocSection",
                name="intro",
                file_path="notes.md",
                line_start=1,
                line_end=1,
                language="markdown",
            ),
            file_hash=digest,
        )
        store.commit()
        store.close()

        result = query_graph(
            pattern="source_of",
            target="notes.md::intro",
            repo_root=str(root),
        )

        assert result["results"][0]["source"] == "# Intro\n\nhello\n"

    def test_truncates_over_max_chars(self, tmp_path: Path) -> None:
        root = _repo(tmp_path)
        app = root / "app.py"
        body_lines = ["def huge():"] + ["    x = 1"] * 600
        app.write_text("\n".join(body_lines) + "\n")
        store = GraphStore(root / ".dagayn" / "graph.db")
        _seed_function(
            store,
            rel_path="app.py",
            name="huge",
            line_start=1,
            line_end=len(body_lines),
            file_hash=_digest(app),
        )
        store.close()

        result = query_graph(
            pattern="source_of",
            target="app.py::huge",
            repo_root=str(root),
        )

        item = result["results"][0]
        assert result["status"] == "ok"
        assert item["truncated"] is True
        assert len(item["source"]) == SOURCE_OF_MAX_CHARS
        assert item["omitted_chars"] > 0
        assert item["omitted_lines"] > 0
        assert any(row["reason_code"] == "live_source_truncated" for row in result["missingness"])

    def test_marks_source_stale_on_hash_mismatch(self, tmp_path: Path) -> None:
        root = _repo(tmp_path)
        app = root / "app.py"
        app.write_text("def greet():\n    return 'hi'\n")
        store = GraphStore(root / ".dagayn" / "graph.db")
        _seed_function(
            store,
            rel_path="app.py",
            name="greet",
            line_start=1,
            line_end=2,
            file_hash="deadbeef",
        )
        store.close()

        result = query_graph(
            pattern="source_of",
            target="app.py::greet",
            repo_root=str(root),
        )

        assert result["status"] == "degraded"
        assert result["results"][0]["source_stale"] is True
        assert result["results"][0]["source"] == "def greet():\n    return 'hi'"
        assert any(row["reason_code"] == "source_stale" for row in result["missingness"])

    def test_rejects_path_that_escapes_repo(self, tmp_path: Path) -> None:
        root = _repo(tmp_path / "repo")
        outside = tmp_path / "outside.py"
        outside.write_text("secret = 1\n")
        store = GraphStore(root / ".dagayn" / "graph.db")
        store.upsert_node(
            NodeInfo(
                kind="Function",
                name="secret",
                file_path=str(outside),
                line_start=1,
                line_end=1,
                language="python",
            )
        )
        store.commit()
        store.close()

        result = query_graph(
            pattern="source_of",
            target=f"{outside}::secret",
            repo_root=str(root),
        )

        assert result["status"] == "degraded"
        item = result["results"][0]
        assert item["source"] == ""
        assert item["read_error"] == "path_escapes_repo"
        assert "secret = 1" not in item["source"]

    def test_missing_file_is_degraded(self, tmp_path: Path) -> None:
        root = _repo(tmp_path)
        store = GraphStore(root / ".dagayn" / "graph.db")
        store.upsert_node(
            NodeInfo(
                kind="Function",
                name="ghost",
                file_path="missing.py",
                line_start=1,
                line_end=2,
                language="python",
            )
        )
        store.commit()
        store.close()

        result = query_graph(
            pattern="source_of",
            target="missing.py::ghost",
            repo_root=str(root),
        )

        assert result["status"] == "degraded"
        assert result["results"][0]["read_error"] == "not_a_file"

    def test_unknown_pattern_still_errors(self, tmp_path: Path) -> None:
        root = _repo(tmp_path)
        GraphStore(root / ".dagayn" / "graph.db").close()
        result = query_graph(pattern="not_a_pattern", target="x", repo_root=str(root))
        assert result["status"] == "error"
        assert "source_of" in result["error"]


class TestSemanticSearchMinimalLocators:
    def test_minimal_keeps_qualified_name_and_span(self, monkeypatch, tmp_path: Path) -> None:
        from dagayn.tools import query as query_module

        root = _repo(tmp_path)
        store = GraphStore(root / ".dagayn" / "graph.db")
        store.close()

        class _Store:
            def close(self) -> None:
                pass

        monkeypatch.setattr(query_module, "_get_store", lambda repo_root: (_Store(), root))
        monkeypatch.setattr(
            query_module,
            "graph_answerability_summary",
            lambda _store: {"status": "ok", "score": 1.0, "reason_codes": []},
        )
        monkeypatch.setattr(
            query_module,
            "missingness_from_answerability",
            lambda _answerability: [],
        )
        monkeypatch.setattr(
            query_module,
            "hybrid_search",
            lambda *_args, **_kwargs: {
                "results": [
                    {
                        "name": "greet",
                        "qualified_name": "app.py::greet",
                        "kind": "Function",
                        "file_path": "app.py",
                        "line_start": 10,
                        "line_end": 18,
                        "signature": "def greet() -> str",
                        "score": 0.9,
                    }
                ],
                "mode": "fts_only",
                "embedding_health": {"status": "ok"},
                "truncated": False,
                "total": 1,
            },
        )

        result = semantic_search_nodes(
            "greet",
            repo_root=str(root),
            detail_level="minimal",
        )

        item = result["results"][0]
        assert item["qualified_name"] == "app.py::greet"
        assert item["line_start"] == 10
        assert item["line_end"] == 18
        assert "signature" not in item
        assert "source_of" in result["next_action"]["suggestion"]
        assert "source_of" in result["guidance"][0]["action"]
