"""Tests for dagayn/exports.py: GraphML, Neo4j Cypher, Obsidian vault, Mermaid C4, SVG."""

from __future__ import annotations

import pytest

from dagayn.graph import GraphStore
from dagayn.parser import EdgeInfo, NodeInfo


@pytest.fixture
def populated_store(tmp_path):
    db_path = tmp_path / "test.db"
    store = GraphStore(db_path)

    def _node(kind, name, file_path, is_test=False, parent_name=None):
        return NodeInfo(
            kind=kind,
            name=name,
            file_path=file_path,
            line_start=1,
            line_end=10,
            language="python",
            parent_name=parent_name,
            params=None,
            return_type=None,
            modifiers=None,
            is_test=is_test,
            extra={},
        )

    def _edge(kind, source, target, file_path="src/app.py"):
        return EdgeInfo(
            kind=kind, source=source, target=target, file_path=file_path, line=1, extra={}
        )

    nodes = [
        _node("File", "app.py", "src/app.py"),
        _node("Class", "AppService", "src/app.py"),
        _node("Function", "run", "src/app.py", parent_name="AppService"),
        _node("File", "test_app.py", "tests/test_app.py"),
        _node("Test", "test_run", "tests/test_app.py", is_test=True),
    ]
    for n in nodes:
        store.upsert_node(n)

    edges = [
        _edge("CONTAINS", "src/app.py", "src/app.py::AppService"),
        _edge("CONTAINS", "src/app.py", "src/app.py::AppService.run"),
        _edge(
            "CALLS",
            "tests/test_app.py::test_run",
            "src/app.py::AppService.run",
            "tests/test_app.py",
        ),
    ]
    for e in edges:
        store.upsert_edge(e)

    store.commit()
    return store


@pytest.fixture
def knowledge_gap_hotspot_store(tmp_path):
    """Graph with a high-degree production node that lacks TESTED_BY coverage."""
    db_path = tmp_path / "hotspot.db"
    store = GraphStore(db_path)

    def _node(kind, name, file_path, *, is_test=False, language="python"):
        return NodeInfo(
            kind=kind,
            name=name,
            file_path=file_path,
            line_start=1,
            line_end=10,
            language=language,
            parent_name=None,
            params=None,
            return_type=None,
            modifiers=None,
            is_test=is_test,
            extra={},
        )

    def _edge(source, target, file_path="src/service.py"):
        return EdgeInfo(
            kind="CALLS",
            source=source,
            target=target,
            file_path=file_path,
            line=1,
            extra={},
        )

    store.upsert_node(_node("Function", "service", "src/service.py"))
    for idx in range(8):
        caller_path = f"src/caller_{idx}.py"
        store.upsert_node(_node("Function", f"caller_{idx}", caller_path))
        store.upsert_edge(
            _edge(f"{caller_path}::caller_{idx}", "src/service.py::service", caller_path)
        )
    store.commit()
    return store


@pytest.fixture
def mermaid_store(tmp_path):
    db_path = tmp_path / "mermaid.db"
    store = GraphStore(db_path)

    def _node(
        kind,
        name,
        file_path,
        *,
        language="python",
        is_test=False,
        parent_name=None,
    ):
        return NodeInfo(
            kind=kind,
            name=name,
            file_path=file_path,
            line_start=1,
            line_end=10,
            language=language,
            parent_name=parent_name,
            params=None,
            return_type=None,
            modifiers=None,
            is_test=is_test,
            extra={},
        )

    def _edge(kind, source, target, file_path):
        return EdgeInfo(
            kind=kind, source=source, target=target, file_path=file_path, line=1, extra={}
        )

    nodes = [
        _node("File", "api.py", "src/api.py"),
        _node("Class", "ApiService", "src/api.py"),
        _node("Function", "fetch", "src/api.py", parent_name="ApiService"),
        _node("Function", "sync", "src/api.py", parent_name="ApiService"),
        _node("File", "db.py", "src/db.py"),
        _node("Function", "query", "src/db.py"),
        _node("File", "api.py", "pkg/api.py"),
        _node("Function", "mirror", "pkg/api.py"),
        _node("File", "test_api.py", "tests/test_api.py"),
        _node("Test", "test_fetch", "tests/test_api.py", is_test=True),
        _node("File", "README.md", "README.md", language="markdown"),
    ]
    for node in nodes:
        store.upsert_node(node)

    edges = [
        _edge("CONTAINS", "src/api.py", "src/api.py::ApiService", "src/api.py"),
        _edge("CONTAINS", "src/api.py::ApiService", "src/api.py::ApiService.fetch", "src/api.py"),
        _edge("CONTAINS", "src/api.py::ApiService", "src/api.py::ApiService.sync", "src/api.py"),
        _edge("CONTAINS", "src/db.py", "src/db.py::query", "src/db.py"),
        _edge("CONTAINS", "pkg/api.py", "pkg/api.py::mirror", "pkg/api.py"),
        _edge(
            "CALLS",
            "src/api.py::ApiService.fetch",
            "src/db.py::query",
            "src/api.py",
        ),
        _edge(
            "CALLS",
            "src/api.py::ApiService.sync",
            "src/db.py::query",
            "src/api.py",
        ),
        _edge(
            "IMPORTS_FROM",
            "src/api.py::ApiService.fetch",
            "src/db.py",
            "src/api.py",
        ),
        _edge(
            "CALLS",
            "tests/test_api.py::test_fetch",
            "src/api.py::ApiService.fetch",
            "tests/test_api.py",
        ),
        _edge(
            "CALLS",
            "src/api.py::ApiService.fetch",
            "src/api.py::ApiService.sync",
            "src/api.py",
        ),
        _edge(
            "IMPORTS_FROM",
            "src/api.py::ApiService.fetch",
            "requests",
            "src/api.py",
        ),
    ]
    for edge in edges:
        store.upsert_edge(edge)

    store.commit()
    return store


class TestExportGraphML:
    def test_returns_path(self, populated_store, tmp_path):
        from dagayn.exports import export_graphml

        out = tmp_path / "graph.graphml"
        result = export_graphml(populated_store, out)
        assert result == out

    def test_file_exists(self, populated_store, tmp_path):
        from dagayn.exports import export_graphml

        out = tmp_path / "graph.graphml"
        export_graphml(populated_store, out)
        assert out.exists()

    def test_contains_graphml_tags(self, populated_store, tmp_path):
        from dagayn.exports import export_graphml

        out = tmp_path / "graph.graphml"
        export_graphml(populated_store, out)
        content = out.read_text()
        assert "<graphml" in content
        assert "<node" in content
        assert "<edge" in content
        assert "</graphml>" in content

    def test_node_count(self, populated_store, tmp_path):
        from dagayn.exports import export_graphml

        out = tmp_path / "graph.graphml"
        export_graphml(populated_store, out)
        content = out.read_text()
        assert content.count("<node ") >= 3

    def test_edge_count(self, populated_store, tmp_path):
        from dagayn.exports import export_graphml

        out = tmp_path / "graph.graphml"
        export_graphml(populated_store, out)
        content = out.read_text()
        assert content.count("<edge ") >= 1

    def test_empty_store(self, tmp_path):
        from dagayn.exports import export_graphml

        store = GraphStore(tmp_path / "empty.db")
        store.commit()
        out = tmp_path / "empty.graphml"
        export_graphml(store, out)
        assert out.exists()
        content = out.read_text()
        assert "<graphml" in content


class TestExportNeo4jCypher:
    def test_returns_path(self, populated_store, tmp_path):
        from dagayn.exports import export_neo4j_cypher

        out = tmp_path / "graph.cypher"
        result = export_neo4j_cypher(populated_store, out)
        assert result == out

    def test_file_exists(self, populated_store, tmp_path):
        from dagayn.exports import export_neo4j_cypher

        out = tmp_path / "graph.cypher"
        export_neo4j_cypher(populated_store, out)
        assert out.exists()

    def test_contains_create_statements(self, populated_store, tmp_path):
        from dagayn.exports import export_neo4j_cypher

        out = tmp_path / "graph.cypher"
        export_neo4j_cypher(populated_store, out)
        content = out.read_text()
        assert "CREATE" in content

    def test_contains_node_labels(self, populated_store, tmp_path):
        from dagayn.exports import export_neo4j_cypher

        out = tmp_path / "graph.cypher"
        export_neo4j_cypher(populated_store, out)
        content = out.read_text()
        assert ":Class" in content or ":Function" in content or ":File" in content

    def test_contains_match_for_edges(self, populated_store, tmp_path):
        from dagayn.exports import export_neo4j_cypher

        out = tmp_path / "graph.cypher"
        export_neo4j_cypher(populated_store, out)
        content = out.read_text()
        assert "MATCH" in content

    def test_cypher_escape(self):
        from dagayn.exports import _cypher_escape

        assert _cypher_escape("it's") == "it\\'s"
        assert _cypher_escape("back\\slash") == "back\\\\slash"
        assert _cypher_escape("normal") == "normal"

    def test_cypher_props_formatting(self):
        from dagayn.exports import _cypher_props

        result = _cypher_props({"name": "foo", "count": 3})
        assert "name: 'foo'" in result
        assert "count: 3" in result
        assert result.startswith("{")
        assert result.endswith("}")


class TestExportObsidianVault:
    def test_returns_directory(self, populated_store, tmp_path):
        from dagayn.exports import export_obsidian_vault

        out = tmp_path / "vault"
        result = export_obsidian_vault(populated_store, out)
        assert result == out

    def test_directory_exists(self, populated_store, tmp_path):
        from dagayn.exports import export_obsidian_vault

        out = tmp_path / "vault"
        export_obsidian_vault(populated_store, out)
        assert out.is_dir()

    def test_creates_index(self, populated_store, tmp_path):
        from dagayn.exports import export_obsidian_vault

        out = tmp_path / "vault"
        export_obsidian_vault(populated_store, out)
        assert (out / "_INDEX.md").exists()

    def test_creates_node_pages(self, populated_store, tmp_path):
        from dagayn.exports import export_obsidian_vault

        out = tmp_path / "vault"
        export_obsidian_vault(populated_store, out)
        md_files = list(out.glob("*.md"))
        assert len(md_files) >= 2

    def test_index_has_node_links(self, populated_store, tmp_path):
        from dagayn.exports import export_obsidian_vault

        out = tmp_path / "vault"
        export_obsidian_vault(populated_store, out)
        index = (out / "_INDEX.md").read_text()
        assert "[[" in index

    def test_node_pages_have_frontmatter(self, populated_store, tmp_path):
        from dagayn.exports import export_obsidian_vault

        out = tmp_path / "vault"
        export_obsidian_vault(populated_store, out)
        for f in out.glob("*.md"):
            if f.name == "_INDEX.md":
                continue
            content = f.read_text()
            assert content.startswith("---")
            break

    def test_obsidian_slug(self):
        from dagayn.exports import _obsidian_slug

        assert _obsidian_slug("MyClass") == "myclass"
        assert _obsidian_slug("my function") == "my-function"
        assert _obsidian_slug("") == "unnamed"
        assert len(_obsidian_slug("a" * 200)) <= 100

    def test_export_obsidian_vault_surfaces_knowledge_gap_hotspot(
        self, knowledge_gap_hotspot_store, tmp_path
    ):
        """High-degree untested nodes should export as first-class vault pages."""
        from dagayn.analysis import find_knowledge_gaps
        from dagayn.exports import export_obsidian_vault, _obsidian_slug

        gaps = find_knowledge_gaps(
            knowledge_gap_hotspot_store,
            top_n=5,
            artifact_scope="code",
        )
        hotspot_qn = "src/service.py::service"
        hotspot_names = {item["qualified_name"] for item in gaps["untested_hotspots"]}
        assert hotspot_qn in hotspot_names

        out = tmp_path / "vault"
        export_obsidian_vault(knowledge_gap_hotspot_store, out)

        hotspot_slug = _obsidian_slug("service")
        hotspot_page = out / f"{hotspot_slug}.md"
        assert hotspot_page.exists()

        hotspot_body = hotspot_page.read_text(encoding="utf-8")
        assert "src/service.py" in hotspot_body
        assert "## Connections" in hotspot_body
        assert "[[" in hotspot_body

        index = (out / "_INDEX.md").read_text(encoding="utf-8")
        assert f"[[{hotspot_slug}]]" in index

    def test_export_obsidian_vault_hotspot_links_callers(
        self, knowledge_gap_hotspot_store, tmp_path
    ):
        """Obsidian export should connect hotspot nodes to their callers."""
        from dagayn.exports import export_obsidian_vault, _obsidian_slug

        out = tmp_path / "vault"
        export_obsidian_vault(knowledge_gap_hotspot_store, out)

        hotspot_page = (out / f"{_obsidian_slug('service')}.md").read_text(encoding="utf-8")
        caller_slug = _obsidian_slug("caller_0")
        assert f"[[{caller_slug}" in hotspot_page


class TestExportSVG:
    def test_requires_matplotlib(self, populated_store, tmp_path):
        matplotlib = pytest.importorskip("matplotlib")  # noqa: F841
        from dagayn.exports import export_svg

        out = tmp_path / "graph.svg"
        result = export_svg(populated_store, out)
        assert result == out
        assert out.exists()
        content = out.read_text()
        assert "<svg" in content

    def test_empty_store_raises(self, tmp_path):
        pytest.importorskip("matplotlib")
        from dagayn.exports import export_svg

        store = GraphStore(tmp_path / "empty.db")
        store.commit()
        out = tmp_path / "empty.svg"
        with pytest.raises(ValueError, match="empty"):
            export_svg(store, out)


class TestExportMermaidC4:
    def test_returns_path(self, populated_store, tmp_path):
        from dagayn.exports import export_mermaid_c4

        out = tmp_path / "graph.mmd"
        result = export_mermaid_c4(populated_store, out)
        assert result == out

    def test_contains_c4_component_syntax(self, populated_store, tmp_path):
        from dagayn.exports import export_mermaid_c4

        out = tmp_path / "graph.mmd"
        export_mermaid_c4(populated_store, out)
        content = out.read_text()
        assert "C4Component" in content
        assert 'Container_Boundary(repo, "Repository")' in content
        assert "Component(" in content
        assert "Rel(" in content

    def test_contains_file_components(self, populated_store, tmp_path):
        from dagayn.exports import export_mermaid_c4

        out = tmp_path / "graph.mmd"
        export_mermaid_c4(populated_store, out)
        content = out.read_text()
        assert "src/app.py" in content
        assert "tests/test_app.py" in content
        assert "app.py" in content
        assert "test_app.py" in content

    def test_empty_store(self, tmp_path):
        from dagayn.exports import export_mermaid_c4

        store = GraphStore(tmp_path / "empty.db")
        store.commit()
        out = tmp_path / "empty.mmd"
        export_mermaid_c4(store, out)
        content = out.read_text()
        assert "C4Component" in content
        assert "Component(" not in content

    def test_groups_components_by_top_level_directory(self, mermaid_store, tmp_path):
        from dagayn.exports import export_mermaid_c4

        out = tmp_path / "graph.mmd"
        export_mermaid_c4(mermaid_store, out)
        content = out.read_text()
        assert "  %% ." in content
        assert "  %% pkg" in content
        assert "  %% src" in content
        assert "  %% tests" in content

    def test_counts_symbols_per_file_in_description(self, mermaid_store, tmp_path):
        from dagayn.exports import export_mermaid_c4

        out = tmp_path / "graph.mmd"
        export_mermaid_c4(mermaid_store, out)
        content = out.read_text()
        assert '"src/api.py · 3 symbols"' in content
        assert '"src/db.py · 1 symbols"' in content
        assert '"README.md · 0 symbols"' in content

    def test_aggregates_cross_file_relations_by_kind_and_count(self, mermaid_store, tmp_path):
        from dagayn.exports import export_mermaid_c4

        out = tmp_path / "graph.mmd"
        export_mermaid_c4(mermaid_store, out)
        content = out.read_text()
        assert 'Rel(cmp_src_api_py, cmp_src_db_py, "CALLS x2, IMPORTS_FROM")' in content
        assert 'Rel(cmp_tests_test_api_py, cmp_src_api_py, "CALLS")' in content

    def test_skips_same_file_and_unknown_target_relations(self, mermaid_store, tmp_path):
        from dagayn.exports import export_mermaid_c4

        out = tmp_path / "graph.mmd"
        export_mermaid_c4(mermaid_store, out)
        content = out.read_text()
        assert content.count("Rel(") == 2
        assert "requests" not in content

    def test_uses_unique_component_ids_for_duplicate_basenames(self, mermaid_store, tmp_path):
        from dagayn.exports import export_mermaid_c4

        out = tmp_path / "graph.mmd"
        export_mermaid_c4(mermaid_store, out)
        content = out.read_text()
        assert 'Component(cmp_src_api_py, "api.py"' in content
        assert 'Component(cmp_pkg_api_py, "api.py"' in content


class TestMermaidHelpers:
    def test_mermaid_id_sanitizes_and_prefixes(self):
        from dagayn.exports import _mermaid_id

        assert _mermaid_id("123/path-name.py", prefix="cmp") == "cmp_n_123_path_name_py"

    def test_mermaid_escape_normalizes_strings(self):
        from dagayn.exports import _mermaid_escape

        assert _mermaid_escape('a\\b"c\nd') == "a/b'c d"

    def test_format_relation_label_orders_and_counts(self):
        from collections import Counter

        from dagayn.exports import _format_relation_label

        assert _format_relation_label(Counter({"IMPORTS_FROM": 1, "CALLS": 2})) == (
            "CALLS x2, IMPORTS_FROM"
        )
