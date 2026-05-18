"""Tests for the fuzzy documentation search benchmark."""

from __future__ import annotations

from pathlib import Path

from dagayn.eval.benchmarks.doc_fuzzy_search import run
from dagayn.graph import GraphStore
from dagayn.parser import NodeInfo
from dagayn.search import rebuild_fts_index


def _doc_section(name: str, display_name: str, file_path: str, line: int) -> NodeInfo:
    return NodeInfo(
        kind="DocSection",
        name=name,
        file_path=file_path,
        line_start=line,
        line_end=line,
        language="markdown",
        extra={
            "display_name": display_name,
            "heading_level": 2,
            "markdown_kind": "section",
        },
    )


def _build_doc_fixture(tmp_path: Path) -> tuple[Path, GraphStore]:
    repo_path = tmp_path / "repo"
    docs = repo_path / "docs"
    docs.mkdir(parents=True)
    (docs / "reliability.md").write_text(
        "# Operations Handbook\n"
        "\n"
        "## Retry Budget\n"
        "Use exponential backoff with jitter and a retry budget. "
        "This keeps upstream services away from thundering herds "
        "after transient errors.\n"
        "\n"
        "## Secret Handling\n"
        "Store credentials in a secret manager and rotate tokens regularly.\n",
        encoding="utf-8",
    )
    (docs / "schema.md").write_text(
        "# Database Guide\n"
        "\n"
        "## Schema Compatibility\n"
        "Migrations record schema versions so older databases can be upgraded safely.\n",
        encoding="utf-8",
    )
    skills = repo_path / "skills"
    skills.mkdir()
    (skills / "noise.md").write_text(
        "# Noise\n"
        "\n"
        "## Retry Budget\n"
        "This section mentions graceful recovery and hammering dependencies, "
        "but it belongs outside the documentation corpus.\n",
        encoding="utf-8",
    )
    (repo_path / ".dagayn").mkdir()

    store = GraphStore(repo_path / ".dagayn" / "graph.db")
    store.set_metadata("repo_root", str(repo_path))
    for node in [
        _doc_section("retry-budget", "Retry Budget", "docs/reliability.md", 3),
        _doc_section("secret-handling", "Secret Handling", "docs/reliability.md", 7),
        _doc_section("schema-compatibility", "Schema Compatibility", "docs/schema.md", 3),
        _doc_section("retry-budget", "Retry Budget", "skills/noise.md", 3),
    ]:
        store.upsert_node(node)
    store.commit()
    rebuild_fts_index(store)
    return repo_path, store


def test_doc_fuzzy_search_returns_fts_and_embedding_rows(tmp_path):
    repo_path, store = _build_doc_fixture(tmp_path)
    try:
        rows = run(
            repo_path,
            store,
            {
                "name": "docs_fixture",
                "doc_fuzzy_search_queries": [
                    {
                        "query": (
                            "users need graceful recovery after brief outages "
                            "without hammering dependencies"
                        ),
                        "expected": "docs/reliability.md::retry-budget",
                        "label": "paraphrase",
                    }
                ],
            },
        )
    finally:
        store.close()

    query_rows = {row["mode"]: row for row in rows if row["query"] != "__aggregate__"}
    assert set(query_rows) == {"fts", "embedding"}
    assert query_rows["embedding"]["rank"] == 1
    assert query_rows["embedding"]["hit_at_5"] == 1
    assert query_rows["embedding"]["reciprocal_rank"] >= query_rows["fts"]["reciprocal_rank"]
    assert query_rows["embedding"]["provider"] == "eval-doc-fuzzy-hash"
    assert query_rows["embedding"]["query_variant"] == "raw"
    assert query_rows["embedding"]["ndcg_at_5"] > 0.0


def test_doc_fuzzy_search_adds_mode_aggregates(tmp_path):
    repo_path, store = _build_doc_fixture(tmp_path)
    try:
        rows = run(
            repo_path,
            store,
            {
                "name": "docs_fixture",
                "doc_fuzzy_search_queries": [
                    {
                        "query": "where should credentials be kept",
                        "expected": "docs/reliability.md::secret-handling",
                        "label": "secrets",
                    }
                ],
            },
        )
    finally:
        store.close()

    aggregates = {row["mode"]: row for row in rows if row["query"] == "__aggregate__"}
    assert set(aggregates) == {"fts", "embedding"}
    assert aggregates["embedding"]["query_count"] == 1
    assert 0.0 <= aggregates["embedding"]["mean_mrr"] <= 1.0
    assert 0.0 <= aggregates["embedding"]["mean_ndcg_at_5"] <= 1.0


def test_doc_fuzzy_search_supports_relevance_and_query_variants(tmp_path):
    repo_path, store = _build_doc_fixture(tmp_path)
    try:
        rows = run(
            repo_path,
            store,
            {
                "name": "docs_fixture",
                "doc_fuzzy_search_query_variants": [
                    {
                        "name": "doc_query",
                        "prefix": "documentation retrieval query: ",
                    }
                ],
                "doc_fuzzy_search_queries": [
                    {
                        "query": "how can old databases continue working after upgrades",
                        "expected": "docs/schema.md::schema-compatibility",
                        "relevant": [
                            {
                                "target": "docs/reliability.md::retry-budget",
                                "grade": 1,
                            }
                        ],
                        "label": "graded",
                    }
                ],
            },
        )
    finally:
        store.close()

    query_rows = {row["mode"]: row for row in rows if row["query"] != "__aggregate__"}
    assert set(query_rows) == {"fts", "embedding", "embedding_doc_query"}
    assert query_rows["embedding"]["relevant_count"] == 2
    assert "docs/schema.md::schema-compatibility:3" in query_rows["embedding"]["relevant"]
    assert query_rows["embedding_doc_query"]["query_variant"] == "doc_query"
    assert query_rows["embedding_doc_query"]["effective_query"].startswith(
        "documentation retrieval query: "
    )


def test_doc_fuzzy_search_filters_document_corpus(tmp_path):
    repo_path, store = _build_doc_fixture(tmp_path)
    try:
        rows = run(
            repo_path,
            store,
            {
                "name": "docs_fixture",
                "doc_fuzzy_search_include_paths": ["docs/"],
                "doc_fuzzy_search_queries": [
                    {
                        "query": (
                            "users need graceful recovery after brief outages "
                            "without hammering dependencies"
                        ),
                        "expected": "docs/reliability.md::retry-budget",
                        "label": "filtered",
                    }
                ],
            },
        )
    finally:
        store.close()

    query_rows = [row for row in rows if row["query"] != "__aggregate__"]
    assert query_rows
    assert all(not str(row["top_result"]).startswith("skills/") for row in query_rows)


def test_doc_fuzzy_search_empty_config_returns_empty(tmp_path):
    repo_path, store = _build_doc_fixture(tmp_path)
    try:
        assert run(repo_path, store, {"name": "docs_fixture"}) == []
    finally:
        store.close()
