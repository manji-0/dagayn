use super::*;

/// `a := New(t); a.Equal()` in Go: `New` returns `*Assertions`, whose
/// `Equal` is declared in another file of its package, while another
/// package has an `Assertions` with an `Equal` too.
#[test]
fn go_methods_of_a_returned_type_are_found_in_its_package() {
    let path = temp_db("returned-go-package-method");
    let mut store = GraphStore::open(&path).expect("open");
    let go_class = |name: &str, file: &str| NodeInput {
        language: "go".to_string(),
        ..class_node(name, file)
    };
    let go_method = |name: &str, file: &str, owner: &str| NodeInput {
        language: "go".to_string(),
        ..method_node(name, file, owner)
    };
    let files: Vec<(&str, &str, Vec<NodeInput>, Vec<EdgeInput>)> = vec![
        (
            "assert/forward_assertions.go",
            "example.com/t/assert",
            vec![
                go_class("Assertions", "assert/forward_assertions.go"),
                NodeInput {
                    language: "go".to_string(),
                    return_type: Some("*Assertions".to_string()),
                    ..function_node("New", "assert/forward_assertions.go")
                },
            ],
            vec![],
        ),
        (
            "assert/assertion_forward.go",
            "example.com/t/assert",
            vec![go_method(
                "Equal",
                "assert/assertion_forward.go",
                "Assertions",
            )],
            vec![],
        ),
        (
            "require/forward_requirements.go",
            "example.com/t/require",
            vec![go_class("Assertions", "require/forward_requirements.go")],
            vec![],
        ),
        (
            "require/require_forward.go",
            "example.com/t/require",
            vec![go_method(
                "Equal",
                "require/require_forward.go",
                "Assertions",
            )],
            vec![],
        ),
        (
            "assert/a_test.go",
            "example.com/t/assert",
            vec![function_node("TestA", "assert/a_test.go")],
            vec![
                edge(
                    "CALLS",
                    "assert/a_test.go::TestA",
                    "New",
                    "assert/a_test.go",
                    2,
                ),
                EdgeInput {
                    extra: json!({
                        "receiver_unknown": true,
                        "receiver_from": {"call": "New", "line": 2, "unwrap": false},
                    }),
                    ..edge(
                        "CALLS",
                        "assert/a_test.go::TestA",
                        "Equal",
                        "assert/a_test.go",
                        3,
                    )
                },
                edge(
                    "CALLS",
                    "assert/a_test.go::TestA",
                    "Open",
                    "assert/a_test.go",
                    4,
                ),
                EdgeInput {
                    extra: json!({
                        "receiver_unknown": true,
                        "receiver_from": {"call": "Open", "line": 4, "unwrap": false},
                    }),
                    ..edge(
                        "CALLS",
                        "assert/a_test.go::TestA",
                        "Close",
                        "assert/a_test.go",
                        5,
                    )
                },
            ],
        ),
        // `Open` returns the standard `io.ReadCloser`, not the repository's
        // only `ReadCloser`, which is another package's.
        (
            "assert/open.go",
            "example.com/t/assert",
            vec![NodeInput {
                language: "go".to_string(),
                return_type: Some("io.ReadCloser".to_string()),
                ..function_node("Open", "assert/open.go")
            }],
            vec![EdgeInput {
                extra: json!({"stdlib": true, "external": true, "external_package": "io"}),
                ..edge("IMPORTS_FROM", "assert/open.go", "io", "assert/open.go", 1)
            }],
        ),
        (
            "zip/reader.go",
            "example.com/t/zip",
            vec![
                go_class("ReadCloser", "zip/reader.go"),
                go_method("Close", "zip/reader.go", "ReadCloser"),
            ],
            vec![],
        ),
    ];
    for (file, namespace, mut nodes, edges) in files {
        nodes.insert(0, namespaced_file_node(file, namespace));
        store
            .store_file_nodes_edges(file, &nodes, &edges, "", 0)
            .expect("store");
    }
    store.resolve_bare_call_targets().unwrap();
    let target = |line: i64| -> String {
        store
            .conn
            .query_row(
                "SELECT target_qualified FROM edges WHERE kind='CALLS' AND line = ?",
                [line],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(target(2), "assert/forward_assertions.go::New");
    assert_eq!(target(3), "assert/assertion_forward.go::Assertions.Equal");
    assert_eq!(target(4), "assert/open.go::Open");
    assert_ne!(target(5), "zip/reader.go::ReadCloser.Close");
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
        .query_map([], |row| <(String, String, Option<String>)>::try_from(row))
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
        .query_map([], |row| <(String, String, Option<String>)>::try_from(row))
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
        .query_map([], |row| <(i64, String, Option<String>)>::try_from(row))
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
        .query_map([], |row| <(i64, String, Option<String>)>::try_from(row))
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
        .query_map([], |row| <(i64, String, Option<String>)>::try_from(row))
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
fn methods_on_a_borrowed_cell_are_its_contents_not_the_standard_librarys() {
    // `self.bindings.borrow().snapshot()`: `Ref<T>` derefs to the bindings
    // type, so `snapshot` is not `std`'s. `std::fs::read(p)?.len()` still is.
    let path = temp_db("returned-guard");
    let mut store = GraphStore::open(&path).expect("open");
    let package = |symbol: &str, line: i64| EdgeInput {
        extra: json!({"external": true, "external_package": "std", "stdlib": true,
                      "external_symbol": symbol}),
        ..edge("CALLS", "src/a.rs::run", "std", "src/a.rs", line)
    };
    let from = |method: &str, call: &str, line: i64, unwrap: bool| EdgeInput {
        extra: json!({"receiver_unknown": true,
                      "receiver_from": {"call": call, "line": line, "unwrap": unwrap}}),
        ..edge("CALLS", "src/a.rs::run", method, "src/a.rs", line)
    };
    store
        .store_file_nodes_edges(
            "src/a.rs",
            &[file_node("src/a.rs"), function_node("run", "src/a.rs")],
            &[
                package("RefCell::borrow", 1),
                from("snapshot", "borrow", 1, false),
                package("Mutex::lock", 2),
                from("restore", "lock", 2, true),
                package("std::fs::read", 3),
                from("len", "read", 3, true),
            ],
            "",
            0,
        )
        .expect("store");
    store.resolve_bare_call_targets().unwrap();
    let rows = store
        .conn
        .prepare(
            "SELECT line, target_qualified FROM edges WHERE kind = 'CALLS' \
               AND json_extract(extra, '$.receiver_from') IS NOT NULL ORDER BY line",
        )
        .unwrap()
        .query_map([], |row| <(i64, String)>::try_from(row))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (1, "snapshot".to_string()),
            (2, "restore".to_string()),
            (3, "std".to_string()),
        ]
    );
    let _ = std::fs::remove_file(path);
}

/// Go calls on a field of a struct another file declares
/// (`receiver_field`) are typed by the struct's `field_types`, and a method
/// a struct does not declare is the one an embedded type of its package
/// declares at the shallowest depth (Go's promotion rule), never another
/// package's type of the same name, nor either of two at one depth.
#[test]
fn go_receivers_typed_by_struct_fields_and_embedded_types() {
    let path = temp_db("go-declared-receivers");
    let mut store = GraphStore::open(&path).expect("open");
    let go = |node: NodeInput| NodeInput {
        language: "go".to_string(),
        ..node
    };
    let go_struct = |name: &str, file: &str, extra: Value| NodeInput {
        extra,
        ..go(class_node(name, file))
    };
    let caller = |file: &str, list: &[(&str, i64, Value)]| -> Vec<EdgeInput> {
        list.iter()
            .map(|(target, line, extra)| EdgeInput {
                extra: extra.clone(),
                ..edge("CALLS", &format!("{file}::run"), target, file, *line)
            })
            .collect()
    };
    let files: Vec<(&str, &str, Vec<NodeInput>, Vec<EdgeInput>)> = vec![
        (
            "fse/scratch.go",
            "example.com/m/fse",
            vec![go_struct(
                "Scratch",
                "fse/scratch.go",
                json!({"field_types": {
                    "bw": "bitWriter",
                    "nodes": "[]nodeElt",
                    "ext": "example.com/m/huff0.bitWriter",
                    "hook": "",
                    "state": "encState",
                }}),
            )],
            vec![],
        ),
        (
            "fse/bitwriter.go",
            "example.com/m/fse",
            vec![
                go_struct("bitWriter", "fse/bitwriter.go", json!({"field_types": {}})),
                go(method_node("close", "fse/bitwriter.go", "bitWriter")),
                go_struct("nodeElt", "fse/bitwriter.go", json!({})),
                go_struct(
                    "encState",
                    "fse/bitwriter.go",
                    json!({"field_types": {"jobs": "[]bitWriter"}}),
                ),
                go(method_node("count", "fse/bitwriter.go", "nodeElt")),
            ],
            vec![],
        ),
        (
            "huff0/h.go",
            "example.com/m/huff0",
            vec![
                go_struct(
                    "Scratch",
                    "huff0/h.go",
                    json!({"field_types": {"bw": "bitWriter"}}),
                ),
                go_struct("bitWriter", "huff0/h.go", json!({"field_types": {}})),
                go(method_node("close", "huff0/h.go", "bitWriter")),
                go(method_node("flush", "huff0/h.go", "bitWriter")),
            ],
            vec![],
        ),
        (
            "fse/compress.go",
            "example.com/m/fse",
            vec![go(function_node("run", "fse/compress.go"))],
            caller(
                "fse/compress.go",
                &[
                    (
                        "close",
                        2,
                        json!({"receiver_unknown": true, "receiver_field": {"owner": "Scratch", "field": "bw"}}),
                    ),
                    (
                        "count",
                        3,
                        json!({"receiver_unknown": true, "receiver_field": {"owner": "Scratch", "field": "nodes", "element": true}}),
                    ),
                    // Another package's type, by its import path.
                    (
                        "flush",
                        4,
                        json!({"receiver_unknown": true, "receiver_field": {"owner": "Scratch", "field": "ext"}}),
                    ),
                    (
                        "close",
                        6,
                        json!({"receiver_type": "bitWriter", "receiver_package": "example.com/m/huff0"}),
                    ),
                    // A path of fields, to an element.
                    (
                        "close",
                        8,
                        json!({"receiver_unknown": true, "receiver_field": {"owner": "Scratch", "field": "state.jobs", "element": true}}),
                    ),
                    // A module outside the repository.
                    (
                        "close",
                        7,
                        json!({"receiver_type": "bitWriter", "receiver_package": "example.org/x"}),
                    ),
                    // The slice itself has no methods.
                    (
                        "count",
                        5,
                        json!({"receiver_unknown": true, "receiver_field": {"owner": "Scratch", "field": "nodes"}}),
                    ),
                ],
            ),
        ),
        (
            "flate/fast.go",
            "example.com/m/flate",
            vec![
                go_struct("fastGen", "flate/fast.go", json!({"field_types": {}})),
                go(method_node("addBlock", "flate/fast.go", "fastGen")),
                go_struct("other", "flate/fast.go", json!({"field_types": {}})),
                go(method_node("addBlock", "flate/fast.go", "other")),
                go_struct(
                    "fastEncL1",
                    "flate/fast.go",
                    json!({"field_types": {}, "embedded": ["fastGen"]}),
                ),
                go_struct(
                    "fastEncL2",
                    "flate/fast.go",
                    json!({"field_types": {}, "embedded": ["fastGen", "other"]}),
                ),
                go_struct(
                    "fastEncL3",
                    "flate/fast.go",
                    json!({"field_types": {"addBlock": ""}, "embedded": ["fastGen"]}),
                ),
                go_struct(
                    "fastEncL4",
                    "flate/fast.go",
                    json!({"field_types": {}, "embedded": ["fastEncL1", "huff0.Writer"]}),
                ),
            ],
            vec![],
        ),
        (
            "flate/level.go",
            "example.com/m/flate",
            vec![go(function_node("run", "flate/level.go"))],
            caller(
                "flate/level.go",
                &[
                    ("addBlock", 2, json!({"receiver_type": "fastEncL1"})),
                    // Two at one depth: ambiguous.
                    ("addBlock", 3, json!({"receiver_type": "fastEncL2"})),
                    // A field of the name comes first.
                    ("addBlock", 4, json!({"receiver_type": "fastEncL3"})),
                    // Another package's embedded type may declare it first.
                    ("addBlock", 5, json!({"receiver_type": "fastEncL4"})),
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
    let at = |file: &str, line: i64, target: &str| (file.to_string(), line, target.to_string());
    assert_eq!(
        targets,
        vec![
            at("flate/level.go", 2, "flate/fast.go::fastGen.addBlock"),
            at("flate/level.go", 3, "addBlock"),
            at("flate/level.go", 4, "addBlock"),
            at("flate/level.go", 5, "addBlock"),
            at("fse/compress.go", 2, "fse/bitwriter.go::bitWriter.close"),
            at("fse/compress.go", 3, "fse/bitwriter.go::nodeElt.count"),
            at("fse/compress.go", 4, "huff0/h.go::bitWriter.flush"),
            at("fse/compress.go", 5, "count"),
            at("fse/compress.go", 6, "huff0/h.go::bitWriter.close"),
            at("fse/compress.go", 7, "close"),
            at("fse/compress.go", 8, "fse/bitwriter.go::bitWriter.close"),
        ]
    );
    drop(stmt);
    let _ = std::fs::remove_file(path);
}
