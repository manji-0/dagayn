use super::*;

#[test]
fn shared_library_stem_strips_prefix_and_extension() {
    assert_eq!(
        shared_library_stem("native/target/release/libfastsum.dylib").as_deref(),
        Some("fastsum")
    );
    assert_eq!(
        shared_library_stem("./libfoo-bar.so.1.2").as_deref(),
        Some("foo_bar")
    );
    assert_eq!(
        shared_library_stem("C:\\x\\foo.dll").as_deref(),
        Some("foo")
    );
    assert_eq!(shared_library_stem("<dynamic:ctypes.CDLL@a.py:1>"), None);
}

#[test]
fn relative_modules_resolve_against_the_file_package() {
    assert_eq!(absolute_module(".", "pkg/__init__.py"), "pkg");
    assert_eq!(absolute_module("._core", "pkg/sub/a.py"), "pkg.sub._core");
    assert_eq!(absolute_module(".._core", "pkg/sub/a.py"), "pkg._core");
    assert!(module_matches("python.pkg._core", "pkg._core"));
    assert!(!module_matches("mypkg._core", "pkg._core"));
}

mod store_tests {
    use super::super::*;
    use serde_json::json;

    fn temp_db(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "dagayn-native-bindings-{name}-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn node(kind: &str, name: &str, file_path: &str, language: &str, extra: Value) -> NodeInput {
        NodeInput {
            kind: kind.to_string(),
            name: name.to_string(),
            file_path: file_path.to_string(),
            line_start: 1,
            line_end: 2,
            language: language.to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra,
        }
    }

    fn edge(kind: &str, source: &str, target: &str, file_path: &str, extra: Value) -> EdgeInput {
        EdgeInput {
            kind: kind.to_string(),
            source: source.to_string(),
            target: target.to_string(),
            file_path: file_path.to_string(),
            line: 1,
            extra,
        }
    }

    fn store_rust_crate(store: &mut GraphStore, lib_name: &str, python_module: Option<&str>) {
        let file = "rust/src/lib.rs";
        store
            .store_file_nodes_edges(
                file,
                &[
                    node("File", file, file, "rust", json!({})),
                    node(
                        "Function",
                        "fast_sum",
                        file,
                        "rust",
                        json!({"ffi_export": {"abi": "pyo3", "kind": "function", "name": "fast_sum"}}),
                    ),
                    node(
                        "Function",
                        "c_sum",
                        file,
                        "rust",
                        json!({"ffi_export": {"abi": "c", "kind": "function", "name": "c_sum"}}),
                    ),
                    node(
                        "Function",
                        "commit",
                        file,
                        "rust",
                        json!({"ffi_export": {"abi": "pyo3", "kind": "method", "name": "commit"}}),
                    ),
                ],
                &[],
                "",
                0,
            )
            .expect("store crate");
        let mut extra = json!({
            "relationship_role": "builds_from_source",
            "extractor": "manifest_bridges",
            "confidence_tier": "HIGH",
            "confidence": 0.8,
            "lib_name": lib_name,
            "crate_types": ["cdylib"],
            "crate_dir": "rust",
        });
        if let Some(module) = python_module {
            extra["python_module"] = json!(module);
        }
        store
            .replace_manifest_bridges(
                "manifest_bridges",
                &[],
                &[edge(
                    "CROSS_ARTIFACT",
                    "rust/Cargo.toml",
                    file,
                    "rust/Cargo.toml",
                    extra,
                )],
            )
            .expect("manifest bridge");
    }

    fn bridges(store: &GraphStore) -> Vec<(String, String, String)> {
        let mut stmt = store
            .conn
            .prepare(
                "SELECT source_qualified, target_qualified, \
                 json_extract(extra, '$.relationship_role') FROM edges \
                 WHERE json_extract(extra, '$.extractor') = 'native_bindings' \
                 ORDER BY 1, 2",
            )
            .unwrap();
        stmt.query_map([], |row| <(_, _, _)>::try_from(row))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    }

    fn triple(a: &str, b: &str, c: &str) -> (String, String, String) {
        (a.to_string(), b.to_string(), c.to_string())
    }

    #[test]
    fn binds_extension_module_imports_and_calls() {
        let path = temp_db("pyo3");
        let mut store = GraphStore::open(&path).expect("open");
        store_rust_crate(&mut store, "_core", Some("pkg._core"));
        store
            .store_file_nodes_edges(
                "pkg/__init__.py",
                &[
                    node(
                        "File",
                        "pkg/__init__.py",
                        "pkg/__init__.py",
                        "python",
                        json!({}),
                    ),
                    node("Function", "total", "pkg/__init__.py", "python", json!({})),
                    node(
                        "Function",
                        "via_module",
                        "pkg/__init__.py",
                        "python",
                        json!({}),
                    ),
                ],
                &[
                    edge(
                        "IMPORTS_FROM",
                        "pkg/__init__.py",
                        "pkg._core",
                        "pkg/__init__.py",
                        json!({"module": "pkg._core", "names": [["fast_sum", "fs"]]}),
                    ),
                    edge(
                        "IMPORTS_FROM",
                        "pkg/__init__.py",
                        "pkg/__init__.py",
                        "pkg/__init__.py",
                        json!({"module": ".", "names": [["_core", "_core"]]}),
                    ),
                    edge(
                        "CALLS",
                        "pkg/__init__.py::total",
                        "fs",
                        "pkg/__init__.py",
                        json!({}),
                    ),
                    edge(
                        "CALLS",
                        "pkg/__init__.py::via_module",
                        "fast_sum",
                        "pkg/__init__.py",
                        json!({"receiver": "_core"}),
                    ),
                    // A method name is not a module attribute.
                    edge(
                        "CALLS",
                        "pkg/__init__.py::via_module",
                        "commit",
                        "pkg/__init__.py",
                        json!({"receiver": "_core"}),
                    ),
                ],
                "",
                0,
            )
            .expect("store python");
        assert_eq!(store.resolve_native_bindings().unwrap(), 3);
        assert_eq!(
            bridges(&store),
            vec![
                triple("pkg/__init__.py", "rust/src/lib.rs", "loads_native_module"),
                triple(
                    "pkg/__init__.py::total",
                    "rust/src/lib.rs::fast_sum",
                    "calls_native_function"
                ),
                triple(
                    "pkg/__init__.py::via_module",
                    "rust/src/lib.rs::fast_sum",
                    "calls_native_function"
                ),
            ]
        );
        // Re-running replaces instead of duplicating.
        assert_eq!(store.resolve_native_bindings().unwrap(), 3);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn binds_ctypes_library_and_its_c_symbols() {
        let path = temp_db("ctypes");
        let mut store = GraphStore::open(&path).expect("open");
        store_rust_crate(&mut store, "fastsum", None);
        let loader_extra = json!({
            "relationship_role": "loads_shared_library",
            "bridge_kind": "ffi",
            "source_language": "python",
            "confidence_tier": "HIGH",
            "confidence": 0.8,
        });
        store
            .store_file_nodes_edges(
                "app/native.py",
                &[
                    node(
                        "File",
                        "app/native.py",
                        "app/native.py",
                        "python",
                        json!({}),
                    ),
                    node("Function", "load", "app/native.py", "python", json!({})),
                    node("Function", "total", "app/native.py", "python", json!({})),
                ],
                &[
                    edge(
                        "CROSS_ARTIFACT",
                        "app/native.py::load",
                        "rust/target/release/libfastsum.dylib",
                        "app/native.py",
                        loader_extra.clone(),
                    ),
                    edge(
                        "CROSS_ARTIFACT",
                        "app/native.py::load",
                        "libother.so",
                        "app/native.py",
                        loader_extra,
                    ),
                    edge(
                        "CALLS",
                        "app/native.py::total",
                        "c_sum",
                        "app/native.py",
                        json!({}),
                    ),
                    // pyo3 exports are not C symbols.
                    edge(
                        "CALLS",
                        "app/native.py::total",
                        "fast_sum",
                        "app/native.py",
                        json!({}),
                    ),
                ],
                "",
                0,
            )
            .expect("store python");
        assert_eq!(store.resolve_native_bindings().unwrap(), 2);
        assert_eq!(
            bridges(&store),
            vec![
                triple(
                    "app/native.py::load",
                    "rust/src/lib.rs",
                    "loads_shared_library"
                ),
                triple(
                    "app/native.py::total",
                    "rust/src/lib.rs::c_sum",
                    "calls_native_function"
                ),
            ]
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn binds_javascript_imports_and_calls_to_a_wasm_bindgen_crate() {
        let path = temp_db("wasm");
        let mut store = GraphStore::open(&path).expect("open");
        let lib = "wasm/src/lib.rs";
        store
            .store_file_nodes_edges(
                lib,
                &[
                    node("File", lib, lib, "rust", json!({})),
                    node(
                        "Function",
                        "mean_of",
                        lib,
                        "rust",
                        json!({"ffi_export": {"abi": "wasm", "kind": "function", "name": "meanOf"}}),
                    ),
                    node(
                        "Class",
                        "Accumulator",
                        lib,
                        "rust",
                        json!({"ffi_export": {"abi": "wasm", "kind": "class", "name": "Accumulator"}}),
                    ),
                ],
                &[],
                "",
                0,
            )
            .expect("store crate");
        store
            .replace_manifest_bridges(
                "manifest_bridges",
                &[],
                &[edge(
                    "CROSS_ARTIFACT",
                    "wasm/Cargo.toml",
                    lib,
                    "wasm/Cargo.toml",
                    json!({
                        "relationship_role": "builds_from_source",
                        "extractor": "manifest_bridges",
                        "confidence_tier": "HIGH",
                        "confidence": 0.8,
                        "lib_name": "fast_sum",
                        "crate_types": ["cdylib"],
                        "crate_dir": "wasm",
                        "wasm_bindgen": true,
                        "js_packages": ["fast-sum"],
                        "wasm_out_dirs": ["wasm/pkg"],
                    }),
                )],
            )
            .expect("manifest bridge");
        store
            .store_file_nodes_edges(
                "web/src/stats.ts",
                &[
                    node(
                        "File",
                        "web/src/stats.ts",
                        "web/src/stats.ts",
                        "typescript",
                        json!({}),
                    ),
                    node(
                        "Function",
                        "mean",
                        "web/src/stats.ts",
                        "typescript",
                        json!({}),
                    ),
                    node(
                        "Function",
                        "run",
                        "web/src/stats.ts",
                        "typescript",
                        json!({}),
                    ),
                ],
                &[
                    edge(
                        "IMPORTS_FROM",
                        "web/src/stats.ts",
                        "fast-sum",
                        "web/src/stats.ts",
                        json!({}),
                    ),
                    edge(
                        "IMPORTS_FROM",
                        "web/src/stats.ts",
                        "../../wasm/pkg/fast_sum.js",
                        "web/src/stats.ts",
                        json!({}),
                    ),
                    edge(
                        "CALLS",
                        "web/src/stats.ts::mean",
                        "fast-sum::meanOf",
                        "web/src/stats.ts",
                        json!({}),
                    ),
                    edge(
                        "CALLS",
                        "web/src/stats.ts::run",
                        "../../wasm/pkg/fast_sum.js::Accumulator",
                        "web/src/stats.ts",
                        json!({}),
                    ),
                    // wasm-bindgen's `init` is JS glue, not a Rust export.
                    edge(
                        "CALLS",
                        "web/src/stats.ts::run",
                        "fast-sum::default",
                        "web/src/stats.ts",
                        json!({}),
                    ),
                    // Another package with the same export name is not the crate.
                    edge(
                        "CALLS",
                        "web/src/stats.ts::run",
                        "other-pkg::meanOf",
                        "web/src/stats.ts",
                        json!({}),
                    ),
                ],
                "",
                0,
            )
            .expect("store typescript");
        assert_eq!(store.resolve_native_bindings().unwrap(), 3);
        assert_eq!(
            bridges(&store),
            vec![
                triple("web/src/stats.ts", lib, "loads_native_module"),
                triple(
                    "web/src/stats.ts::mean",
                    "wasm/src/lib.rs::mean_of",
                    "calls_native_function"
                ),
                triple(
                    "web/src/stats.ts::run",
                    "wasm/src/lib.rs::Accumulator",
                    "calls_native_function"
                ),
            ]
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn relative_specifiers_resolve_against_the_importing_file() {
        assert_eq!(
            join_relative("web/src/stats.ts", "../../wasm/pkg/fast_sum.js").as_deref(),
            Some("wasm/pkg/fast_sum.js")
        );
        assert_eq!(join_relative("a.ts", "../x"), None);
        assert_eq!(package_of("@scope/name/sub"), "@scope/name");
        assert_eq!(package_of("fast-sum/snippets"), "fast-sum");
    }

    fn wasm_build_edge(config: &str, root: &str, producer: &str, extra: Value) -> EdgeInput {
        let mut payload = json!({
            "relationship_role": "builds_from_source",
            "extractor": "manifest_bridges",
            "confidence_tier": "HIGH",
            "confidence": 0.8,
            "manifest_kind": "wasm_build",
            "wasm_producer": producer,
        });
        for (key, value) in extra.as_object().unwrap() {
            payload[key] = value.clone();
        }
        edge("CROSS_ARTIFACT", config, root, config, payload)
    }

    #[test]
    fn binds_javascript_to_go_and_assemblyscript_webassembly() {
        let path = temp_db("wasm-producers");
        let mut store = GraphStore::open(&path).expect("open");
        let go = "gowasm/main.go";
        let tiny = "tinygo/main.go";
        let asc = "as/assembly/index.ts";
        store
            .store_file_nodes_edges(
                go,
                &[
                    node("File", go, go, "go", json!({})),
                    node(
                        "Function",
                        "fastSum",
                        go,
                        "go",
                        json!({"ffi_exports": [{"abi": "js_global", "kind": "function", "name": "goFastSum"}]}),
                    ),
                ],
                &[],
                "",
                0,
            )
            .expect("store go");
        store
            .store_file_nodes_edges(
                tiny,
                &[
                    node("File", tiny, tiny, "go", json!({})),
                    node(
                        "Function",
                        "mul",
                        tiny,
                        "go",
                        json!({"ffi_export": {"abi": "c", "kind": "function", "name": "mul"}}),
                    ),
                ],
                &[],
                "",
                0,
            )
            .expect("store tinygo");
        store
            .store_file_nodes_edges(
                asc,
                &[
                    node("File", asc, asc, "typescript", json!({})),
                    node(
                        "Function",
                        "asSum",
                        asc,
                        "typescript",
                        json!({"exported": true}),
                    ),
                    node("Function", "helper", asc, "typescript", json!({})),
                ],
                &[],
                "",
                0,
            )
            .expect("store assemblyscript");
        store
            .replace_manifest_bridges(
                "manifest_bridges",
                &[],
                &[
                    wasm_build_edge(
                        "web/package.json",
                        go,
                        "go",
                        json!({"wasm_outputs": ["web/public/go.wasm"], "export_dir": "gowasm"}),
                    ),
                    wasm_build_edge(
                        "web/package.json",
                        tiny,
                        "tinygo",
                        json!({"wasm_outputs": ["web/public/tiny.wasm"], "export_dir": "tinygo"}),
                    ),
                    wasm_build_edge(
                        "as/asconfig.json",
                        asc,
                        "assemblyscript",
                        json!({"wasm_outputs": ["as/build/release.wasm"], "entry_files": [asc]}),
                    ),
                ],
            )
            .expect("manifest bridges");
        let web = "web/src/app.ts";
        let loader = json!({
            "relationship_role": "loads_wasm_module",
            "bridge_kind": "wasm",
            "confidence_tier": "HIGH",
            "confidence": 0.8,
        });
        store
            .store_file_nodes_edges(
                web,
                &[
                    node("File", web, web, "typescript", json!({})),
                    node("Function", "load", web, "typescript", json!({})),
                    node("Function", "useAll", web, "typescript", json!({})),
                    node(
                        "Function",
                        "goFastSum",
                        web,
                        "typescript",
                        json!({"ambient": true, "declaration_only": true}),
                    ),
                ],
                &[
                    edge(
                        "CROSS_ARTIFACT",
                        "web/src/app.ts::load",
                        "tiny.wasm?v=1",
                        web,
                        loader,
                    ),
                    edge(
                        "IMPORTS_FROM",
                        web,
                        "../../as/build/release.js",
                        web,
                        json!({}),
                    ),
                    edge(
                        "CALLS",
                        "web/src/app.ts::useAll",
                        "mul",
                        web,
                        json!({"receiver_unknown": true}),
                    ),
                    edge(
                        "CALLS",
                        "web/src/app.ts::useAll",
                        "../../as/build/release.js::asSum",
                        web,
                        json!({"unresolved_module": "../../as/build/release.js"}),
                    ),
                    // Not exported from the entry file.
                    edge(
                        "CALLS",
                        "web/src/app.ts::useAll",
                        "../../as/build/release.js::helper",
                        web,
                        json!({}),
                    ),
                    edge(
                        "CALLS",
                        "web/src/app.ts::useAll",
                        "web/src/app.ts::goFastSum",
                        web,
                        json!({}),
                    ),
                ],
                "",
                0,
            )
            .expect("store typescript");
        store.resolve_native_bindings().unwrap();
        assert_eq!(
            bridges(&store),
            vec![
                triple(web, asc, "loads_native_module"),
                triple("web/src/app.ts::load", tiny, "loads_native_module"),
                triple(
                    "web/src/app.ts::useAll",
                    "as/assembly/index.ts::asSum",
                    "calls_native_function"
                ),
                triple(
                    "web/src/app.ts::useAll",
                    "gowasm/main.go::fastSum",
                    "calls_native_function"
                ),
                triple(
                    "web/src/app.ts::useAll",
                    "tinygo/main.go::mul",
                    "calls_native_function"
                ),
            ]
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn wasm_output_paths_match_as_written_or_as_a_suffix() {
        let producer = |outputs: &[&str]| NativeCrate {
            root: String::new(),
            lib_name: String::new(),
            cdylib: false,
            python_module: None,
            js_packages: Vec::new(),
            wasm_out_dirs: Vec::new(),
            wasm_outputs: outputs.iter().map(|o| o.to_string()).collect(),
            scope: ExportScope::Files(Vec::new()),
            language: "go",
            build_system: "go".to_string(),
            node_addon: None,
            emscripten: None,
            uniffi: None,
        };
        let crates = [
            producer(&["web/public/go.wasm"]),
            producer(&["as/build/release.wasm"]),
        ];
        assert_eq!(
            wasm_output_crate(&crates, "web/src/a.ts", "/go.wasm"),
            Some(0)
        );
        assert_eq!(
            wasm_output_crate(&crates, "web/src/a.ts", "go.wasm?v=3"),
            Some(0)
        );
        assert_eq!(
            wasm_output_crate(&crates, "web/src/a.ts", "../../as/build/release.wasm"),
            Some(1)
        );
        assert_eq!(
            wasm_output_crate(&crates, "web/src/a.ts", "other.wasm"),
            None
        );
        assert_eq!(
            js_crate_for(&crates, "web/src/a.ts", "../../as/build/release.js"),
            Some(1)
        );
    }
}
