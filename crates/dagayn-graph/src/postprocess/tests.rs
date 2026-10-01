use crate::*;

use serde_json::{Value, json};

use std::path::PathBuf;

fn temp_db(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "dagayn-postprocess-{}-{}.db",
        name,
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn function_node(name: &str, file_path: &str) -> NodeInput {
    NodeInput {
        kind: "Function".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end: 2,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    }
}

fn class_node(name: &str, file_path: &str) -> NodeInput {
    NodeInput {
        kind: "Class".to_string(),
        ..function_node(name, file_path)
    }
}

fn method_node(name: &str, file_path: &str, owner: &str) -> NodeInput {
    NodeInput {
        parent_name: Some(owner.to_string()),
        ..function_node(name, file_path)
    }
}

fn file_node(file_path: &str) -> NodeInput {
    NodeInput {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end: 1,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    }
}

fn edge(kind: &str, source: &str, target: &str, file_path: &str, line: i64) -> EdgeInput {
    EdgeInput {
        kind: kind.to_string(),
        source: source.to_string(),
        target: target.to_string(),
        file_path: file_path.to_string(),
        line,
        extra: json!({}),
    }
}

fn test_node(name: &str, file_path: &str) -> NodeInput {
    NodeInput {
        kind: "Test".to_string(),
        is_test: true,
        ..function_node(name, file_path)
    }
}

fn tested_by_rows(store: &GraphStore) -> Vec<(String, String, i64, String)> {
    let mut stmt = store
        .conn
        .prepare(
            "SELECT source_qualified, target_qualified, line, confidence_tier FROM edges \
             WHERE kind = 'TESTED_BY' ORDER BY line, source_qualified",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })
    .unwrap()
    .collect::<std::result::Result<Vec<_>, _>>()
    .unwrap()
}

/// A test file importing `a.py`: `test_run` calls `helper` (resolvable),
/// `missing` (not), and both `helper` / `other` on line 4, as parsers
/// emit them: bare `CALLS` plus the mirrored bare `TESTED_BY`.
fn store_test_file(store: &mut GraphStore, test_edges: &[EdgeInput]) {
    store
        .store_file_nodes_edges(
            "a.py",
            &[
                file_node("a.py"),
                function_node("helper", "a.py"),
                function_node("other", "a.py"),
            ],
            &[],
            "",
            0,
        )
        .expect("store a");
    let mut edges = vec![edge(
        "IMPORTS_FROM",
        "tests/test_b.py",
        "a.py",
        "tests/test_b.py",
        1,
    )];
    edges.extend_from_slice(test_edges);
    store
        .store_file_nodes_edges(
            "tests/test_b.py",
            &[
                file_node("tests/test_b.py"),
                test_node("test_run", "tests/test_b.py"),
            ],
            &edges,
            "",
            0,
        )
        .expect("store test");
}

#[test]
fn resolving_bare_calls_moves_tested_by_to_the_resolved_target() {
    let path = temp_db("tested-by-sync");
    let mut store = GraphStore::open(&path).expect("open");
    let test = "tests/test_b.py::test_run";
    let file = "tests/test_b.py";
    store_test_file(
        &mut store,
        &[
            edge("CALLS", test, "helper", file, 2),
            edge("TESTED_BY", "helper", test, file, 2),
            edge("CALLS", test, "missing", file, 3),
            edge("TESTED_BY", "missing", test, file, 3),
            edge("CALLS", test, "other", file, 4),
            edge("TESTED_BY", "other", test, file, 4),
        ],
    );
    assert_eq!(store.resolve_bare_call_targets().unwrap(), 2);
    assert_eq!(
        tested_by_rows(&store),
        vec![
            (
                "a.py::helper".to_string(),
                test.to_string(),
                2,
                "HIGH".to_string()
            ),
            // `missing` stays unresolved: its TESTED_BY names no node.
            (
                "a.py::other".to_string(),
                test.to_string(),
                4,
                "HIGH".to_string()
            ),
        ]
    );
    // Idempotent: a second run changes nothing.
    assert_eq!(store.resolve_bare_call_targets().unwrap(), 0);
    assert_eq!(tested_by_rows(&store).len(), 2);
    let _ = std::fs::remove_file(path);
}

#[test]
fn tested_by_follows_calls_resolved_by_an_earlier_run() {
    // Graphs built before the sync: the CALLS edge is already resolved,
    // the TESTED_BY edge is still bare. A rewrite that would duplicate an
    // edge drops the bare copy, and a TESTED_BY of a symbol that is not a
    // node (`fetch`, `ext.py::Remote.fetch` of a file not in the graph)
    // is dropped.
    let path = temp_db("tested-by-stale");
    let mut store = GraphStore::open(&path).expect("open");
    let test = "tests/test_b.py::test_run";
    let file = "tests/test_b.py";
    store_test_file(
        &mut store,
        &[
            edge("CALLS", test, "a.py::helper", file, 2),
            edge("TESTED_BY", "helper", test, file, 2),
            edge("CALLS", test, "a.py::other", file, 3),
            edge("CALLS", test, "other", file, 3),
            edge("TESTED_BY", "a.py::other", test, file, 3),
            edge("TESTED_BY", "other", test, file, 3),
            edge("CALLS", test, "a.py::helper", file, 5),
            edge("TESTED_BY", "a.py::helper", test, file, 5),
            edge("TESTED_BY", "helper", test, file, 5),
            edge("CALLS", test, "ext.py::Remote.fetch", file, 6),
            edge("CALLS", test, "fetch", file, 6),
            edge("TESTED_BY", "ext.py::Remote.fetch", test, file, 6),
            edge("TESTED_BY", "fetch", test, file, 6),
        ],
    );
    store.resolve_bare_call_targets().unwrap();
    let rows = tested_by_rows(&store)
        .into_iter()
        .map(|(source, _, line, _)| (source, line))
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        vec![
            ("a.py::helper".to_string(), 2),
            ("a.py::other".to_string(), 3),
            ("a.py::helper".to_string(), 5),
        ]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn demotes_missing_call_targets() {
    let path = temp_db("demote");
    let mut store = GraphStore::open(&path).expect("open");
    store
        .store_file_nodes_edges(
            "app.py",
            &[file_node("app.py"), function_node("main", "app.py")],
            &[EdgeInput {
                kind: "CALLS".to_string(),
                source: "app.py::main".to_string(),
                target: "missing".to_string(),
                file_path: "app.py".to_string(),
                line: 1,
                extra: json!({}),
            }],
            "",
            0,
        )
        .expect("store");
    assert_eq!(store.demote_unresolved_endpoint_edges().unwrap(), 1);
    let tier: String = store
        .conn
        .query_row(
            "SELECT confidence_tier FROM edges WHERE kind='CALLS'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tier, "LOW");
    let _ = std::fs::remove_file(path);
}

#[test]
fn resolves_unique_terraform_handler() {
    let path = temp_db("terraform");
    let mut store = GraphStore::open(&path).expect("open");
    store
        .store_file_nodes_edges(
            "app/hello.py",
            &[
                file_node("app/hello.py"),
                function_node("main", "app/hello.py"),
            ],
            &[],
            "",
            0,
        )
        .expect("store");
    let extra = json!({
        "source_language": "terraform",
        "evidence_source": "handler",
        "relationship_role": "maps_entrypoint",
        "original_symbol_name": "hello.main",
    });
    store
        .conn
        .execute(
            "INSERT INTO edges (kind, source_qualified, target_qualified, target_name,
                 file_path, line, extra, confidence, confidence_tier, updated_at)
             VALUES ('CROSS_ARTIFACT', 'infra/main.tf', '<unresolved:hello.main>',
                     'hello.main', 'infra/main.tf', 1, ?, 0.8, 'HIGH', 0)",
            [extra.to_string()],
        )
        .unwrap();
    assert_eq!(store.resolve_terraform_artifact_refs().unwrap(), (1, 0));
    let target: String = store
        .conn
        .query_row(
            "SELECT target_qualified FROM edges WHERE kind='CROSS_ARTIFACT'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target, "app/hello.py::main");
    let _ = std::fs::remove_file(path);
}

fn terraform_node(kind: &str, name: &str, file_path: &str) -> NodeInput {
    NodeInput {
        kind: kind.to_string(),
        language: "terraform".to_string(),
        ..function_node(name, file_path)
    }
}

fn terraform_file_node(file_path: &str) -> NodeInput {
    NodeInput {
        language: "terraform".to_string(),
        ..file_node(file_path)
    }
}

fn reference_edge(source: &str, target: &str, file_path: &str, line: i64) -> EdgeInput {
    EdgeInput {
        kind: "REFERENCES".to_string(),
        source: source.to_string(),
        target: target.to_string(),
        file_path: file_path.to_string(),
        line,
        extra: json!({}),
    }
}

#[test]
fn resolves_terraform_references_within_module_directory() {
    let path = temp_db("terraform-module-refs");
    let mut store = GraphStore::open(&path).expect("open");
    store
        .store_file_nodes_edges(
            "infra/variables.tf",
            &[
                terraform_file_node("infra/variables.tf"),
                terraform_node("Function", "var.region", "infra/variables.tf"),
                terraform_node("Function", "var.dup", "infra/variables.tf"),
            ],
            &[],
            "",
            0,
        )
        .expect("store variables");
    store
        .store_file_nodes_edges(
            "infra/override.tf",
            &[
                terraform_file_node("infra/override.tf"),
                terraform_node("Function", "var.dup", "infra/override.tf"),
            ],
            &[],
            "",
            0,
        )
        .expect("store override");
    store
        .store_file_nodes_edges(
            "infra/modules/net/variables.tf",
            &[
                terraform_file_node("infra/modules/net/variables.tf"),
                terraform_node("Function", "var.cidr", "infra/modules/net/variables.tf"),
            ],
            &[],
            "",
            0,
        )
        .expect("store child module");
    let source = "infra/main.tf::resource.aws_vpc.main";
    store
        .store_file_nodes_edges(
            "infra/main.tf",
            &[
                terraform_file_node("infra/main.tf"),
                terraform_node("Class", "resource.aws_vpc.main", "infra/main.tf"),
            ],
            &[
                reference_edge(source, "var.region", "infra/main.tf", 1),
                // Declared twice in the module: ambiguous, stays bare.
                reference_edge(source, "var.dup", "infra/main.tf", 2),
                // Only declared in a child module directory: out of scope.
                reference_edge(source, "var.cidr", "infra/main.tf", 3),
                reference_edge(source, "var.missing", "infra/main.tf", 4),
            ],
            "",
            0,
        )
        .expect("store main");
    // A non-Terraform file with the same bare target is left alone.
    store
        .store_file_nodes_edges(
            "infra/app.py",
            &[
                file_node("infra/app.py"),
                function_node("main", "infra/app.py"),
            ],
            &[reference_edge(
                "infra/app.py::main",
                "var.region",
                "infra/app.py",
                1,
            )],
            "",
            0,
        )
        .expect("store python");

    assert_eq!(store.resolve_terraform_module_references().unwrap(), 1);
    let rows = {
        let mut stmt = store
            .conn
            .prepare(
                "SELECT file_path, line, target_qualified, confidence_tier FROM edges \
                 WHERE kind='REFERENCES' ORDER BY file_path, line",
            )
            .unwrap();
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap()
    };
    let targets = rows
        .iter()
        .map(|(file, line, target, _)| (file.as_str(), *line, target.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        targets,
        vec![
            ("infra/app.py", 1, "var.region"),
            ("infra/main.tf", 1, "infra/variables.tf::var.region"),
            ("infra/main.tf", 2, "var.dup"),
            ("infra/main.tf", 3, "var.cidr"),
            ("infra/main.tf", 4, "var.missing"),
        ]
    );
    assert_eq!(rows[1].3, "HIGH");
    // The resolved edge survives endpoint demotion; the bare ones do not.
    store.demote_unresolved_endpoint_edges().unwrap();
    let tier: String = store
        .conn
        .query_row(
            "SELECT confidence_tier FROM edges WHERE file_path='infra/main.tf' AND line=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tier, "HIGH");
    let _ = std::fs::remove_file(path);
}

fn namespaced_file_node(file_path: &str, namespace: &str) -> NodeInput {
    let mut node = file_node(file_path);
    node.extra = json!({"namespaces": [namespace]});
    node
}

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
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
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
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
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
fn standard_library_edges_keep_their_tier() {
    // A standard-library edge targets a package, never a node: it keeps the
    // tier its extractor gave it; any other edge to no node is demoted.
    let path = temp_db("stdlib-tier");
    let mut store = GraphStore::open(&path).expect("open");
    let file = "app/main.py";
    let caller = "app/main.py::main";
    let stdlib = |tier: &str, line: i64| EdgeInput {
        extra: json!({
            "external": true,
            "external_package": "subprocess",
            "stdlib": true,
            "confidence_tier": tier,
        }),
        ..edge("CALLS", caller, "subprocess", file, line)
    };
    store
        .store_file_nodes_edges(
            file,
            &[file_node(file), function_node("main", file)],
            &[
                stdlib("HIGH", 4),
                stdlib("MEDIUM", 5),
                edge("CALLS", caller, "missing", file, 6),
            ],
            "",
            0,
        )
        .expect("store main");
    store.demote_unresolved_endpoint_edges().unwrap();
    let mut stmt = store
        .conn
        .prepare("SELECT line, confidence_tier FROM edges WHERE kind = 'CALLS' ORDER BY line")
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (4, "HIGH".to_string()),
            (5, "MEDIUM".to_string()),
            (6, "LOW".to_string()),
        ]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn tested_by_comes_back_when_a_later_run_resolves_the_call() {
    // The first run cannot resolve `later` and drops its bare TESTED_BY;
    // once `a.py` declares it, the next run resolves the call and the test
    // covers it again.
    let path = temp_db("tested-by-later");
    let mut store = GraphStore::open(&path).expect("open");
    let test = "tests/test_b.py::test_run";
    let file = "tests/test_b.py";
    store_test_file(
        &mut store,
        &[
            edge("CALLS", test, "later", file, 2),
            edge("TESTED_BY", "later", test, file, 2),
        ],
    );
    store.resolve_bare_call_targets().unwrap();
    assert!(tested_by_rows(&store).is_empty());
    store
        .store_file_nodes_edges(
            "a.py",
            &[
                file_node("a.py"),
                function_node("helper", "a.py"),
                function_node("later", "a.py"),
            ],
            &[],
            "",
            0,
        )
        .expect("store a");
    assert_eq!(store.resolve_bare_call_targets().unwrap(), 1);
    assert_eq!(
        tested_by_rows(&store),
        vec![(
            "a.py::later".to_string(),
            test.to_string(),
            2,
            "HIGH".to_string()
        )]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn reexported_targets_follow_the_package_imports() {
    // `from pkg import NodeInfo` names `pkg/__init__.py::NodeInfo`, which
    // `__init__.py` imports from `pkg/types.py`; `Store` comes from a star
    // import of `pkg/store.py`, renamed on the way (`from .core import
    // Store as Engine` in `pkg/store.py`).
    let path = temp_db("reexports");
    let mut store = GraphStore::open(&path).expect("open");
    let names =
        |pairs: &[(&str, &str)]| json!(pairs.iter().map(|(n, a)| [n, a]).collect::<Vec<_>>());
    for (file, nodes, edges) in [
        (
            "pkg/types.py",
            vec![
                file_node("pkg/types.py"),
                class_node("NodeInfo", "pkg/types.py"),
                method_node("load", "pkg/types.py", "NodeInfo"),
            ],
            vec![],
        ),
        (
            "pkg/core.py",
            vec![file_node("pkg/core.py"), class_node("Store", "pkg/core.py")],
            vec![],
        ),
        (
            "pkg/store.py",
            vec![file_node("pkg/store.py")],
            vec![EdgeInput {
                extra: json!({"module": ".core", "names": names(&[("Store", "Engine")])}),
                ..edge(
                    "IMPORTS_FROM",
                    "pkg/store.py",
                    "pkg/core.py",
                    "pkg/store.py",
                    1,
                )
            }],
        ),
        (
            "pkg/__init__.py",
            vec![file_node("pkg/__init__.py")],
            vec![
                EdgeInput {
                    extra: json!({"module": ".types", "names": names(&[("NodeInfo", "NodeInfo")])}),
                    ..edge(
                        "IMPORTS_FROM",
                        "pkg/__init__.py",
                        "pkg/types.py",
                        "pkg/__init__.py",
                        1,
                    )
                },
                EdgeInput {
                    extra: json!({"module": ".store", "names": []}),
                    ..edge(
                        "IMPORTS_FROM",
                        "pkg/__init__.py",
                        "pkg/store.py",
                        "pkg/__init__.py",
                        2,
                    )
                },
            ],
        ),
        (
            "app.py",
            vec![file_node("app.py"), function_node("main", "app.py")],
            vec![
                edge(
                    "CALLS",
                    "app.py::main",
                    "pkg/__init__.py::NodeInfo",
                    "app.py",
                    3,
                ),
                edge(
                    "CALLS",
                    "app.py::main",
                    "pkg/__init__.py::NodeInfo.load",
                    "app.py",
                    4,
                ),
                edge(
                    "CALLS",
                    "app.py::main",
                    "pkg/__init__.py::Engine",
                    "app.py",
                    5,
                ),
                edge(
                    "CALLS",
                    "app.py::main",
                    "pkg/__init__.py::Missing",
                    "app.py",
                    6,
                ),
            ],
        ),
    ] {
        store
            .store_file_nodes_edges(file, &nodes, &edges, "", 0)
            .expect("store");
    }
    assert_eq!(store.resolve_bare_call_targets().unwrap(), 3);
    let mut stmt = store
        .conn
        .prepare(
            "SELECT line, target_qualified, confidence_tier FROM edges \
             WHERE kind = 'CALLS' ORDER BY line",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (3, "pkg/types.py::NodeInfo".to_string(), "HIGH".to_string()),
            (
                4,
                "pkg/types.py::NodeInfo.load".to_string(),
                "HIGH".to_string()
            ),
            (5, "pkg/core.py::Store".to_string(), "HIGH".to_string()),
            (
                6,
                "pkg/__init__.py::Missing".to_string(),
                "EXTRACTED".to_string()
            ),
        ]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn methods_only_the_standard_library_defines_point_at_it() {
    // `names.iter()` on a receiver of unknown type: no Rust function of the
    // repository is `iter`, so it is `std`'s. `get` is also a method of a
    // repository type, so it stays unresolved; so does a Python `strip`
    // with a known receiver.
    let path = temp_db("stdlib-methods");
    let mut store = GraphStore::open(&path).expect("open");
    let unknown = |target: &str, file: &str, source: &str, line: i64| EdgeInput {
        extra: json!({"receiver_unknown": true}),
        ..edge("CALLS", source, target, file, line)
    };
    store
        .store_file_nodes_edges(
            "src/lib.rs",
            &[
                file_node("src/lib.rs"),
                function_node("main", "src/lib.rs"),
                class_node("Store", "src/lib.rs"),
                method_node("get", "src/lib.rs", "Store"),
            ],
            &[
                unknown("iter", "src/lib.rs", "src/lib.rs::main", 2),
                unknown("get", "src/lib.rs", "src/lib.rs::main", 3),
            ],
            "",
            0,
        )
        .expect("store rust");
    store
        .store_file_nodes_edges(
            "app.py",
            &[file_node("app.py"), function_node("run", "app.py")],
            &[
                unknown("strip", "app.py", "app.py::run", 2),
                unknown("write_text", "app.py", "app.py::run", 3),
                edge("CALLS", "app.py::run", "strip", "app.py", 4),
            ],
            "",
            0,
        )
        .expect("store python");
    store.resolve_bare_call_targets().unwrap();
    store.demote_unresolved_endpoint_edges().unwrap();
    let mut stmt = store
        .conn
        .prepare(
            "SELECT file_path, line, target_qualified, confidence_tier, \
             json_extract(extra, '$.external_symbol') FROM edges \
             WHERE kind = 'CALLS' ORDER BY file_path, line",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    let row = |line: i64, target: &str, tier: &str, symbol: Option<&str>| {
        (
            line,
            target.to_string(),
            tier.to_string(),
            symbol.map(str::to_string),
        )
    };
    assert_eq!(
        rows,
        vec![
            row(2, "builtins", "MEDIUM", Some("strip")),
            row(3, "pathlib", "MEDIUM", Some("write_text")),
            row(4, "strip", "LOW", None),
            row(2, "std", "MEDIUM", Some("iter")),
            row(3, "get", "LOW", None),
        ]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn names_a_glob_imports_from_a_crate_point_at_it() {
    // `src/core_tests/x.rs` has `use super::*;` of `src/core.rs`, which
    // imports `HashMap` from `std` and `Node` from `tree_sitter`.
    let path = temp_db("glob-crates");
    let mut store = GraphStore::open(&path).expect("open");
    store
        .store_file_nodes_edges(
            "src/core.rs",
            &[file_node("src/core.rs")],
            &[
                EdgeInput {
                    extra: json!({"external": true, "external_package": "std", "stdlib": true,
                                  "paths": ["std::collections::HashMap"]}),
                    ..edge("IMPORTS_FROM", "src/core.rs", "std", "src/core.rs", 1)
                },
                EdgeInput {
                    extra: json!({"external": true, "external_package": "tree_sitter",
                                  "paths": ["tree_sitter::Node"]}),
                    ..edge(
                        "IMPORTS_FROM",
                        "src/core.rs",
                        "tree_sitter",
                        "src/core.rs",
                        2,
                    )
                },
            ],
            "",
            0,
        )
        .expect("store core");
    let file = "src/core_tests/x.rs";
    let caller = "src/core_tests/x.rs::check";
    let typed = |target: &str, receiver: &str, line: i64| EdgeInput {
        extra: json!({"receiver_type": receiver}),
        ..edge("CALLS", caller, target, file, line)
    };
    store
        .store_file_nodes_edges(
            file,
            &[file_node(file), function_node("check", file)],
            &[
                EdgeInput {
                    extra: json!({"glob": true}),
                    ..edge("IMPORTS_FROM", file, "src/core.rs", file, 1)
                },
                typed("new", "HashMap", 3),
                typed("kind", "Node", 4),
                typed("save", "Store", 5),
            ],
            "",
            0,
        )
        .expect("store tests");
    store.resolve_bare_call_targets().unwrap();
    let mut stmt = store
        .conn
        .prepare(
            "SELECT line, target_qualified, json_extract(extra, '$.external_symbol'), \
             COALESCE(json_extract(extra, '$.stdlib'), 0) FROM edges \
             WHERE kind = 'CALLS' ORDER BY line",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (3, "std".to_string(), Some("HashMap::new".to_string()), 1),
            (
                4,
                "tree_sitter".to_string(),
                Some("Node::kind".to_string()),
                0
            ),
            (5, "save".to_string(), None, 0),
        ]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn reexported_targets_reach_a_class_the_module_getattr_defines() {
    // `from pkg import Store` with `pkg/__init__.py` returning a class its
    // module `__getattr__` defines (PEP 562).
    let path = temp_db("reexports-getattr");
    let mut store = GraphStore::open(&path).expect("open");
    store
        .store_file_nodes_edges(
            "pkg/__init__.py",
            &[
                file_node("pkg/__init__.py"),
                function_node("__getattr__", "pkg/__init__.py"),
                method_node("Store", "pkg/__init__.py", "__getattr__"),
            ],
            &[],
            "",
            0,
        )
        .expect("store pkg");
    store
        .store_file_nodes_edges(
            "app.py",
            &[file_node("app.py"), function_node("main", "app.py")],
            &[edge(
                "CALLS",
                "app.py::main",
                "pkg/__init__.py::Store",
                "app.py",
                2,
            )],
            "",
            0,
        )
        .expect("store app");
    assert_eq!(store.resolve_bare_call_targets().unwrap(), 1);
    let target: String = store
        .conn
        .query_row(
            "SELECT target_qualified FROM edges WHERE kind = 'CALLS'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target, "pkg/__init__.py::__getattr__.Store");
    let _ = std::fs::remove_file(path);
}

#[test]
fn methods_a_type_lacks_are_those_it_dereferences_to() {
    // `FilePath: Deref<Target = str>` declares no `as_str`; `file_path.as_str()`
    // is `str`'s. `Store` dereferences to nothing: its call stays.
    let path = temp_db("deref");
    let mut store = GraphStore::open(&path).expect("open");
    let mut file_path = class_node("FilePath", "src/types.rs");
    file_path.extra = json!({"deref_target": "str"});
    store
        .store_file_nodes_edges(
            "src/types.rs",
            &[
                file_node("src/types.rs"),
                file_path,
                class_node("Store", "src/types.rs"),
            ],
            &[],
            "",
            0,
        )
        .expect("store types");
    let typed = |target: &str, receiver: &str, line: i64| EdgeInput {
        extra: json!({"receiver_type": receiver}),
        ..edge("CALLS", "src/run.rs::run", target, "src/run.rs", line)
    };
    store
        .store_file_nodes_edges(
            "src/run.rs",
            &[file_node("src/run.rs"), function_node("run", "src/run.rs")],
            &[
                typed("as_str", "FilePath", 2),
                typed("save", "Store", 3),
                typed("expect", "Store", 4),
            ],
            "",
            0,
        )
        .expect("store run");
    store.resolve_bare_call_targets().unwrap();
    let mut stmt = store
        .conn
        .prepare(
            "SELECT line, target_qualified, json_extract(extra, '$.external_symbol') \
             FROM edges WHERE kind = 'CALLS' ORDER BY line",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (2, "std".to_string(), Some("str::as_str".to_string())),
            (3, "save".to_string(), None),
            // `Store::open(p).expect(..)`: the `Result`'s.
            (4, "std".to_string(), Some("Store::expect".to_string())),
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
fn standard_methods_yield_only_to_functions_the_caller_can_see() {
    // `pool.py` declares `Pool.get`. `report.py` never imports it, so its
    // `data.get(...)` is `dict`'s; `app.py` imports `pool.py`, so its
    // `thing.get(...)` may be `Pool.get`, never `dict`'s.
    let path = temp_db("stdlib-methods-visible");
    let mut store = GraphStore::open(&path).expect("open");
    let unknown = |file: &str, source: &str| EdgeInput {
        extra: json!({"receiver_unknown": true}),
        ..edge("CALLS", source, "get", file, 2)
    };
    for (file, nodes, edges) in [
        (
            "pool.py",
            vec![
                file_node("pool.py"),
                class_node("Pool", "pool.py"),
                method_node("get", "pool.py", "Pool"),
            ],
            vec![],
        ),
        (
            "report.py",
            vec![file_node("report.py"), function_node("run", "report.py")],
            vec![unknown("report.py", "report.py::run")],
        ),
        (
            "app.py",
            vec![file_node("app.py"), function_node("run", "app.py")],
            vec![
                edge("IMPORTS_FROM", "app.py", "pool.py", "app.py", 1),
                unknown("app.py", "app.py::run"),
            ],
        ),
    ] {
        store
            .store_file_nodes_edges(file, &nodes, &edges, "", 0)
            .expect("store");
    }
    store.resolve_bare_call_targets().unwrap();
    let mut stmt = store
        .conn
        .prepare(
            "SELECT file_path, target_qualified FROM edges WHERE kind = 'CALLS' \
             ORDER BY file_path",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            // Bound by import visibility, as any bare name.
            ("app.py".to_string(), "pool.py::Pool.get".to_string()),
            ("report.py".to_string(), "builtins".to_string()),
        ]
    );
    let _ = std::fs::remove_file(path);
}

fn returning_function(
    name: &str,
    file_path: &str,
    owner: Option<&str>,
    returns: &str,
) -> NodeInput {
    let mut node = match owner {
        Some(owner) => method_node(name, file_path, owner),
        None => function_node(name, file_path),
    };
    node.return_type = Some(returns.to_string());
    node
}

#[test]
fn calls_on_a_returned_value_follow_the_declared_return_type() {
    // `helpers.py`: `store_conn(...) -> sqlite3.Connection` (a standard
    // library type) and `make_store() -> Store` (a class of `store.py`);
    // `store.py`: `Store.pool() -> Pool` and `Pool.get`.
    let path = temp_db("returned");
    let mut store = GraphStore::open(&path).expect("open");
    let from = |target: &str, call: &str, line: i64, at: i64| EdgeInput {
        extra: json!({"receiver_unknown": true,
                      "receiver_from": {"call": call, "line": line, "unwrap": false}}),
        ..edge("CALLS", "app.py::run", target, "app.py", at)
    };
    for (file, nodes, edges) in [
        (
            "store.py",
            vec![
                file_node("store.py"),
                class_node("Store", "store.py"),
                returning_function("pool", "store.py", Some("Store"), "Pool"),
                class_node("Pool", "store.py"),
                method_node("get", "store.py", "Pool"),
            ],
            vec![],
        ),
        (
            "helpers.py",
            vec![
                file_node("helpers.py"),
                returning_function("store_conn", "helpers.py", None, "sqlite3.Connection"),
                returning_function("make_store", "helpers.py", None, "Store"),
            ],
            vec![
                EdgeInput {
                    extra: json!({"external": true, "external_package": "sqlite3",
                                  "stdlib": true, "module": "sqlite3"}),
                    ..edge("IMPORTS_FROM", "helpers.py", "sqlite3", "helpers.py", 1)
                },
                EdgeInput {
                    extra: json!({"module": "store", "names": [["Store", "Store"]]}),
                    ..edge("IMPORTS_FROM", "helpers.py", "store.py", "helpers.py", 2)
                },
            ],
        ),
        (
            "app.py",
            vec![file_node("app.py"), function_node("run", "app.py")],
            vec![
                edge("IMPORTS_FROM", "app.py", "helpers.py", "app.py", 1),
                // store_conn(x).execute()
                edge("CALLS", "app.py::run", "store_conn", "app.py", 2),
                from("execute", "store_conn", 2, 2),
                // make_store().pool().get()
                edge("CALLS", "app.py::run", "make_store", "app.py", 3),
                EdgeInput {
                    line: 3,
                    ..from("pool", "make_store", 3, 3)
                },
                from("get", "pool", 3, 3),
            ],
        ),
    ] {
        store
            .store_file_nodes_edges(file, &nodes, &edges, "", 0)
            .expect("store");
    }
    store.resolve_bare_call_targets().unwrap();
    let mut stmt = store
        .conn
        .prepare(
            "SELECT target_qualified, confidence_tier, json_extract(extra, '$.external_symbol') \
             FROM edges WHERE kind = 'CALLS' ORDER BY line, id",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    let rows = rows
        .into_iter()
        .map(|(target, tier, symbol)| (target, symbol.unwrap_or(tier)))
        .collect::<Vec<_>>();
    let row = |target: &str, detail: &str| (target.to_string(), detail.to_string());
    assert_eq!(
        rows,
        vec![
            row("helpers.py::store_conn", "HIGH"),
            row("sqlite3", "sqlite3.Connection.execute"),
            row("helpers.py::make_store", "HIGH"),
            row("store.py::Store.pool", "MEDIUM"),
            row("store.py::Pool.get", "MEDIUM"),
        ]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn rust_results_unwrap_to_their_value_and_self_is_the_owner() {
    // `Store::open(p)?.save()`: `open -> Result<Self>` unwrapped is `Store`.
    let path = temp_db("returned-rust");
    let mut store = GraphStore::open(&path).expect("open");
    store
        .store_file_nodes_edges(
            "src/store.rs",
            &[
                file_node("src/store.rs"),
                class_node("Store", "src/store.rs"),
                returning_function("open", "src/store.rs", Some("Store"), "Result<Self>"),
                method_node("save", "src/store.rs", "Store"),
                function_node("run", "src/store.rs"),
            ],
            &[
                edge(
                    "CALLS",
                    "src/store.rs::run",
                    "src/store.rs::Store.open",
                    "src/store.rs",
                    2,
                ),
                EdgeInput {
                    extra: json!({"receiver_unknown": true,
                                  "receiver_from": {"call": "open", "line": 2, "unwrap": true}}),
                    ..edge("CALLS", "src/store.rs::run", "save", "src/store.rs", 2)
                },
            ],
            "",
            0,
        )
        .expect("store");
    store.resolve_bare_call_targets().unwrap();
    let target: String = store
        .conn
        .query_row(
            "SELECT target_qualified FROM edges WHERE kind = 'CALLS' AND id = \
             (SELECT MAX(id) FROM edges WHERE kind = 'CALLS')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target, "src/store.rs::Store.save");
    let _ = std::fs::remove_file(path);
}

#[test]
fn python_calls_on_a_pyclass_bind_to_its_pymethods() {
    // `#[pyclass(name = "GraphStore")] struct PyGraphStore` with
    // `#[pymethods] fn upsert_node`; Python calls `store.upsert_node()`
    // on `store: GraphStore`.
    let path = temp_db("pyo3-methods");
    let mut store = GraphStore::open(&path).expect("open");
    let mut class = class_node("PyGraphStore", "src/lib.rs");
    class.extra = json!({"ffi_export": {"abi": "pyo3", "kind": "class", "name": "GraphStore"}});
    let mut method = method_node("upsert_node", "src/lib.rs", "PyGraphStore");
    method.extra = json!({"ffi_export": {"abi": "pyo3", "kind": "method", "name": "upsert_node"}});
    store
        .store_file_nodes_edges(
            "src/lib.rs",
            &[file_node("src/lib.rs"), class, method],
            &[],
            "",
            0,
        )
        .expect("store rust");
    store
        .store_file_nodes_edges(
            "app.py",
            &[file_node("app.py"), function_node("run", "app.py")],
            &[EdgeInput {
                extra: json!({"receiver_type": "GraphStore"}),
                ..edge("CALLS", "app.py::run", "upsert_node", "app.py", 2)
            }],
            "",
            0,
        )
        .expect("store python");
    store.resolve_bare_call_targets().unwrap();
    let target: String = store
        .conn
        .query_row(
            "SELECT target_qualified FROM edges WHERE kind = 'CALLS'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target, "src/lib.rs::PyGraphStore.upsert_node");
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_returned_value_is_not_bound_by_a_visible_name() {
    // `store_conn(s).commit()` with `store_conn -> sqlite3.Connection`:
    // `sqlite3`'s `commit`, not the visible `Protocol.commit` of `proto.py`.
    let path = temp_db("returned-over-bare");
    let mut store = GraphStore::open(&path).expect("open");
    for (file, nodes, edges) in [
        (
            "proto.py",
            vec![
                file_node("proto.py"),
                class_node("Protocol", "proto.py"),
                method_node("commit", "proto.py", "Protocol"),
            ],
            vec![],
        ),
        (
            "helpers.py",
            vec![
                file_node("helpers.py"),
                returning_function("store_conn", "helpers.py", None, "sqlite3.Connection"),
            ],
            vec![EdgeInput {
                extra: json!({"external": true, "external_package": "sqlite3",
                              "stdlib": true, "module": "sqlite3"}),
                ..edge("IMPORTS_FROM", "helpers.py", "sqlite3", "helpers.py", 1)
            }],
        ),
        (
            "app.py",
            vec![file_node("app.py"), function_node("run", "app.py")],
            vec![
                edge("IMPORTS_FROM", "app.py", "proto.py", "app.py", 1),
                edge(
                    "CALLS",
                    "app.py::run",
                    "helpers.py::store_conn",
                    "app.py",
                    2,
                ),
                EdgeInput {
                    extra: json!({"receiver_unknown": true,
                                  "receiver_from": {"call": "store_conn", "line": 2, "unwrap": false}}),
                    ..edge("CALLS", "app.py::run", "commit", "app.py", 2)
                },
            ],
        ),
    ] {
        store
            .store_file_nodes_edges(file, &nodes, &edges, "", 0)
            .expect("store");
    }
    store.resolve_bare_call_targets().unwrap();
    let target: String = store
        .conn
        .query_row(
            "SELECT target_qualified FROM edges WHERE kind = 'CALLS' AND target_name != 'store_conn'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target, "sqlite3");
    let _ = std::fs::remove_file(path);
}

#[test]
fn unknown_receivers_take_the_package_typed_calls_of_the_method_reach() {
    // `Node::kind` twice on typed receivers (tree_sitter); `child.kind()`
    // in a loop is tree_sitter's too. `name` is seen on two packages as
    // often, and `save` once: both stay unresolved.
    let path = temp_db("observed-methods");
    let mut store = GraphStore::open(&path).expect("open");
    let typed = |symbol: &str, package: &str, line: i64| EdgeInput {
        extra: json!({"external": true, "external_package": package,
                      "external_symbol": symbol, "confidence_tier": "MEDIUM"}),
        ..edge("CALLS", "src/a.rs::run", package, "src/a.rs", line)
    };
    let unknown = |method: &str, line: i64| EdgeInput {
        extra: json!({"receiver_unknown": true}),
        ..edge("CALLS", "src/a.rs::run", method, "src/a.rs", line)
    };
    store
        .store_file_nodes_edges(
            "src/a.rs",
            &[file_node("src/a.rs"), function_node("run", "src/a.rs")],
            &[
                typed("Node::kind", "tree_sitter", 1),
                typed("Node::kind", "tree_sitter", 2),
                typed("Field::name", "syn", 3),
                typed("Ident::name", "proc_macro2", 4),
                typed("Store::save", "sled", 5),
                unknown("kind", 10),
                unknown("name", 11),
                unknown("save", 12),
            ],
            "",
            0,
        )
        .expect("store");
    store.resolve_bare_call_targets().unwrap();
    let mut stmt = store
        .conn
        .prepare("SELECT line, target_qualified FROM edges WHERE kind = 'CALLS' AND line >= 10 ORDER BY line")
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (10, "tree_sitter".to_string()),
            (11, "name".to_string()),
            (12, "save".to_string()),
        ]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn returned_values_follow_unwrapped_packages_globbed_imports_and_skip_themselves() {
    // `tx.rs`: `write_tx() -> Result<Transaction<'_>>`, `Transaction` from
    // `use rusqlite::Transaction` in `lib.rs`, which `tx.rs` glob-imports.
    // `run.rs`: `write_tx()?.execute()` is rusqlite's; `stmt(..)?.query_map()`
    // on an unwrapped rusqlite call is rusqlite's; `Build::new().flag()` and
    // `.include()` are the builder's.
    let path = temp_db("returned-more");
    let mut store = GraphStore::open(&path).expect("open");
    for (file, nodes, edges) in [
        (
            "src/lib.rs",
            vec![file_node("src/lib.rs")],
            vec![EdgeInput {
                extra: json!({"external": true, "external_package": "rusqlite",
                              "paths": ["rusqlite::Transaction"]}),
                ..edge("IMPORTS_FROM", "src/lib.rs", "rusqlite", "src/lib.rs", 1)
            }],
        ),
        (
            "src/tx.rs",
            vec![
                file_node("src/tx.rs"),
                returning_function("write_tx", "src/tx.rs", None, "Result<Transaction<'_>>"),
            ],
            vec![EdgeInput {
                extra: json!({"glob": true}),
                ..edge("IMPORTS_FROM", "src/tx.rs", "src/lib.rs", "src/tx.rs", 1)
            }],
        ),
        (
            "src/run.rs",
            vec![file_node("src/run.rs"), function_node("run", "src/run.rs")],
            vec![
                edge(
                    "CALLS",
                    "src/run.rs::run",
                    "src/tx.rs::write_tx",
                    "src/run.rs",
                    2,
                ),
                EdgeInput {
                    extra: json!({"receiver_unknown": true,
                                  "receiver_from": {"call": "write_tx", "line": 2, "unwrap": true}}),
                    ..edge("CALLS", "src/run.rs::run", "execute", "src/run.rs", 2)
                },
                EdgeInput {
                    extra: json!({"external": true, "external_package": "rusqlite",
                                  "external_symbol": "Connection::prepare"}),
                    ..edge("CALLS", "src/run.rs::run", "rusqlite", "src/run.rs", 3)
                },
                EdgeInput {
                    extra: json!({"receiver_unknown": true,
                                  "receiver_from": {"call": "prepare", "line": 3, "unwrap": true}}),
                    ..edge("CALLS", "src/run.rs::run", "query_map", "src/run.rs", 3)
                },
                EdgeInput {
                    extra: json!({"external": true, "external_package": "cc",
                                  "external_symbol": "cc::Build::new"}),
                    ..edge("CALLS", "src/run.rs::run", "cc", "src/run.rs", 4)
                },
                EdgeInput {
                    extra: json!({"receiver_unknown": true,
                                  "receiver_from": {"call": "new", "line": 4, "unwrap": false}}),
                    ..edge("CALLS", "src/run.rs::run", "flag", "src/run.rs", 4)
                },
                EdgeInput {
                    extra: json!({"receiver_unknown": true,
                                  "receiver_from": {"call": "new", "line": 4, "unwrap": false}}),
                    ..edge("CALLS", "src/run.rs::run", "include", "src/run.rs", 4)
                },
            ],
        ),
    ] {
        store
            .store_file_nodes_edges(file, &nodes, &edges, "", 0)
            .expect("store");
    }
    store.resolve_bare_call_targets().unwrap();
    let mut stmt = store
        .conn
        .prepare(
            "SELECT target_qualified FROM edges WHERE kind = 'CALLS' \
             AND json_extract(extra, '$.receiver_from') IS NOT NULL ORDER BY id",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows, vec!["rusqlite", "rusqlite", "cc", "cc"]);
    // `Build::new().flag(a).flag(b)`: the one `flag` edge's receiver is
    // what `new` returned, never itself.
    let _ = std::fs::remove_file(path);
}

#[test]
fn awaited_promises_and_package_qualified_types_type_their_results() {
    // `(await load()).save()` with `load(): Promise<User>`; Go
    // `client().Get(url)` with `func client() *http.Client` and `import
    // "net/http"`.
    let path = temp_db("returned-languages");
    let mut store = GraphStore::open(&path).expect("open");
    let from = |file: &str, target: &str, call: &str, unwrap: bool| EdgeInput {
        extra: json!({"receiver_unknown": true,
                      "receiver_from": {"call": call, "line": 2, "unwrap": unwrap}}),
        ..edge("CALLS", &format!("{file}::run"), target, file, 2)
    };
    for (file, nodes, edges) in [
        (
            "src/user.ts",
            vec![
                file_node("src/user.ts"),
                class_node("User", "src/user.ts"),
                method_node("save", "src/user.ts", "User"),
            ],
            vec![],
        ),
        (
            "src/api.ts",
            vec![
                file_node("src/api.ts"),
                returning_function("load", "src/api.ts", None, "Promise<User>"),
            ],
            vec![EdgeInput {
                extra: json!({"names": [["User", "User"]]}),
                ..edge("IMPORTS_FROM", "src/api.ts", "src/user.ts", "src/api.ts", 1)
            }],
        ),
        (
            "src/app.ts",
            vec![file_node("src/app.ts"), function_node("run", "src/app.ts")],
            vec![
                edge(
                    "CALLS",
                    "src/app.ts::run",
                    "src/api.ts::load",
                    "src/app.ts",
                    2,
                ),
                from("src/app.ts", "save", "load", true),
            ],
        ),
        (
            "net.go",
            vec![
                file_node("net.go"),
                returning_function("client", "net.go", None, "*http.Client"),
                function_node("run", "net.go"),
            ],
            vec![
                EdgeInput {
                    extra: json!({"external": true, "external_package": "net/http", "stdlib": true}),
                    ..edge("IMPORTS_FROM", "net.go", "net/http", "net.go", 1)
                },
                edge("CALLS", "net.go::run", "net.go::client", "net.go", 2),
                from("net.go", "Get", "client", false),
            ],
        ),
    ] {
        store
            .store_file_nodes_edges(file, &nodes, &edges, "", 0)
            .expect("store");
    }
    store.resolve_bare_call_targets().unwrap();
    let mut stmt = store
        .conn
        .prepare(
            "SELECT file_path, target_qualified, json_extract(extra, '$.external_symbol') \
             FROM edges WHERE kind = 'CALLS' \
             AND json_extract(extra, '$.receiver_from') IS NOT NULL ORDER BY file_path",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (
                "net.go".to_string(),
                "net/http".to_string(),
                Some("http.Client.Get".to_string())
            ),
            (
                "src/app.ts".to_string(),
                "src/user.ts::User.save".to_string(),
                None
            ),
        ]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn enum_variant_calls_construct_their_enum() {
    // `Receiver::Local(x)` (left as `Local` with `receiver_type`) is a call
    // of the enum `Receiver`; `Receiver::missing()` is not a variant.
    let path = temp_db("enum-variants");
    let mut store = GraphStore::open(&path).expect("open");
    let mut receiver = class_node("Receiver", "src/types.rs");
    receiver.extra = json!({"type_role": "enum", "variants": ["Local", "Unknown"]});
    store
        .store_file_nodes_edges(
            "src/types.rs",
            &[file_node("src/types.rs"), receiver],
            &[],
            "",
            0,
        )
        .expect("store types");
    let typed = |target: &str, line: i64| EdgeInput {
        extra: json!({"receiver_type": "Receiver"}),
        ..edge("CALLS", "src/run.rs::run", target, "src/run.rs", line)
    };
    store
        .store_file_nodes_edges(
            "src/run.rs",
            &[file_node("src/run.rs"), function_node("run", "src/run.rs")],
            &[typed("Local", 2), typed("missing", 3)],
            "",
            0,
        )
        .expect("store run");
    store.resolve_bare_call_targets().unwrap();
    let mut stmt = store
        .conn
        .prepare(
            "SELECT line, target_qualified, json_extract(extra, '$.enum_variant') \
             FROM edges WHERE kind = 'CALLS' ORDER BY line",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (
                2,
                "src/types.rs::Receiver".to_string(),
                Some("Local".to_string())
            ),
            (3, "missing".to_string(), None),
        ]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn calls_on_what_an_observed_method_returned_follow_its_package() {
    // `Statement::query_map` twice and `Connection::prepare` twice on typed
    // receivers; `conn.prepare(..)?` on an untyped `conn` is rusqlite's only
    // once every other pass ran, and `.query_map(..)` on its result follows.
    let path = temp_db("returned-observed");
    let mut store = GraphStore::open(&path).expect("open");
    let typed = |symbol: &str, line: i64| EdgeInput {
        extra: json!({"external": true, "external_package": "rusqlite",
                      "external_symbol": symbol, "confidence_tier": "MEDIUM"}),
        ..edge("CALLS", "src/a.rs::run", "rusqlite", "src/a.rs", line)
    };
    store
        .store_file_nodes_edges(
            "src/a.rs",
            &[file_node("src/a.rs"), function_node("run", "src/a.rs")],
            &[
                typed("Connection::prepare", 1),
                typed("Connection::prepare", 2),
                EdgeInput {
                    extra: json!({"receiver_unknown": true}),
                    ..edge("CALLS", "src/a.rs::run", "prepare", "src/a.rs", 10)
                },
                EdgeInput {
                    extra: json!({"receiver_unknown": true,
                                  "receiver_from": {"call": "prepare", "line": 10, "unwrap": true}}),
                    ..edge("CALLS", "src/a.rs::run", "query_map", "src/a.rs", 11)
                },
            ],
            "",
            0,
        )
        .expect("store");
    store.resolve_bare_call_targets().unwrap();
    let rows = store
        .conn
        .prepare(
            "SELECT line, target_qualified, json_extract(extra, '$.external_symbol') \
             FROM edges WHERE kind = 'CALLS' AND line >= 10 ORDER BY line",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (10, "rusqlite".to_string(), Some("prepare".to_string())),
            (
                11,
                "rusqlite".to_string(),
                Some("prepare()::query_map".to_string())
            ),
        ]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn python_standard_library_calls_of_known_return_type_type_their_result() {
    // `conn.execute(..).fetchall()` (a `sqlite3.Cursor`), `re.match(..)
    // .group()` (a `re.Match`). `path.read_text().splitlines()` is `str`'s
    // (builtins), not pathlib's; `pytest.importorskip("numpy").array()`
    // anything, so it stays. `close`
    // is a function `app.py` declares, so a call on a result is left to it.
    let path = temp_db("returned-python-stdlib");
    let mut store = GraphStore::open(&path).expect("open");
    let stdlib = |package: &str, symbol: &str, line: i64| EdgeInput {
        extra: json!({"external": true, "external_package": package, "stdlib": true,
                      "external_symbol": symbol, "confidence_tier": "MEDIUM"}),
        ..edge("CALLS", "app.py::run", package, "app.py", line)
    };
    let from = |method: &str, call: &str, line: i64| EdgeInput {
        extra: json!({"receiver_unknown": true,
                      "receiver_from": {"call": call, "line": line, "unwrap": false}}),
        ..edge("CALLS", "app.py::run", method, "app.py", line)
    };
    store
        .store_file_nodes_edges(
            "app.py",
            &[
                file_node("app.py"),
                function_node("run", "app.py"),
                function_node("close", "app.py"),
            ],
            &[
                stdlib("sqlite3", "execute", 1),
                from("fetchall", "execute", 1),
                stdlib("re", "re.match", 2),
                from("group", "match", 2),
                stdlib("pathlib", "pathlib.Path.read_text", 3),
                from("splitlines", "read_text", 3),
                EdgeInput {
                    extra: json!({"external": true, "external_package": "pytest",
                                  "external_symbol": "pytest.importorskip"}),
                    ..edge("CALLS", "app.py::run", "pytest", "app.py", 4)
                },
                from("array", "importorskip", 4),
                stdlib("sqlite3", "connect", 5),
                from("close", "connect", 5),
            ],
            "",
            0,
        )
        .expect("store");
    store.resolve_bare_call_targets().unwrap();
    let rows = store
        .conn
        .prepare(
            "SELECT line, target_qualified, json_extract(extra, '$.external_symbol') \
             FROM edges WHERE kind = 'CALLS' \
               AND json_extract(extra, '$.receiver_from') IS NOT NULL ORDER BY line",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    let row = |line: i64, target: &str, symbol: Option<&str>| {
        (line, target.to_string(), symbol.map(str::to_string))
    };
    assert_eq!(
        rows,
        vec![
            row(1, "sqlite3", Some("sqlite3.Cursor.fetchall")),
            row(2, "re", Some("re.Match.group")),
            row(3, "builtins", Some("splitlines")),
            row(4, "array", None),
            row(5, "app.py::close", None),
        ]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn javascript_package_calls_of_known_return_type_type_their_result() {
    // `vscode.workspace.getConfiguration(..).get(..)` is a
    // `vscode.WorkspaceConfiguration`'s; `d3.select(..).append(..).attr(..)`
    // stays a `d3.Selection` down the chain. `fs.readFileSync(p).toString()`
    // is a `String`'s, not `node:fs`'s. Calls the table typed teach
    // observed-method inference nothing: `cache.get(key)` stays unresolved
    // however many configurations were read.
    let path = temp_db("returned-javascript");
    let mut store = GraphStore::open(&path).expect("open");
    let package = |target: &str, package: &str, line: i64| EdgeInput {
        extra: json!({"external": true, "external_package": package}),
        ..edge("CALLS", "src/a.ts::run", target, "src/a.ts", line)
    };
    let from = |method: &str, call: &str, line: i64| EdgeInput {
        extra: json!({"receiver_unknown": true,
                      "receiver_from": {"call": call, "line": line, "unwrap": false}}),
        ..edge("CALLS", "src/a.ts::run", method, "src/a.ts", line)
    };
    store
        .store_file_nodes_edges(
            "src/a.ts",
            &[file_node("src/a.ts"), function_node("run", "src/a.ts")],
            &[
                package("vscode::workspace.getConfiguration", "vscode", 1),
                from("get", "getConfiguration", 1),
                package("d3::select", "d3", 2),
                from("append", "select", 2),
                from("attr", "append", 2),
                package("node:fs::readFileSync", "node:fs", 3),
                from("toString", "readFileSync", 3),
                package("vscode::workspace.getConfiguration", "vscode", 4),
                from("get", "getConfiguration", 4),
                EdgeInput {
                    extra: json!({"receiver_unknown": true}),
                    ..edge("CALLS", "src/a.ts::run", "get", "src/a.ts", 5)
                },
            ],
            "",
            0,
        )
        .expect("store");
    store.resolve_bare_call_targets().unwrap();
    let rows = store
        .conn
        .prepare(
            "SELECT line, target_qualified, json_extract(extra, '$.external_symbol') \
             FROM edges WHERE kind = 'CALLS' \
               AND json_extract(extra, '$.receiver_from') IS NOT NULL ORDER BY line, id",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    let row = |line: i64, target: &str, symbol: Option<&str>| {
        (line, target.to_string(), symbol.map(str::to_string))
    };
    assert_eq!(
        rows,
        vec![
            row(1, "vscode", Some("vscode.WorkspaceConfiguration.get")),
            row(2, "d3", Some("d3.Selection.append")),
            row(2, "d3", Some("d3.Selection.attr")),
            row(3, "globalThis", Some("toString")),
            row(4, "vscode", Some("vscode.WorkspaceConfiguration.get")),
        ]
    );
    let cache_get: String = store
        .conn
        .query_row(
            "SELECT target_qualified FROM edges WHERE kind = 'CALLS' AND line = 5",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cache_get, "get");
    let _ = std::fs::remove_file(path);
}

#[test]
fn rust_orderings_compared_by_name_are_the_standard_librarys() {
    // `a.cmp(&b).then_with(..)` on untyped `a`: `cmp` is `Ord`'s and
    // `then_with` `Ordering`'s.
    let path = temp_db("rust-ordering");
    let mut store = GraphStore::open(&path).expect("open");
    store
        .store_file_nodes_edges(
            "src/a.rs",
            &[file_node("src/a.rs"), function_node("run", "src/a.rs")],
            &[
                EdgeInput {
                    extra: json!({"receiver_unknown": true}),
                    ..edge("CALLS", "src/a.rs::run", "cmp", "src/a.rs", 1)
                },
                EdgeInput {
                    extra: json!({"receiver_unknown": true,
                                  "receiver_from": {"call": "cmp", "line": 1, "unwrap": false}}),
                    ..edge("CALLS", "src/a.rs::run", "then_with", "src/a.rs", 1)
                },
            ],
            "",
            0,
        )
        .expect("store");
    store.resolve_bare_call_targets().unwrap();
    let targets = store
        .conn
        .prepare("SELECT target_qualified FROM edges WHERE kind = 'CALLS' ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(targets, vec!["std".to_string(), "std".to_string()]);
    let _ = std::fs::remove_file(path);
}
