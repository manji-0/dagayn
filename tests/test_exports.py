"""Tests for dagayn/exports.py: GraphML, Neo4j Cypher, Obsidian vault, SVG."""

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
