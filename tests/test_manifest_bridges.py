"""Focused tests for Phase 3 manifest-backed CROSS_ARTIFACT bridges."""

from __future__ import annotations

import json
import tempfile
from pathlib import Path, PurePosixPath

from dagayn.graph import GraphStore
from dagayn.incremental import full_build
from dagayn.parser._base.types import NodeInfo
from dagayn.parser.manifest_bridges import (
    EXTRACTOR_ID,
    _resolve_rel,
    discover_manifest_bridges,
)
from dagayn.postprocessing import _apply_manifest_bridges, run_post_processing

FIXTURES = Path(__file__).parent / "fixtures" / "cross_artifact_manifest"


def _ca_edges(store: GraphStore) -> list:
    rows = store._conn.execute(
        "SELECT source_qualified, target_qualified, extra FROM edges WHERE kind='CROSS_ARTIFACT'"
    ).fetchall()
    out = []
    for row in rows:
        extra = json.loads(row["extra"] or "{}")
        out.append((row["source_qualified"], row["target_qualified"], extra))
    return out


def _manifest_edges(store: GraphStore) -> list:
    return [e for e in _ca_edges(store) if e[2].get("extractor") == EXTRACTOR_ID]


class TestResolveRelContainment:
    def test_rejects_parent_traversal_escape(self):
        assert _resolve_rel(PurePosixPath("pkg"), "../../../etc/passwd") is None
        assert _resolve_rel(PurePosixPath("pkg/sub"), "../../..") is None
        assert _resolve_rel(PurePosixPath("."), "../../outside.toml") is None
        assert _resolve_rel(PurePosixPath("pkg"), "/../../etc/passwd") is None

    def test_allows_contained_relative_paths(self):
        assert _resolve_rel(PurePosixPath("pkg/sub"), "../Cargo.toml") == "pkg/Cargo.toml"
        assert _resolve_rel(PurePosixPath("."), "rust/Cargo.toml") == "rust/Cargo.toml"
        assert _resolve_rel(PurePosixPath("pkg"), "Cargo.toml") == "pkg/Cargo.toml"

    def test_absolute_paths_become_repo_relative_when_contained(self):
        assert _resolve_rel(PurePosixPath("pkg"), "/rust/Cargo.toml") == "rust/Cargo.toml"

    def test_discover_skips_escaping_maturin_manifest_path(self, tmp_path: Path):
        (tmp_path / "pyproject.toml").write_text(
            '[tool.maturin]\nmanifest-path = "../../../etc/passwd"\nmodule-name = "evil"\n',
            encoding="utf-8",
        )
        result = discover_manifest_bridges(tmp_path)
        assert result.edges == []


class TestDiscoverManifestBridges:
    def test_maturin_pyproject_links_cargo(self):
        result = discover_manifest_bridges(FIXTURES / "py_rust")
        edges = [
            e
            for e in result.edges
            if e.extra.get("relationship_role") == "builds_artifact"
            and e.extra.get("manifest_kind") == "maturin"
        ]
        assert len(edges) == 1
        edge = edges[0]
        assert edge.source == "pyproject.toml"
        assert edge.target == "rust/Cargo.toml"
        assert edge.extra["confidence_tier"] == "EXACT"
        assert edge.extra["evidence_kind"] == "manifest"
        assert edge.extra["evidence_source"] == "tool.maturin.manifest-path"
        assert edge.extra["module_name"] == "demo_native._core"
        assert edge.extra["bridge_kind"] == "extension_module"

    def test_openapitools_schema_to_package_to_consumer(self):
        result = discover_manifest_bridges(FIXTURES / "generated_client")
        generate_edges = [
            e for e in result.edges if e.extra.get("relationship_role") == "generates_code"
        ]
        bind_edges = [
            e for e in result.edges if e.extra.get("relationship_role") == "binds_generated_client"
        ]
        assert len(generate_edges) == 1
        assert len(bind_edges) == 1

        gen = generate_edges[0]
        assert gen.source == "openapi.json"
        assert gen.target == "packages/api-client/package.json"
        assert gen.extra["confidence_tier"] == "EXACT"
        assert gen.extra["evidence_source"] == "openapitools.generator-cli.generators"

        bind = bind_edges[0]
        assert bind.source == "apps/web/package.json"
        assert bind.target == "packages/api-client/package.json"
        assert bind.extra["dependency_name"] == "@acme/api-client"
        assert bind.extra["confidence_tier"] == "EXACT"

        # Conceptual schema → package → consumer path via shared package node.
        package = gen.target
        assert bind.target == package

    def test_negative_fixture_emits_no_bridges(self):
        result = discover_manifest_bridges(FIXTURES / "negative")
        assert result.edges == []


class TestApplyManifestBridges:
    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def test_apply_persists_edges_and_stats(self):
        repo = FIXTURES / "py_rust"
        self.store.set_metadata("repo_root", str(repo.resolve()))
        result: dict = {}
        _apply_manifest_bridges(self.store, result, [])
        assert result["manifest_bridges_edges"] == 1

        edges = _manifest_edges(self.store)
        assert len(edges) == 1
        assert edges[0][0] == "pyproject.toml"
        assert edges[0][1] == "rust/Cargo.toml"

        stats = self.store.get_stats()
        assert stats.edges_by_kind.get("CROSS_ARTIFACT", 0) == 1

    def test_apply_is_idempotent(self):
        repo = FIXTURES / "generated_client"
        self.store.set_metadata("repo_root", str(repo.resolve()))
        first: dict = {}
        second: dict = {}
        _apply_manifest_bridges(self.store, first, [])
        _apply_manifest_bridges(self.store, second, [])
        assert first["manifest_bridges_edges"] == second["manifest_bridges_edges"] == 2
        assert len(_manifest_edges(self.store)) == 2

    def test_apply_rolls_back_when_upsert_fails(self, monkeypatch):
        repo = FIXTURES / "py_rust"
        self.store.set_metadata("repo_root", str(repo.resolve()))
        _apply_manifest_bridges(self.store, {}, [])
        assert len(_manifest_edges(self.store)) == 1
        prior = _manifest_edges(self.store)

        def boom(_edge):
            raise RuntimeError("simulated upsert failure")

        monkeypatch.setattr(self.store, "upsert_edge", boom)
        warnings: list[str] = []
        result: dict = {}
        _apply_manifest_bridges(self.store, result, warnings)

        assert len(_manifest_edges(self.store)) == 1
        assert _manifest_edges(self.store) == prior
        assert any("Manifest bridge extraction failed" in w for w in warnings)
        assert "manifest_bridges_edges" not in result

    def test_apply_preserves_existing_file_hash_and_mtime(self):
        repo = FIXTURES / "py_rust"
        self.store.set_metadata("repo_root", str(repo.resolve()))
        self.store.upsert_node(
            NodeInfo(
                kind="File",
                name="pyproject.toml",
                file_path="pyproject.toml",
                line_start=1,
                line_end=10,
                language="toml",
            ),
            file_hash="parser-hash-abc",
            mtime_ns=1_700_000_000_000_000_000,
        )
        self.store.commit()

        _apply_manifest_bridges(self.store, {}, [])

        row = self.store._conn.execute(
            "SELECT file_hash, mtime_ns, extra FROM nodes WHERE qualified_name=?",
            ("pyproject.toml",),
        ).fetchone()
        assert row is not None
        assert row["file_hash"] == "parser-hash-abc"
        assert row["mtime_ns"] == 1_700_000_000_000_000_000
        extra = json.loads(row["extra"] or "{}")
        assert extra.get("extractor") != EXTRACTOR_ID
        assert len(_manifest_edges(self.store)) == 1

    def test_full_build_postprocess_surfaces_manifest_edges(self):
        repo = FIXTURES / "generated_client"
        full_build(repo, self.store)
        result = run_post_processing(self.store)
        assert result.get("manifest_bridges_edges", 0) >= 2

        edges = _manifest_edges(self.store)
        roles = {e[2]["relationship_role"] for e in edges}
        assert "generates_code" in roles
        assert "binds_generated_client" in roles

        stats = self.store.get_stats()
        assert stats.edges_by_kind.get("CROSS_ARTIFACT", 0) >= 2

    def test_false_positive_rate_on_negative_fixture(self):
        repo = FIXTURES / "negative"
        full_build(repo, self.store)
        run_post_processing(self.store)
        assert _manifest_edges(self.store) == []
        assert self.store.get_stats().edges_by_kind.get("CROSS_ARTIFACT", 0) == 0
