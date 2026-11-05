use super::*;

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
            <(i64, String, Option<String>, i64)>::try_from(row)
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
        .query_map([], |row| <(i64, String, Option<String>)>::try_from(row))
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
        .query_map([], |row| <(String, String)>::try_from(row))
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
        .query_map([], |row| <(i64, String)>::try_from(row))
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
        .query_map([], |row| <(i64, String, Option<String>)>::try_from(row))
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
