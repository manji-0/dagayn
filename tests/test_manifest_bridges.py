"""Focused tests for Phase 3 manifest-backed CROSS_ARTIFACT bridges."""

from __future__ import annotations

import json
import tempfile
from pathlib import Path, PurePosixPath

from dagayn.contracts.state_types import PostprocessResult
from dagayn.graph import GraphStore
from dagayn.incremental import full_build
from dagayn.parser._base.types import NodeInfo
from dagayn.parser.manifest_bridges import (
    EXTRACTOR_ID,
    _resolve_rel,
    discover_manifest_bridges,
)
from dagayn.postprocessing import _apply_manifest_bridges, run_post_processing
from tests.store_sql import store_conn

FIXTURES = Path(__file__).parent / "fixtures" / "cross_artifact_manifest"


def _ca_edges(store: GraphStore) -> list:
    rows = (
        store_conn(store)
        .execute(
            "SELECT source_qualified, target_qualified, extra "
            "FROM edges WHERE kind='CROSS_ARTIFACT'"
        )
        .fetchall()
    )
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

    def test_cargo_manifest_links_library_root(self):
        result = discover_manifest_bridges(FIXTURES / "py_rust")
        edges = [
            e for e in result.edges if e.extra.get("relationship_role") == "builds_from_source"
        ]
        assert len(edges) == 1
        edge = edges[0]
        assert (edge.source, edge.target) == ("rust/Cargo.toml", "rust/src/lib.rs")
        assert edge.extra["lib_name"] == "demo_native"
        assert edge.extra["crate_dir"] == "rust"
        # maturin's module-name is what Python imports.
        assert edge.extra["python_module"] == "demo_native._core"
        assert edge.extra["confidence_tier"] == "HIGH"

    def test_cargo_manifest_without_cdylib_or_maturin_is_skipped(self, tmp_path: Path):
        (tmp_path / "src").mkdir()
        (tmp_path / "src" / "lib.rs").write_text("pub fn f() {}\n")
        (tmp_path / "Cargo.toml").write_text('[package]\nname = "plain"\n')
        assert discover_manifest_bridges(tmp_path).edges == []
        (tmp_path / "Cargo.toml").write_text(
            '[package]\nname = "plain-lib"\n\n[lib]\ncrate-type = ["cdylib"]\n'
        )
        [edge] = discover_manifest_bridges(tmp_path).edges
        assert edge.extra["lib_name"] == "plain_lib"
        assert "python_module" not in edge.extra

    def test_wasm_bindgen_crate_records_js_packages_and_out_dirs(self, tmp_path: Path):
        (tmp_path / "wasm" / "src").mkdir(parents=True)
        (tmp_path / "wasm" / "src" / "lib.rs").write_text("pub fn f() {}\n")
        (tmp_path / "wasm" / "Cargo.toml").write_text(
            '[package]\nname = "fast-sum"\n\n[lib]\ncrate-type = ["cdylib"]\n\n'
            '[dependencies]\nwasm-bindgen = "0.2"\n'
        )
        (tmp_path / "web").mkdir()
        (tmp_path / "web" / "package.json").write_text(
            json.dumps(
                {
                    "scripts": {
                        "wasm": "wasm-pack build ../wasm --out-dir ../web/wasm-out --scope acme"
                    },
                    "dependencies": {
                        "fast-sum": "file:../wasm/pkg",
                        "@acme/fast-sum": "file:wasm-out",
                    },
                }
            )
        )
        [edge] = discover_manifest_bridges(tmp_path).edges
        assert edge.extra["wasm_bindgen"] is True
        assert edge.extra["wasm_out_dirs"] == ["wasm/pkg", "web/wasm-out"]
        assert edge.extra["js_packages"] == ["fast-sum", "@acme/fast-sum"]

    def test_cdylib_without_wasm_bindgen_has_no_js_packages(self, tmp_path: Path):
        (tmp_path / "src").mkdir()
        (tmp_path / "src" / "lib.rs").write_text("pub fn f() {}\n")
        (tmp_path / "Cargo.toml").write_text(
            '[package]\nname = "plain"\n\n[lib]\ncrate-type = ["cdylib"]\n'
        )
        [edge] = discover_manifest_bridges(tmp_path).edges
        assert "js_packages" not in edge.extra

    def test_go_and_tinygo_wasm_builds_in_scripts_and_makefiles(self, tmp_path: Path):
        (tmp_path / "gowasm").mkdir()
        (tmp_path / "gowasm" / "util.go").write_text("package main\n")
        (tmp_path / "gowasm" / "main.go").write_text("package main\n\nfunc main() {}\n")
        (tmp_path / "tiny").mkdir()
        (tmp_path / "tiny" / "lib.go").write_text("package main\n")
        (tmp_path / "web").mkdir()
        (tmp_path / "web" / "package.json").write_text(
            json.dumps(
                {
                    "scripts": {
                        "go": "GOOS=js GOARCH=wasm go build -o public/app.wasm ../gowasm",
                        # Not a WebAssembly build.
                        "server": "go build -o bin/server ../gowasm",
                    }
                }
            )
        )
        (tmp_path / "Makefile").write_text(
            "wasm:\n\ttinygo build -o web/public/tiny.wasm \\\n\t\t-target wasi ./tiny\n"
        )
        edges = {
            (e.source, e.target): e.extra
            for e in discover_manifest_bridges(tmp_path).edges
            if e.extra.get("manifest_kind") == "wasm_build"
        }
        assert edges.keys() == {
            ("web/package.json", "gowasm/main.go"),
            ("Makefile", "tiny/lib.go"),
        }
        go = edges[("web/package.json", "gowasm/main.go")]
        assert go["wasm_producer"] == "go"
        assert go["wasm_outputs"] == ["web/public/app.wasm"]
        assert go["export_dir"] == "gowasm"
        tiny = edges[("Makefile", "tiny/lib.go")]
        assert tiny["wasm_producer"] == "tinygo"
        assert tiny["wasm_outputs"] == ["web/public/tiny.wasm"]

    def test_assemblyscript_asconfig_and_asc_script(self, tmp_path: Path):
        (tmp_path / "as" / "assembly").mkdir(parents=True)
        (tmp_path / "as" / "assembly" / "index.ts").write_text(
            "export function f(): i32 { return 1; }\n"
        )
        (tmp_path / "as" / "asconfig.json").write_text(
            json.dumps(
                {
                    "entries": ["assembly/index.ts"],
                    "targets": {
                        "debug": {"outFile": "build/debug.wasm"},
                        "release": {"outFile": "build/release.wasm"},
                    },
                }
            )
        )
        (tmp_path / "as" / "package.json").write_text(
            json.dumps({"scripts": {"small": "asc assembly/index.ts --outFile out/small.wasm -O3"}})
        )
        edges = {
            e.source: e.extra
            for e in discover_manifest_bridges(tmp_path).edges
            if e.extra.get("wasm_producer") == "assemblyscript"
        }
        assert edges["as/asconfig.json"]["wasm_outputs"] == [
            "as/build/debug.wasm",
            "as/build/release.wasm",
        ]
        assert edges["as/asconfig.json"]["entry_files"] == ["as/assembly/index.ts"]
        assert edges["as/package.json"]["wasm_outputs"] == ["as/out/small.wasm"]

    def test_manifest_walk_prunes_ignored_directories(self, tmp_path: Path):
        nested = tmp_path / "node_modules" / "dep"
        nested.mkdir(parents=True)
        (nested / "asconfig.json").write_text(
            json.dumps({"entries": ["a.ts"], "targets": {"r": {"outFile": "a.wasm"}}})
        )
        (nested / "a.ts").write_text("export function a(): void {}\n")
        assert discover_manifest_bridges(tmp_path).edges == []

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
        result = PostprocessResult()
        _apply_manifest_bridges(self.store, result, [])
        # pyproject.toml -> Cargo.toml, and Cargo.toml -> its library root.
        assert result.manifest_bridges_edges == 2

        edges = {(source, target) for source, target, _ in _manifest_edges(self.store)}
        assert edges == {
            ("pyproject.toml", "rust/Cargo.toml"),
            ("rust/Cargo.toml", "rust/src/lib.rs"),
        }

        stats = self.store.get_stats()
        assert stats.edges_by_kind.get("CROSS_ARTIFACT", 0) == 2

    def test_apply_is_idempotent(self):
        repo = FIXTURES / "generated_client"
        self.store.set_metadata("repo_root", str(repo.resolve()))
        first = PostprocessResult()
        second = PostprocessResult()
        _apply_manifest_bridges(self.store, first, [])
        _apply_manifest_bridges(self.store, second, [])
        assert first.manifest_bridges_edges == second.manifest_bridges_edges == 2
        assert len(_manifest_edges(self.store)) == 2

    def test_apply_rolls_back_when_upsert_fails(self, monkeypatch):
        repo = FIXTURES / "py_rust"
        self.store.set_metadata("repo_root", str(repo.resolve()))
        _apply_manifest_bridges(self.store, PostprocessResult(), [])
        assert len(_manifest_edges(self.store)) == 2
        prior = _manifest_edges(self.store)

        def boom(*_args, **_kwargs):
            raise RuntimeError("simulated upsert failure")

        monkeypatch.setattr(self.store, "replace_manifest_bridges_json", boom)
        warnings: list[str] = []
        result = PostprocessResult()
        _apply_manifest_bridges(self.store, result, warnings)

        assert len(_manifest_edges(self.store)) == 2
        assert _manifest_edges(self.store) == prior
        assert any("Manifest bridge extraction failed" in w for w in warnings)
        assert result.manifest_bridges_edges is None

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

        _apply_manifest_bridges(self.store, PostprocessResult(), [])

        row = (
            store_conn(self.store)
            .execute(
                "SELECT file_hash, mtime_ns, extra FROM nodes WHERE qualified_name=?",
                ("pyproject.toml",),
            )
            .fetchone()
        )
        assert row is not None
        assert row["file_hash"] == "parser-hash-abc"
        assert row["mtime_ns"] == 1_700_000_000_000_000_000
        extra = json.loads(row["extra"] or "{}")
        assert extra.get("extractor") != EXTRACTOR_ID
        assert len(_manifest_edges(self.store)) == 2

    def test_full_build_postprocess_surfaces_manifest_edges(self):
        repo = FIXTURES / "generated_client"
        full_build(repo, self.store)
        result = run_post_processing(self.store)
        assert (result.manifest_bridges_edges or 0) >= 2

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
