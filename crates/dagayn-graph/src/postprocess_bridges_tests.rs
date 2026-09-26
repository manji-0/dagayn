use super::*;

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
                "MEDIUM".to_string()
            ),
            (
                "missing".to_string(),
                test.to_string(),
                3,
                "EXTRACTED".to_string()
            ),
            (
                "a.py::other".to_string(),
                test.to_string(),
                4,
                "MEDIUM".to_string()
            ),
        ]
    );
    // Idempotent: a second run changes nothing.
    assert_eq!(store.resolve_bare_call_targets().unwrap(), 0);
    assert_eq!(tested_by_rows(&store).len(), 3);
    let _ = std::fs::remove_file(path);
}

#[test]
fn tested_by_follows_calls_resolved_by_an_earlier_run() {
    // Graphs built before the sync: the CALLS edge is already resolved,
    // the TESTED_BY edge is still bare. A bare TESTED_BY whose own bare
    // CALLS remains (`obj.helper()` next to `helper()`) stays, and a
    // rewrite that would duplicate an edge drops the bare copy.
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
            ("ext.py::Remote.fetch".to_string(), 6),
            ("fetch".to_string(), 6),
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
