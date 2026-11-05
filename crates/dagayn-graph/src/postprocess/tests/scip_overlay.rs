use super::*;

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn one_index(path: &std::path::Path, authoritative: bool) -> ScipIndexInput {
    ScipIndexInput {
        path: path.to_path_buf(),
        prefix: String::new(),
        authoritative,
    }
}

#[test]
fn a_scip_index_settles_calls_by_the_reference_at_their_name() {
    use protobuf::{EnumOrUnknown, Message};
    use scip::types::{Document, Index, Occurrence, PositionEncoding};

    let root = std::env::temp_dir().join(format!("dagayn-scip-overlay-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    let a = "fn helper() {}\n\
             fn run(cell: Cell, items: Vec<u8>) {\n    \
             helper();\n    \
             cell.borrow().snapshot();\n    \
             items.push(1);\n    \
             items.len();\n    \
             let f = |x| x;\n    \
             f(1);\n\
             }\n";
    let b = "impl Bindings {\n    fn snapshot(&self) {}\n}\n";
    std::fs::write(root.join("src/a.rs"), a).unwrap();
    std::fs::write(root.join("src/b.rs"), b).unwrap();
    std::fs::write(root.join("src/c.rs"), "fn edited() { helper(); }\n").unwrap();

    let col = |line: &str, name: &str| line.find(name).unwrap() as i32;
    let lines = a.lines().collect::<Vec<_>>();
    let reference = |row: usize, name: &str, symbol: &str| Occurrence {
        range: vec![
            row as i32,
            col(lines[row], name),
            col(lines[row], name) + name.len() as i32,
        ],
        symbol: symbol.to_string(),
        ..Occurrence::default()
    };
    let definition = |row: i32, start: i32, name: &str, symbol: &str| Occurrence {
        range: vec![row, start, start + name.len() as i32],
        symbol: symbol.to_string(),
        symbol_roles: 1,
        ..Occurrence::default()
    };
    let helper = "rust-analyzer cargo demo 0.1.0 helper().";
    let snapshot = "rust-analyzer cargo demo 0.1.0 b/Bindings#snapshot().";
    let index = Index {
        documents: vec![
            Document {
                relative_path: "src/a.rs".to_string(),
                position_encoding: EnumOrUnknown::new(
                    PositionEncoding::UTF8CodeUnitOffsetFromLineStart,
                ),
                occurrences: vec![
                    definition(0, 3, "helper", helper),
                    reference(2, "helper", helper),
                    reference(3, "snapshot", snapshot),
                    reference(4, "push", "rust-analyzer cargo alloc 1.0.0 vec/Vec#push()."),
                    reference(5, "len", "rust-analyzer cargo alloc 1.0.0 vec/Vec#len()."),
                    reference(7, "f", "local 3"),
                ],
                ..Document::default()
            },
            Document {
                relative_path: "src/b.rs".to_string(),
                occurrences: vec![definition(1, 7, "snapshot", snapshot)],
                ..Document::default()
            },
            Document {
                relative_path: "src/c.rs".to_string(),
                occurrences: vec![Occurrence {
                    range: vec![0, 14, 20],
                    symbol: helper.to_string(),
                    ..Occurrence::default()
                }],
                ..Document::default()
            },
        ],
        ..Index::default()
    };
    let index_path = root.join("index.scip");
    std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();

    let path = temp_db("scip-overlay");
    let mut store = GraphStore::open(&path).expect("open");
    let call = |target: &str, line: i64, extra: Value| EdgeInput {
        extra,
        ..edge("CALLS", "src/a.rs::run", target, "src/a.rs", line)
    };
    let ranged = |node: NodeInput, start: i64, end: i64| NodeInput {
        line_start: start,
        line_end: end,
        ..node
    };
    store
        .store_file_nodes_edges(
            "src/a.rs",
            &[
                file_node("src/a.rs"),
                ranged(function_node("helper", "src/a.rs"), 1, 1),
                ranged(function_node("run", "src/a.rs"), 2, 9),
            ],
            &[
                call("src/a.rs::helper", 3, json!({})),
                call("snapshot", 4, json!({"receiver_unknown": true})),
                call(
                    "std",
                    5,
                    json!({"external": true, "external_package": "std",
                                     "stdlib": true, "external_symbol": "push",
                                     "inferred_from": "observed_method"}),
                ),
                call("len", 6, json!({"receiver_unknown": true})),
                call("f", 8, json!({})),
            ],
            &sha256_hex(a.as_bytes()),
            0,
        )
        .expect("store a");
    store
        .store_file_nodes_edges(
            "src/b.rs",
            &[
                file_node("src/b.rs"),
                ranged(class_node("Bindings", "src/b.rs"), 1, 3),
                ranged(method_node("snapshot", "src/b.rs", "Bindings"), 2, 2),
            ],
            &[],
            &sha256_hex(b.as_bytes()),
            0,
        )
        .expect("store b");
    store
        .store_file_nodes_edges(
            "src/c.rs",
            &[file_node("src/c.rs")],
            &[edge("CALLS", "src/c.rs", "helper", "src/c.rs", 1)],
            "hash-of-an-older-version",
            0,
        )
        .expect("store c");
    // Low and medium tiers as resolution leaves them.
    store
        .conn
        .execute(
            "UPDATE edges SET confidence_tier = CASE line WHEN 5 THEN 'MEDIUM' \
             WHEN 3 THEN 'EXTRACTED' ELSE 'LOW' END WHERE kind = 'CALLS'",
            [],
        )
        .unwrap();

    let stats = store
        .apply_scip_overlays(&[one_index(&index_path, true)], &root)
        .unwrap()
        .remove(0);
    assert_eq!(
        stats,
        ScipOverlayStats {
            documents: 3,
            calls: 6,
            stale_skipped: 1,
            confirmed: 2,
            rewritten_to_node: 1,
            rewritten_to_package: 1,
            marked_local: 1,
            ..ScipOverlayStats::default()
        }
    );
    // Endpoint demotion, later in the pipeline, keeps what the index settled.
    store.demote_unresolved_endpoint_edges().unwrap();
    let rows = store
        .conn
        .prepare(
            "SELECT line, target_qualified, confidence_tier, \
                    json_extract(extra, '$.resolved_by'), \
                    json_extract(extra, '$.external_symbol'), \
                    json_extract(extra, '$.callee_local') \
             FROM edges WHERE kind = 'CALLS' AND file_path = 'src/a.rs' ORDER BY line",
        )
        .unwrap()
        .query_map([], |row| {
            <(
                i64,
                String,
                String,
                Option<String>,
                Option<String>,
                Option<i64>,
            )>::try_from(row)
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    let scip = Some("scip".to_string());
    assert_eq!(
        rows,
        vec![
            (
                3,
                "src/a.rs::helper".into(),
                "EXTRACTED".into(),
                scip.clone(),
                None,
                None
            ),
            (
                4,
                "src/b.rs::Bindings.snapshot".into(),
                "HIGH".into(),
                scip.clone(),
                None,
                None
            ),
            (
                5,
                "std".into(),
                "HIGH".into(),
                scip.clone(),
                Some("push".into()),
                None
            ),
            (
                6,
                "std".into(),
                "HIGH".into(),
                scip,
                Some("Vec::len".into()),
                None
            ),
            (8, "f".into(), "LOW".into(), None, None, Some(1)),
        ]
    );
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn calls_a_scip_index_settled_teach_observed_methods_nothing() {
    // Two `Database.exec` calls SCIP typed; an untyped `cp.exec()` stays.
    let path = temp_db("observed-scip");
    let mut store = GraphStore::open(&path).expect("open");
    let typed = |line: i64| EdgeInput {
        extra: json!({"external": true, "external_package": "better-sqlite3",
                      "external_symbol": "Database.exec", "resolved_by": "scip"}),
        ..edge("CALLS", "src/a.ts::run", "better-sqlite3", "src/a.ts", line)
    };
    store
        .store_file_nodes_edges(
            "src/a.ts",
            &[file_node("src/a.ts"), function_node("run", "src/a.ts")],
            &[
                typed(1),
                typed(2),
                EdgeInput {
                    extra: json!({"receiver_unknown": true}),
                    ..edge("CALLS", "src/a.ts::run", "exec", "src/a.ts", 3)
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
            "SELECT target_qualified FROM edges WHERE kind = 'CALLS' AND line = 3",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target, "exec");
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_callee_a_scip_index_found_local_is_not_bound_by_name() {
    // `reinitializeFolder(folder)` calls a callback parameter, not the
    // file's `reinitializeFolder`.
    let path = temp_db("callee-local");
    let mut store = GraphStore::open(&path).expect("open");
    store
        .store_file_nodes_edges(
            "src/a.ts",
            &[
                file_node("src/a.ts"),
                function_node("register", "src/a.ts"),
                function_node("reinitializeFolder", "src/a.ts"),
            ],
            &[EdgeInput {
                extra: json!({"callee_local": true}),
                ..edge(
                    "CALLS",
                    "src/a.ts::register",
                    "reinitializeFolder",
                    "src/a.ts",
                    3,
                )
            }],
            "",
            0,
        )
        .expect("store");
    store.resolve_bare_call_targets().unwrap();
    let target: String = store
        .conn
        .query_row(
            "SELECT target_qualified FROM edges WHERE kind = 'CALLS'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target, "reinitializeFolder");
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_filling_scip_index_keeps_resolutions_answer() {
    use protobuf::{EnumOrUnknown, Message};
    use scip::types::{Document, Index, Occurrence, PositionEncoding};

    // `helper()` is bound to `Other.helper` by resolution and to the
    // top-level `helper` by the index: a filling index leaves it. `save()`
    // is unresolved: filled.
    let root = std::env::temp_dir().join(format!("dagayn-scip-fill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let source = "def helper(): pass\ndef save(): pass\ndef run(s):\n    helper()\n    s.save()\n";
    std::fs::write(root.join("a.py"), source).unwrap();
    let lines = source.lines().collect::<Vec<_>>();
    let occurrence = |row: usize, name: &str, symbol: &str, roles: i32| {
        let start = lines[row].find(name).unwrap() as i32;
        Occurrence {
            range: vec![row as i32, start, start + name.len() as i32],
            symbol: symbol.to_string(),
            symbol_roles: roles,
            ..Occurrence::default()
        }
    };
    let helper = "scip-python python demo 0.1.0 `a`/helper().";
    let save = "scip-python python demo 0.1.0 `a`/Store#save().";
    let index = Index {
        documents: vec![Document {
            relative_path: "a.py".to_string(),
            position_encoding: EnumOrUnknown::new(
                PositionEncoding::UTF8CodeUnitOffsetFromLineStart,
            ),
            occurrences: vec![
                occurrence(0, "helper", helper, 1),
                occurrence(1, "save", save, 1),
                occurrence(3, "helper", helper, 0),
                occurrence(4, "save", save, 0),
            ],
            ..Document::default()
        }],
        ..Index::default()
    };
    let index_path = root.join("index.scip");
    std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();

    let path = temp_db("scip-fill");
    let mut store = GraphStore::open(&path).expect("open");
    let ranged = |name: &str, line: i64, end: i64| NodeInput {
        line_start: line,
        line_end: end,
        ..function_node(name, "a.py")
    };
    store
        .store_file_nodes_edges(
            "a.py",
            &[
                file_node("a.py"),
                ranged("helper", 1, 1),
                ranged("save", 2, 2),
                ranged("run", 3, 5),
                NodeInput {
                    line_start: 7,
                    line_end: 8,
                    ..method_node("helper", "a.py", "Other")
                },
            ],
            &[
                edge("CALLS", "a.py::run", "a.py::Other.helper", "a.py", 4),
                EdgeInput {
                    extra: json!({"receiver_unknown": true}),
                    ..edge("CALLS", "a.py::run", "save", "a.py", 5)
                },
            ],
            &sha256_hex(source.as_bytes()),
            0,
        )
        .expect("store");
    let stats = store
        .apply_scip_overlays(&[one_index(&index_path, false)], &root)
        .unwrap()
        .remove(0);
    assert_eq!((stats.left, stats.rewritten_to_node), (1, 1));
    let targets = store
        .conn
        .prepare("SELECT target_qualified FROM edges WHERE kind = 'CALLS' ORDER BY line")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        targets,
        vec!["a.py::Other.helper".to_string(), "a.py::save".to_string()]
    );
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_name_on_the_edges_line_without_a_reference_is_not_taken_from_a_later_line() {
    use protobuf::{EnumOrUnknown, Message};
    use scip::types::{Document, Index, Occurrence, PositionEncoding};

    // scip-go has no occurrence for the builtin `len(in)`; `h.len()` two
    // lines down is another call. A chain's `.count()` is still found on a
    // line after the edge's, past the variable `count` the edge's line binds.
    let root = std::env::temp_dir().join(format!("dagayn-scip-window-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let go = "package z\n\
              func (h hcode) len() int { return 0 }\n\
              func run(in []byte, h hcode) int {\n\
              \tn := len(in)\n\
              \tn++\n\
              \treturn n + h.len()\n\
              }\n";
    let rust = "fn total(items: Vec<u8>) -> usize {\n    \
                let count = items\n        \
                .iter()\n        \
                .count();\n    \
                count\n\
                }\n";
    std::fs::write(root.join("a.go"), go).unwrap();
    std::fs::write(root.join("b.rs"), rust).unwrap();
    let occurrence = |source: &str, row: usize, name: &str, symbol: &str, roles: i32| {
        let line = source.lines().nth(row).unwrap();
        let start = line.rfind(name).unwrap() as i32;
        Occurrence {
            range: vec![row as i32, start, start + name.len() as i32],
            symbol: symbol.to_string(),
            symbol_roles: roles,
            ..Occurrence::default()
        }
    };
    let hcode_len = "scip-go gomod example.com/z . `example.com/z`/hcode#len().";
    let count = "rust-analyzer cargo core 1.0.0 iter/traits/iterator/Iterator#count().";
    let utf8 = EnumOrUnknown::new(PositionEncoding::UTF8CodeUnitOffsetFromLineStart);
    let index = Index {
        documents: vec![
            Document {
                relative_path: "a.go".to_string(),
                position_encoding: utf8,
                occurrences: vec![
                    occurrence(go, 1, "len", hcode_len, 1),
                    occurrence(go, 5, "len", hcode_len, 0),
                ],
                ..Document::default()
            },
            Document {
                relative_path: "b.rs".to_string(),
                position_encoding: utf8,
                occurrences: vec![occurrence(rust, 3, "count", count, 0)],
                ..Document::default()
            },
        ],
        ..Index::default()
    };
    let index_path = root.join("index.scip");
    std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();

    let path = temp_db("scip-window");
    let mut store = GraphStore::open(&path).expect("open");
    let ranged = |node: NodeInput, start: i64, end: i64| NodeInput {
        line_start: start,
        line_end: end,
        ..node
    };
    store
        .store_file_nodes_edges(
            "a.go",
            &[
                file_node("a.go"),
                ranged(method_node("len", "a.go", "hcode"), 2, 2),
                ranged(function_node("run", "a.go"), 3, 7),
            ],
            &[EdgeInput {
                extra: json!({"external": true, "external_package": "builtin",
                              "stdlib": true, "external_symbol": "len"}),
                ..edge("CALLS", "a.go::run", "builtin", "a.go", 4)
            }],
            &sha256_hex(go.as_bytes()),
            0,
        )
        .expect("store a.go");
    store
        .store_file_nodes_edges(
            "b.rs",
            &[
                file_node("b.rs"),
                ranged(function_node("total", "b.rs"), 1, 6),
            ],
            &[EdgeInput {
                extra: json!({"external": true, "external_package": "std",
                              "stdlib": true, "external_symbol": "Iterator::count"}),
                ..edge("CALLS", "b.rs::total", "std", "b.rs", 2)
            }],
            &sha256_hex(rust.as_bytes()),
            0,
        )
        .expect("store b.rs");
    let stats = store
        .apply_scip_overlays(&[one_index(&index_path, true)], &root)
        .unwrap()
        .remove(0);
    assert_eq!((stats.unmatched, stats.confirmed), (1, 1), "{stats:?}");
    let targets = store
        .conn
        .prepare(
            "SELECT file_path, target_qualified, json_extract(extra, '$.resolved_by') \
             FROM edges WHERE kind = 'CALLS' ORDER BY file_path",
        )
        .unwrap()
        .query_map([], |row| <(String, String, Option<String>)>::try_from(row))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        targets,
        vec![
            ("a.go".to_string(), "builtin".to_string(), None),
            (
                "b.rs".to_string(),
                "std".to_string(),
                Some("scip".to_string())
            ),
        ]
    );
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_definition_one_index_holds_settles_another_indexs_reference() {
    use protobuf::{EnumOrUnknown, Message};
    use scip::types::{Document, Index, Occurrence, PositionEncoding};

    let root = std::env::temp_dir().join(format!("dagayn-scip-pooled-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("pkg/src")).unwrap();
    std::fs::create_dir_all(root.join("e2e")).unwrap();
    let x = "export const make = () => 1;\nexport const trim = () => 2;\n";
    let lib = "make();\ntrim();\n";
    std::fs::write(root.join("pkg/src/x.ts"), x).unwrap();
    std::fs::write(root.join("e2e/lib.ts"), lib).unwrap();
    let occurrence = |row: i32, start: i32, name: &str, symbol: &str, roles: i32| Occurrence {
        range: vec![row, start, start + name.len() as i32],
        symbol: symbol.to_string(),
        symbol_roles: roles,
        ..Occurrence::default()
    };
    let make = "scip-typescript npm @acme/pkg 0.1.0 src/`x.ts`/make.";
    let trim = "scip-typescript npm typescript 5.9.3 lib/`lib.es5.d.ts`/String#trim().";
    let utf16 = EnumOrUnknown::new(PositionEncoding::UTF16CodeUnitOffsetFromLineStart);
    let write = |name: &str, index: Index| {
        let path = root.join(name);
        std::fs::write(&path, index.write_to_bytes().unwrap()).unwrap();
        path
    };
    let pkg_index = write(
        "pkg.scip",
        Index {
            documents: vec![Document {
                relative_path: "src/x.ts".to_string(),
                position_encoding: utf16,
                occurrences: vec![occurrence(0, 13, "make", make, 1)],
                ..Document::default()
            }],
            ..Index::default()
        },
    );
    let e2e_index = write(
        "e2e.scip",
        Index {
            documents: vec![Document {
                relative_path: "lib.ts".to_string(),
                position_encoding: utf16,
                occurrences: vec![
                    occurrence(0, 0, "make", make, 0),
                    occurrence(1, 0, "trim", trim, 0),
                ],
                ..Document::default()
            }],
            ..Index::default()
        },
    );

    let path = temp_db("scip-pooled");
    let mut store = GraphStore::open(&path).expect("open");
    let at = |node: NodeInput, line: i64| NodeInput {
        line_start: line,
        line_end: line,
        ..node
    };
    store
        .store_file_nodes_edges(
            "pkg/src/x.ts",
            &[
                file_node("pkg/src/x.ts"),
                at(function_node("make", "pkg/src/x.ts"), 1),
                at(function_node("trim", "pkg/src/x.ts"), 2),
            ],
            &[],
            &sha256_hex(x.as_bytes()),
            0,
        )
        .expect("store x");
    store
        .store_file_nodes_edges(
            "e2e/lib.ts",
            &[file_node("e2e/lib.ts")],
            &[
                edge("CALLS", "e2e/lib.ts", "pkg/src/x.ts::make", "e2e/lib.ts", 1),
                edge("CALLS", "e2e/lib.ts", "pkg/src/x.ts::trim", "e2e/lib.ts", 2),
            ],
            &sha256_hex(lib.as_bytes()),
            0,
        )
        .expect("store lib");

    let stats = store
        .apply_scip_overlays(
            &[
                ScipIndexInput {
                    path: pkg_index,
                    prefix: "pkg/".to_string(),
                    authoritative: true,
                },
                ScipIndexInput {
                    path: e2e_index,
                    prefix: "e2e/".to_string(),
                    authoritative: true,
                },
            ],
            &root,
        )
        .unwrap();
    assert_eq!(stats.len(), 2);
    assert_eq!(
        stats[1],
        ScipOverlayStats {
            documents: 1,
            calls: 2,
            confirmed: 1,
            rewritten_to_package: 1,
            ..ScipOverlayStats::default()
        }
    );
    let targets = store
        .conn
        .prepare(
            "SELECT target_qualified, json_extract(extra, '$.resolved_by') \
             FROM edges WHERE kind = 'CALLS' ORDER BY line",
        )
        .unwrap()
        .query_map([], |row| <(String, Option<String>)>::try_from(row))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    let scip = Some("scip".to_string());
    assert_eq!(
        targets,
        vec![
            ("pkg/src/x.ts::make".to_string(), scip.clone()),
            ("globalThis".to_string(), scip),
        ]
    );
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir_all(root);
}
