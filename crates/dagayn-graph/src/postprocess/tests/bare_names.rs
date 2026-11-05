use super::*;

/// Issue #154: C# files in one namespace need no `using` between them.
#[test]
fn resolves_bare_call_via_shared_namespace() {
    let path = temp_db("bare-call-namespace");
    let mut store = GraphStore::open(&path).expect("open");
    store
        .store_file_nodes_edges(
            "Factory.cs",
            &[
                namespaced_file_node("Factory.cs", "Repro.Infra"),
                function_node("CreateCriteria", "Factory.cs"),
            ],
            &[],
            "",
            0,
        )
        .expect("store factory");
    // Same method name in another namespace must not win the resolution.
    store
        .store_file_nodes_edges(
            "Decoy.cs",
            &[
                namespaced_file_node("Decoy.cs", "Repro.Other"),
                function_node("CreateCriteria", "Decoy.cs"),
            ],
            &[],
            "",
            0,
        )
        .expect("store decoy");
    store
        .store_file_nodes_edges(
            "Broker.cs",
            &[
                namespaced_file_node("Broker.cs", "Repro.Infra"),
                function_node("Resolve", "Broker.cs"),
            ],
            &[EdgeInput {
                kind: "CALLS".to_string(),
                source: "Broker.cs::Resolve".to_string(),
                target: "CreateCriteria".to_string(),
                file_path: "Broker.cs".to_string(),
                line: 2,
                extra: json!({}),
            }],
            "",
            0,
        )
        .expect("store broker");
    assert_eq!(store.resolve_bare_call_targets().unwrap(), 1);
    let target: String = store
        .conn
        .query_row(
            "SELECT target_qualified FROM edges WHERE kind='CALLS'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target, "Factory.cs::CreateCriteria");
    let _ = std::fs::remove_file(path);
}

/// The files of a Go package share its import path as their namespace, so a
/// plain call binds to a function of another file of the package. A call
/// through a package qualifier or a value (`huff0.Compress()`, `w.Flush()`)
/// is no plain call, and a plain call never names a method. A call through
/// an import binds to that package's top-level function, never to one of
/// its test files.
#[test]
fn go_plain_calls_bind_within_their_package_only() {
    let path = temp_db("bare-call-go-package");
    let mut store = GraphStore::open(&path).expect("open");
    let calls = |file: &str, caller: &str, list: &[(&str, i64, Value)]| -> Vec<EdgeInput> {
        list.iter()
            .map(|(target, line, extra)| EdgeInput {
                extra: extra.clone(),
                ..edge("CALLS", &format!("{file}::{caller}"), target, file, *line)
            })
            .collect()
    };
    let files: Vec<(&str, &str, Vec<NodeInput>, Vec<EdgeInput>)> = vec![
        (
            "zstd/enc.go",
            "example.com/m/zstd",
            vec![
                function_node("encode", "zstd/enc.go"),
                function_node("Compress", "zstd/enc.go"),
                method_node("Reset", "zstd/enc.go", "Encoder"),
                method_node("Flush", "zstd/enc.go", "Encoder"),
                method_node("Close", "zstd/enc.go", "Encoder"),
                class_node("Scratch", "zstd/enc.go"),
                method_node("prepare", "zstd/enc.go", "Scratch"),
            ],
            vec![],
        ),
        (
            "huff0/h.go",
            "example.com/m/huff0",
            vec![
                function_node("Compress", "huff0/h.go"),
                function_node("encode", "huff0/h.go"),
                // Another package's type of the same name.
                class_node("Scratch", "huff0/h.go"),
                method_node("prepare", "huff0/h.go", "Scratch"),
            ],
            vec![],
        ),
        (
            "zstd/dec.go",
            "example.com/m/zstd",
            vec![
                function_node("run", "zstd/dec.go"),
                method_node("Close", "zstd/dec.go", "writer"),
                method_node("Sum", "zstd/dec.go", "digest"),
            ],
            calls(
                "zstd/dec.go",
                "run",
                &[
                    ("encode", 2, json!({})),
                    ("Reset", 3, json!({})),
                    (
                        "Compress",
                        4,
                        json!({"receiver": "huff0", "import_path": "example.com/m/huff0"}),
                    ),
                    ("Flush", 5, json!({"receiver_unknown": true})),
                    // A value of unknown type is never bound by name, even
                    // to the file's own `Close` or the package's only `Sum`.
                    ("Close", 6, json!({"receiver_unknown": true})),
                    ("Sum", 7, json!({"receiver_unknown": true})),
                    // `s *Scratch`: the package's `Scratch`.
                    ("prepare", 8, json!({"receiver_type": "Scratch"})),
                ],
            ),
        ),
        (
            "zstd/enc_test.go",
            "example.com/m/zstd",
            vec![function_node("newFixture", "zstd/enc_test.go")],
            vec![],
        ),
        (
            "zstd/x_test.go",
            "example.com/m/zstd_test",
            vec![function_node("TestX", "zstd/x_test.go")],
            calls(
                "zstd/x_test.go",
                "TestX",
                &[
                    ("encode", 2, json!({})),
                    (
                        "Compress",
                        3,
                        json!({"receiver": "zstd", "import_path": "example.com/m/zstd"}),
                    ),
                    // Only the package's tests see its test files.
                    (
                        "newFixture",
                        4,
                        json!({"receiver": "zstd", "import_path": "example.com/m/zstd"}),
                    ),
                ],
            ),
        ),
    ];
    for (file, namespace, mut nodes, edges) in files {
        nodes.insert(0, namespaced_file_node(file, namespace));
        store
            .store_file_nodes_edges(file, &nodes, &edges, "", 0)
            .expect("store");
    }
    store.resolve_bare_call_targets().unwrap();
    let mut stmt = store
        .conn
        .prepare("SELECT file_path, line, target_qualified FROM edges WHERE kind='CALLS' ORDER BY file_path, line")
        .unwrap();
    let targets = stmt
        .query_map([], |row| <(String, i64, String)>::try_from(row))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        targets,
        vec![
            (
                "zstd/dec.go".to_string(),
                2,
                "zstd/enc.go::encode".to_string()
            ),
            ("zstd/dec.go".to_string(), 3, "Reset".to_string()),
            (
                "zstd/dec.go".to_string(),
                4,
                "huff0/h.go::Compress".to_string()
            ),
            ("zstd/dec.go".to_string(), 5, "Flush".to_string()),
            ("zstd/dec.go".to_string(), 6, "Close".to_string()),
            ("zstd/dec.go".to_string(), 7, "Sum".to_string()),
            (
                "zstd/dec.go".to_string(),
                8,
                "zstd/enc.go::Scratch.prepare".to_string()
            ),
            // The external test package sees none of the package's names.
            ("zstd/x_test.go".to_string(), 2, "encode".to_string()),
            (
                "zstd/x_test.go".to_string(),
                3,
                "zstd/enc.go::Compress".to_string()
            ),
            ("zstd/x_test.go".to_string(), 4, "newFixture".to_string()),
        ]
    );
    drop(stmt);
    // `callers_of`'s name fallback: the call through `huff0` is a caller of
    // huff0's `Compress`, though zstd's files cannot see it, and none of
    // zstd's own `Compress`, which only the call through `zstd` names.
    let callers = |qualified: &str| {
        let node = store.get_node(qualified).unwrap().expect("node");
        store
            .bare_name_edges(&node, "CALLS")
            .unwrap()
            .into_iter()
            .map(|edge| (edge.file_path, edge.line))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        callers("huff0/h.go::Compress"),
        vec![("zstd/dec.go".to_string(), 4)]
    );
    assert_eq!(
        callers("zstd/enc.go::Compress"),
        vec![("zstd/x_test.go".to_string(), 3)]
    );
    let _ = std::fs::remove_file(path);
}

/// A namespace is shared only within one language family: a PHP file and a
/// C# file that both declare `Shop` cannot name each other's symbols, while
/// Kotlin and Java in one package can.
#[test]
fn a_shared_namespace_does_not_cross_language_families() {
    let path = temp_db("bare-call-namespace-family");
    let mut store = GraphStore::open(&path).expect("open");
    let files: [(&str, &str, Vec<NodeInput>); 4] = [
        (
            "Cart.cs",
            "Shop",
            vec![
                class_node("Base", "Cart.cs"),
                function_node("Total", "Cart.cs"),
            ],
        ),
        ("run.php", "Shop", vec![function_node("run", "run.php")]),
        (
            "Util.java",
            "app",
            vec![function_node("helper", "Util.java")],
        ),
        (
            "Main.kt",
            "app",
            vec![
                function_node("main", "Main.kt"),
                class_node("Impl", "Main.kt"),
            ],
        ),
    ];
    for (file, namespace, nodes) in files {
        let mut all = vec![namespaced_file_node(file, namespace)];
        all.extend(nodes);
        let edges = match file {
            "run.php" => vec![
                edge("CALLS", "run.php::run", "Total", file, 2),
                edge("INHERITS", "run.php::run", "Base", file, 3),
            ],
            "Main.kt" => vec![edge("CALLS", "Main.kt::main", "helper", file, 2)],
            _ => vec![],
        };
        store
            .store_file_nodes_edges(file, &all, &edges, "", 0)
            .expect("store");
    }
    store.resolve_bare_call_targets().unwrap();
    store.resolve_bare_inheritance_targets().unwrap();
    let target = |source: &str, kind: &str| -> String {
        store
            .conn
            .query_row(
                "SELECT target_qualified FROM edges WHERE source_qualified = ? AND kind = ?",
                [source, kind],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(target("run.php::run", "CALLS"), "Total");
    assert_eq!(target("run.php::run", "INHERITS"), "Base");
    assert_eq!(target("Main.kt::main", "CALLS"), "Util.java::helper");
    let _ = std::fs::remove_file(path);
}

#[test]
fn resolves_bare_call_via_imported_namespace() {
    let path = temp_db("bare-call-imported-namespace");
    let mut store = GraphStore::open(&path).expect("open");
    store
        .store_file_nodes_edges(
            "Broker.php",
            &[
                namespaced_file_node("Broker.php", "App\\Util"),
                function_node("phpBuild", "Broker.php"),
            ],
            &[],
            "",
            0,
        )
        .expect("store broker");
    store
        .store_file_nodes_edges(
            "Factory.php",
            &[
                namespaced_file_node("Factory.php", "App\\Infra"),
                function_node("make", "Factory.php"),
            ],
            &[
                EdgeInput {
                    kind: "IMPORTS_FROM".to_string(),
                    source: "Factory.php".to_string(),
                    // `use App\Util\Broker` names a symbol in the namespace.
                    target: "App\\Util\\Broker".to_string(),
                    file_path: "Factory.php".to_string(),
                    line: 1,
                    extra: json!({}),
                },
                EdgeInput {
                    kind: "CALLS".to_string(),
                    source: "Factory.php::make".to_string(),
                    target: "phpBuild".to_string(),
                    file_path: "Factory.php".to_string(),
                    line: 2,
                    extra: json!({}),
                },
            ],
            "",
            0,
        )
        .expect("store factory");
    assert_eq!(store.resolve_bare_call_targets().unwrap(), 1);
    let target: String = store
        .conn
        .query_row(
            "SELECT target_qualified FROM edges WHERE kind='CALLS'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target, "Broker.php::phpBuild");
    let _ = std::fs::remove_file(path);
}

/// A C++ definition lives in a `.cpp` nobody includes; only its class
/// declaration is reachable from the caller's header.
#[test]
fn resolves_bare_call_via_declaring_class() {
    let path = temp_db("bare-call-declaring-class");
    let mut store = GraphStore::open(&path).expect("open");
    store
        .store_file_nodes_edges(
            "include/factory.hpp",
            &[
                file_node("include/factory.hpp"),
                class_node("Factory", "include/factory.hpp"),
            ],
            &[],
            "",
            0,
        )
        .expect("store header");
    store
        .store_file_nodes_edges(
            "src/factory.cpp",
            &[
                file_node("src/factory.cpp"),
                method_node("createAllowed", "src/factory.cpp", "Factory"),
            ],
            &[EdgeInput {
                kind: "IMPORTS_FROM".to_string(),
                source: "src/factory.cpp".to_string(),
                target: "include/factory.hpp".to_string(),
                file_path: "src/factory.cpp".to_string(),
                line: 1,
                extra: json!({}),
            }],
            "",
            0,
        )
        .expect("store definition");
    // An unrelated class with a same-named method must not win.
    store
        .store_file_nodes_edges(
            "src/other.cpp",
            &[
                file_node("src/other.cpp"),
                class_node("Unrelated", "src/other.cpp"),
                method_node("createAllowed", "src/other.cpp", "Unrelated"),
            ],
            &[],
            "",
            0,
        )
        .expect("store other");
    store
        .store_file_nodes_edges(
            "src/broker.cpp",
            &[
                file_node("src/broker.cpp"),
                function_node("use", "src/broker.cpp"),
            ],
            &[
                EdgeInput {
                    kind: "IMPORTS_FROM".to_string(),
                    source: "src/broker.cpp".to_string(),
                    target: "include/factory.hpp".to_string(),
                    file_path: "src/broker.cpp".to_string(),
                    line: 1,
                    extra: json!({}),
                },
                EdgeInput {
                    kind: "CALLS".to_string(),
                    source: "src/broker.cpp::use".to_string(),
                    target: "createAllowed".to_string(),
                    file_path: "src/broker.cpp".to_string(),
                    line: 3,
                    extra: json!({}),
                },
            ],
            "",
            0,
        )
        .expect("store caller");
    assert_eq!(store.resolve_bare_call_targets().unwrap(), 1);
    let target: String = store
        .conn
        .query_row(
            "SELECT target_qualified FROM edges WHERE kind='CALLS'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target, "src/factory.cpp::Factory.createAllowed");
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_type_call_binds_to_the_same_named_type_the_caller_sees() {
    let path = temp_db("bare-call-same-named-types");
    let mut store = GraphStore::open(&path).expect("open");
    // Two crates each declare a `Namer` with a `new`.
    for file in ["crates/a/src/namer.rs", "crates/b/src/lib.rs"] {
        store
            .store_file_nodes_edges(
                file,
                &[
                    file_node(file),
                    class_node("Namer", file),
                    method_node("new", file, "Namer"),
                ],
                &[],
                "",
                0,
            )
            .expect("store type");
    }
    let call = |caller: &str| EdgeInput {
        kind: "CALLS".to_string(),
        source: format!("{caller}::run"),
        target: "new".to_string(),
        file_path: caller.to_string(),
        line: 3,
        extra: json!({"receiver_type": "Namer"}),
    };
    // `use namer::Namer;` in crate a, and a file that imports neither.
    store
        .store_file_nodes_edges(
            "crates/a/src/lib.rs",
            &[
                file_node("crates/a/src/lib.rs"),
                function_node("run", "crates/a/src/lib.rs"),
            ],
            &[
                edge(
                    "IMPORTS_FROM",
                    "crates/a/src/lib.rs",
                    "crates/a/src/namer.rs",
                    "crates/a/src/lib.rs",
                    1,
                ),
                call("crates/a/src/lib.rs"),
            ],
            "",
            0,
        )
        .expect("store caller");
    store
        .store_file_nodes_edges(
            "crates/c/src/lib.rs",
            &[
                file_node("crates/c/src/lib.rs"),
                function_node("run", "crates/c/src/lib.rs"),
            ],
            &[call("crates/c/src/lib.rs")],
            "",
            0,
        )
        .expect("store blind caller");
    store.resolve_bare_call_targets().unwrap();
    let targets: Vec<(String, String)> = store
        .conn
        .prepare(
            "SELECT source_qualified, target_qualified FROM edges WHERE kind='CALLS' ORDER BY 1",
        )
        .unwrap()
        .query_map([], |row| <(String, String)>::try_from(row))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(
        targets,
        [
            (
                "crates/a/src/lib.rs::run".to_string(),
                "crates/a/src/namer.rs::Namer.new".to_string()
            ),
            ("crates/c/src/lib.rs::run".to_string(), "new".to_string()),
        ]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn resolves_bare_call_via_import() {
    let path = temp_db("bare-call");
    let mut store = GraphStore::open(&path).expect("open");
    store
        .store_file_nodes_edges(
            "a.py",
            &[file_node("a.py"), function_node("helper", "a.py")],
            &[],
            "",
            0,
        )
        .expect("store a");
    store
        .store_file_nodes_edges(
            "b.py",
            &[file_node("b.py"), function_node("run", "b.py")],
            &[
                EdgeInput {
                    kind: "IMPORTS_FROM".to_string(),
                    source: "b.py".to_string(),
                    target: "a.py".to_string(),
                    file_path: "b.py".to_string(),
                    line: 1,
                    extra: json!({}),
                },
                EdgeInput {
                    kind: "CALLS".to_string(),
                    source: "b.py::run".to_string(),
                    target: "helper".to_string(),
                    file_path: "b.py".to_string(),
                    line: 2,
                    extra: json!({}),
                },
            ],
            "",
            0,
        )
        .expect("store b");
    assert_eq!(store.resolve_bare_call_targets().unwrap(), 1);
    let target: String = store
        .conn
        .query_row(
            "SELECT target_qualified FROM edges WHERE kind='CALLS'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target, "a.py::helper");
    let _ = std::fs::remove_file(path);
}

#[test]
fn external_package_calls_keep_their_package_target() {
    // `App.test.tsx` imports `ClassComp.tsx` (which declares
    // `ClassComp.render`) and calls `render` from
    // `@testing-library/react`. The extractor qualifies the external call,
    // so bare-name resolution cannot steal it; only the bare call binds.
    let path = temp_db("external-package");
    let mut store = GraphStore::open(&path).expect("open");
    store
        .store_file_nodes_edges(
            "src/ClassComp.tsx",
            &[
                file_node("src/ClassComp.tsx"),
                class_node("ClassComp", "src/ClassComp.tsx"),
                method_node("render", "src/ClassComp.tsx", "ClassComp"),
            ],
            &[],
            "",
            0,
        )
        .expect("store component");
    let test = "src/App.test.tsx::renders";
    let file = "src/App.test.tsx";
    let external = EdgeInput {
        extra: json!({"external": true, "external_package": "@testing-library/react"}),
        ..edge("CALLS", test, "@testing-library/react::render", file, 4)
    };
    store
        .store_file_nodes_edges(
            file,
            &[file_node(file), test_node("renders", file)],
            &[
                edge("IMPORTS_FROM", file, "@testing-library/react", file, 1),
                edge("IMPORTS_FROM", file, "src/ClassComp.tsx", file, 2),
                external,
                edge("CALLS", test, "render", file, 5),
            ],
            "",
            0,
        )
        .expect("store test");
    assert_eq!(store.resolve_bare_call_targets().unwrap(), 1);
    store.demote_unresolved_endpoint_edges().unwrap();
    let mut stmt = store
        .conn
        .prepare(
            "SELECT line, target_qualified, confidence_tier, \
             json_extract(extra, '$.external_package') \
             FROM edges WHERE kind = 'CALLS' ORDER BY line",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            <(i64, String, String, Option<String>)>::try_from(row)
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (
                4,
                "@testing-library/react::render".to_string(),
                "LOW".to_string(),
                Some("@testing-library/react".to_string()),
            ),
            (
                5,
                "src/ClassComp.tsx::ClassComp.render".to_string(),
                "MEDIUM".to_string(),
                None,
            ),
        ]
    );
    let _ = std::fs::remove_file(path);
}

/// dagayn's own shape: a Python wrapper class shares its name with a Rust
/// struct. Importing the wrapper must not make the struct's methods visible
/// to a builtin `open(...)` call.
#[test]
fn class_declared_in_another_language_does_not_expose_methods() {
    let path = temp_db("bare-call-cross-language-class");
    let mut store = GraphStore::open(&path).expect("open");
    store
        .store_file_nodes_edges(
            "pkg/graph.py",
            &[
                file_node("pkg/graph.py"),
                class_node("GraphStore", "pkg/graph.py"),
            ],
            &[],
            "",
            0,
        )
        .expect("store python wrapper");
    store
        .store_file_nodes_edges(
            "src/core.rs",
            &[
                NodeInput {
                    language: "rust".to_string(),
                    ..file_node("src/core.rs")
                },
                NodeInput {
                    language: "rust".to_string(),
                    ..class_node("GraphStore", "src/core.rs")
                },
                NodeInput {
                    language: "rust".to_string(),
                    ..method_node("open", "src/core.rs", "GraphStore")
                },
            ],
            &[],
            "",
            0,
        )
        .expect("store rust struct");
    store
        .store_file_nodes_edges(
            "pkg/runner.py",
            &[
                file_node("pkg/runner.py"),
                function_node("load", "pkg/runner.py"),
            ],
            &[
                edge(
                    "IMPORTS_FROM",
                    "pkg/runner.py",
                    "pkg/graph.py",
                    "pkg/runner.py",
                    1,
                ),
                edge("CALLS", "pkg/runner.py::load", "open", "pkg/runner.py", 3),
            ],
            "",
            0,
        )
        .expect("store caller");
    assert_eq!(store.resolve_bare_call_targets().unwrap(), 0);
    let _ = std::fs::remove_file(path);
}

#[test]
fn standard_library_calls_keep_their_package_target() {
    // `app/main.py` imports `app/shell.py`, which declares a function named
    // `subprocess`. The standard library's `subprocess.run(...)` targets the
    // package `subprocess` (marked external) and must not bind to it; only
    // the bare call does.
    let path = temp_db("stdlib-package");
    let mut store = GraphStore::open(&path).expect("open");
    store
        .store_file_nodes_edges(
            "app/shell.py",
            &[
                file_node("app/shell.py"),
                function_node("subprocess", "app/shell.py"),
            ],
            &[],
            "",
            0,
        )
        .expect("store shell");
    let file = "app/main.py";
    let caller = "app/main.py::main";
    let stdlib = EdgeInput {
        extra: json!({
            "external": true,
            "external_package": "subprocess",
            "external_symbol": "subprocess.run",
        }),
        ..edge("CALLS", caller, "subprocess", file, 4)
    };
    store
        .store_file_nodes_edges(
            file,
            &[file_node(file), function_node("main", file)],
            &[
                edge("IMPORTS_FROM", file, "app/shell.py", file, 1),
                stdlib,
                edge("CALLS", caller, "subprocess", file, 5),
            ],
            "",
            0,
        )
        .expect("store main");
    assert_eq!(store.resolve_bare_call_targets().unwrap(), 1);
    let mut stmt = store
        .conn
        .prepare("SELECT line, target_qualified FROM edges WHERE kind = 'CALLS' ORDER BY line")
        .unwrap();
    let rows = stmt
        .query_map([], |row| <(i64, String)>::try_from(row))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (4, "subprocess".to_string()),
            (5, "app/shell.py::subprocess".to_string()),
        ]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_package_init_lends_what_it_imports() {
    // `app.py` imports `pkg/__init__.py`, which imports `pkg/helpers.py`:
    // the bare `helper()` of `app.py` sees `pkg/helpers.py::helper`.
    let path = temp_db("init-reexports");
    let mut store = GraphStore::open(&path).expect("open");
    for (file, nodes, edges) in [
        (
            "pkg/helpers.py",
            vec![
                file_node("pkg/helpers.py"),
                function_node("helper", "pkg/helpers.py"),
            ],
            vec![],
        ),
        (
            "other/helpers.py",
            vec![
                file_node("other/helpers.py"),
                function_node("helper", "other/helpers.py"),
            ],
            vec![],
        ),
        (
            "pkg/__init__.py",
            vec![file_node("pkg/__init__.py")],
            vec![edge(
                "IMPORTS_FROM",
                "pkg/__init__.py",
                "pkg/helpers.py",
                "pkg/__init__.py",
                1,
            )],
        ),
        (
            "app.py",
            vec![file_node("app.py"), function_node("main", "app.py")],
            vec![
                edge("IMPORTS_FROM", "app.py", "pkg/__init__.py", "app.py", 1),
                edge("CALLS", "app.py::main", "helper", "app.py", 3),
            ],
        ),
    ] {
        store
            .store_file_nodes_edges(file, &nodes, &edges, "", 0)
            .expect("store");
    }
    assert_eq!(store.resolve_bare_call_targets().unwrap(), 1);
    let target: String = store
        .conn
        .query_row(
            "SELECT target_qualified FROM edges WHERE kind = 'CALLS'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target, "pkg/helpers.py::helper");
    let _ = std::fs::remove_file(path);
}

#[test]
fn unknown_receivers_do_not_bind_to_the_callers_own_class() {
    // `StatusBar.dispose` calls `this.item.dispose()`: the item's, not the
    // class's own `dispose`. `reader.getStats()` still binds to the
    // imported `Reader.getStats`.
    let path = temp_db("unknown-receiver-own-class");
    let mut store = GraphStore::open(&path).expect("open");
    let unknown = |target: &str, line: i64| EdgeInput {
        extra: json!({"receiver_unknown": true}),
        ..edge(
            "CALLS",
            "src/bar.ts::StatusBar.dispose",
            target,
            "src/bar.ts",
            line,
        )
    };
    for (file, nodes, edges) in [
        (
            "src/reader.ts",
            vec![
                file_node("src/reader.ts"),
                class_node("Reader", "src/reader.ts"),
                method_node("getStats", "src/reader.ts", "Reader"),
            ],
            vec![],
        ),
        (
            "src/bar.ts",
            vec![
                file_node("src/bar.ts"),
                class_node("StatusBar", "src/bar.ts"),
                method_node("dispose", "src/bar.ts", "StatusBar"),
            ],
            vec![
                edge(
                    "IMPORTS_FROM",
                    "src/bar.ts",
                    "src/reader.ts",
                    "src/bar.ts",
                    1,
                ),
                unknown("dispose", 2),
                unknown("getStats", 3),
            ],
        ),
    ] {
        store
            .store_file_nodes_edges(file, &nodes, &edges, "", 0)
            .expect("store");
    }
    store.resolve_bare_call_targets().unwrap();
    let rows = store
        .conn
        .prepare("SELECT line, target_qualified FROM edges WHERE kind = 'CALLS' ORDER BY line")
        .unwrap()
        .query_map([], |row| <(i64, String)>::try_from(row))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (2, "dispose".to_string()),
            (3, "src/reader.ts::Reader.getStats".to_string()),
        ]
    );
    let _ = std::fs::remove_file(path);
}
