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

    def test_napi_and_neon_crates_record_node_addon_facts(self, tmp_path: Path):
        for crate, dep in (("napi-crate", 'napi = "3"'), ("neon-crate", 'neon = "1"')):
            (tmp_path / crate / "src").mkdir(parents=True)
            (tmp_path / crate / "src" / "lib.rs").write_text("")
            (tmp_path / crate / "Cargo.toml").write_text(
                f'[package]\nname = "{crate}"\n\n[lib]\ncrate-type = ["cdylib"]\n\n'
                f"[dependencies]\n{dep}\n"
            )
        (tmp_path / "napi-crate" / "package.json").write_text(
            json.dumps({"name": "@demo/napi", "napi": {"binaryName": "demo"}})
        )
        (tmp_path / "neon-crate" / "package.json").write_text(
            json.dumps({"name": "demo-neon", "main": "lib/addon.node"})
        )
        (tmp_path / "web").mkdir()
        (tmp_path / "web" / "package.json").write_text(
            json.dumps({"dependencies": {"napi-local": "file:../napi-crate"}})
        )
        edges = {
            e.source: e.extra
            for e in discover_manifest_bridges(tmp_path).edges
            if e.extra.get("node_addon")
        }
        napi = edges["napi-crate/Cargo.toml"]
        assert napi["node_addon"] == "napi"
        assert napi["js_packages"] == ["@demo/napi", "napi-local"]
        assert napi["js_entry_files"] == ["napi-crate/index.js", "napi-crate/index.d.ts"]
        assert napi["node_binary_names"] == ["demo"]
        neon = edges["neon-crate/Cargo.toml"]
        assert neon["node_addon"] == "neon"
        assert neon["js_packages"] == ["demo-neon"]
        assert neon["node_outputs"] == ["neon-crate/lib/addon.node", "neon-crate/index.node"]
        assert neon["js_entry_files"] == []

    def test_cmake_shared_libraries(self, tmp_path: Path):
        src = tmp_path / "native" / "src"
        src.mkdir(parents=True)
        for name in ("a.c", "b.cpp", "extra.c", "static_only.c", "gen1.c"):
            (src / name).write_text("int f(void) { return 0; }\n")
        (src / "a.h").write_text("int f(void);\n")
        (tmp_path / "native" / "CMakeLists.txt").write_text(
            "project(demo C CXX)\n"
            "#[[ add_library(commented SHARED src/a.c) ]]\n"
            "set(CORE src/a.c src/a.h)\n"
            "list(APPEND CORE ${CMAKE_CURRENT_SOURCE_DIR}/src/b.cpp)\n"
            "file(GLOB GENERATED CONFIGURE_DEPENDS src/gen*.c)\n"
            "add_library(core SHARED ${CORE} ${GENERATED})\n"
            "target_sources(core PRIVATE src/extra.c)\n"
            'set_target_properties(core PROPERTIES OUTPUT_NAME "demo-core")\n'
            "add_library(plugin MODULE src/missing.c)\n"
            "add_library(archive STATIC src/static_only.c)\n"
            "add_library(core::alias ALIAS core)\n"
        )
        edges = [
            e
            for e in discover_manifest_bridges(tmp_path).edges
            if e.extra.get("manifest_kind") == "native_library"
        ]
        assert [(e.source, e.target) for e in edges] == [
            ("native/CMakeLists.txt", "native/src/a.c")
        ]
        extra = edges[0].extra
        assert extra["build_system"] == "cmake"
        assert extra["lib_name"] == "demo_core"
        assert extra["target_language"] == "cpp"
        assert extra["source_files"] == [
            "native/src/a.c",
            "native/src/b.cpp",
            "native/src/gen1.c",
            "native/src/extra.c",
        ]

    def test_cmake_build_shared_libs_makes_plain_add_library_shared(self, tmp_path: Path):
        (tmp_path / "lib.c").write_text("int f(void) { return 0; }\n")
        (tmp_path / "CMakeLists.txt").write_text(
            'option(BUILD_SHARED_LIBS "shared" ON)\nadd_library(plain lib.c)\n'
        )
        libs = [
            e.extra["lib_name"]
            for e in discover_manifest_bridges(tmp_path).edges
            if e.extra.get("manifest_kind") == "native_library"
        ]
        assert libs == ["plain"]

    def test_meson_libraries_respect_default_library(self, tmp_path: Path):
        (tmp_path / "a").mkdir()
        (tmp_path / "a" / "x.c").write_text("int x(void) { return 0; }\n")
        (tmp_path / "a" / "y.c").write_text("int y(void) { return 0; }\n")
        (tmp_path / "a" / "meson.build").write_text(
            "srcs = ['x.c']\nsrcs += files('y.c')\n"
            "library('dyn', srcs, c_args: '-DX')  # shared by default\n"
            "static_library('st', 'x.c')\n"
        )
        (tmp_path / "b").mkdir()
        (tmp_path / "b" / "z.c").write_text("int z(void) { return 0; }\n")
        (tmp_path / "b" / "meson.build").write_text(
            "project('b', 'c', default_options: ['default_library=static'])\n"
            "library('onlystatic', 'z.c')\n"
            "shared_module('plug', sources: ['z.c'])\n"
        )
        edges = {
            e.extra["lib_name"]: e.extra["source_files"]
            for e in discover_manifest_bridges(tmp_path).edges
            if e.extra.get("manifest_kind") == "native_library"
        }
        assert edges == {"dyn": ["a/x.c", "a/y.c"], "plug": ["b/z.c"]}

    def test_binding_gyp_targets_are_node_addons(self, tmp_path: Path):
        (tmp_path / "addon" / "src").mkdir(parents=True)
        (tmp_path / "addon" / "src" / "a.cc").write_text("int a() { return 0; }\n")
        (tmp_path / "addon" / "binding.gyp").write_text(
            "{\n  # comment\n  'targets': [\n"
            "    {'target_name': 'native', 'sources': ['src/a.cc', 'src/missing.cc']},\n"
            "    {'target_name': 'tool', 'type': 'executable', 'sources': ['src/a.cc']},\n"
            "  ],\n}\n"
        )
        (tmp_path / "addon" / "package.json").write_text(
            json.dumps({"name": "native-addon", "main": "index.js"})
        )
        edges = [
            e.extra
            for e in discover_manifest_bridges(tmp_path).edges
            if e.extra.get("manifest_kind") == "native_library"
        ]
        assert len(edges) == 1
        extra = edges[0]
        assert extra["build_system"] == "node-gyp"
        assert extra["lib_name"] == "native"
        assert extra["source_files"] == ["addon/src/a.cc"]
        assert extra["node_addon"] == "node-gyp"
        assert extra["js_packages"] == ["native-addon"]
        assert extra["js_entry_files"] == ["addon/index.js"]
        assert extra["node_outputs"] == [
            "addon/build/Release/native.node",
            "addon/build/Debug/native.node",
        ]

    def test_emscripten_commands_record_outputs_and_exports(self, tmp_path: Path):
        (tmp_path / "src").mkdir()
        (tmp_path / "src" / "lib.c").write_text("int add(int a, int b) { return a + b; }\n")
        (tmp_path / "package.json").write_text(
            json.dumps(
                {
                    "scripts": {
                        "wasm": "emcc src/lib.c -o dist/lib.mjs -s "
                        "EXPORTED_FUNCTIONS=['_add','_malloc']",
                        "page": "em++ src/lib.c -o site/index.html",
                    }
                }
            )
        )
        edges = {
            e.extra["lib_name"]: e.extra
            for e in discover_manifest_bridges(tmp_path).edges
            if e.extra.get("build_system") == "emscripten"
        }
        assert edges["lib"]["wasm_outputs"] == ["dist/lib.mjs", "dist/lib.wasm"]
        assert edges["lib"]["wasm_exports"] == ["add", "malloc"]
        assert edges["lib"]["source_files"] == ["src/lib.c"]
        assert edges["index"]["wasm_outputs"] == [
            "site/index.html",
            "site/index.js",
            "site/index.wasm",
        ]
        assert "wasm_exports" not in edges["index"]

    def test_build_rs_cc_and_cxx_chains(self, tmp_path: Path):
        crate = tmp_path / "sys"
        (crate / "csrc").mkdir(parents=True)
        for name in ("a.c", "b.c", "c.c", "bridge.cc"):
            (crate / "csrc" / name).write_text("int f(void) { return 0; }\n")
        (crate / "Cargo.toml").write_text('[package]\nname = "sys"\n')
        (crate / "build.rs").write_text(
            "fn main() {\n"
            '    // cc::Build::new().file("csrc/c.c").compile("commented");\n'
            "    cc::Build::new()\n"
            '        .file("csrc/a.c")\n'
            '        .files(&["csrc/b.c", "csrc/missing.c"])\n'
            '        .compile("fast");\n'
            '    cxx_build::bridge("src/main.rs").file("csrc/bridge.cc").compile("demo");\n'
            "}\n"
        )
        # A build.rs without a Cargo.toml beside it is not a build script.
        (tmp_path / "stray").mkdir()
        (tmp_path / "stray" / "build.rs").write_text(
            'fn main() { cc::Build::new().file("x.c").compile("x"); }\n'
        )
        edges = {
            e.extra["lib_name"]: e.extra
            for e in discover_manifest_bridges(tmp_path).edges
            if e.extra.get("build_system") in ("cc", "cxx")
        }
        assert edges.keys() == {"fast", "demo"}
        assert edges["fast"]["source_files"] == ["sys/csrc/a.c", "sys/csrc/b.c"]
        assert edges["fast"]["build_system"] == "cc"
        assert edges["demo"]["build_system"] == "cxx"
        assert edges["demo"]["source_files"] == ["sys/csrc/bridge.cc"]

    def test_compiler_commands_in_makefiles_and_scripts(self, tmp_path: Path):
        (tmp_path / "c").mkdir()
        (tmp_path / "c" / "one.c").write_text("int one(void) { return 1; }\n")
        (tmp_path / "c" / "two.cc").write_text('extern "C" int two() { return 2; }\n')
        (tmp_path / "c" / "Makefile").write_text(
            "OBJS = one.o two.o\nLIB := libpair.so\n\n"
            "$(LIB): $(OBJS)\n\t@$(CXX) -shared -Wl,-soname,$@ -o $@ $^\n\n"
            "app: one.o\n\t$(CC) -o app one.o\n"
        )
        (tmp_path / "package.json").write_text(
            json.dumps({"scripts": {"native": "clang -dynamiclib -o out/libone.dylib c/one.c"}})
        )
        edges = {
            (e.source, e.extra["lib_name"]): e.extra["source_files"]
            for e in discover_manifest_bridges(tmp_path).edges
            if e.extra.get("manifest_kind") == "native_library"
        }
        assert edges == {
            ("c/Makefile", "pair"): ["c/one.c", "c/two.cc"],
            ("package.json", "one"): ["c/one.c"],
        }

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
