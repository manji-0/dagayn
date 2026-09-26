use super::*;

#[test]
fn store_file_batch_populates_edge_target_name() {
    let path = temp_db("edge-target-name");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let file = NodeInput {
        kind: "File".to_string(),
        name: "app.py".to_string(),
        file_path: "app.py".to_string(),
        line_start: 1,
        line_end: 1,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let caller = NodeInput {
        kind: "Function".to_string(),
        name: "main".to_string(),
        file_path: "app.py".to_string(),
        line_start: 1,
        line_end: 3,
        language: "python".to_string(),
        parent_name: None,
        params: Some("()".to_string()),
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    store
        .store_file_batch(&[(
            "app.py".to_string(),
            vec![file, caller],
            vec![EdgeInput {
                kind: "CALLS".to_string(),
                source: "app.py::main".to_string(),
                target: "app.py::helper".to_string(),
                file_path: "app.py".to_string(),
                line: 2,
                extra: Value::Object(Default::default()),
            }],
            "hash".to_string(),
            0,
        )])
        .expect("store file batch");

    let (target_qualified, target_name): (String, String) = store
        .conn
        .query_row(
            "SELECT target_qualified, target_name FROM edges WHERE kind = 'CALLS'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(target_qualified, "app.py::helper");
    assert_eq!(target_name, "helper");
    let _ = std::fs::remove_file(path);
}

#[test]
fn atomically_replaces_file_data() {
    let path = temp_db("replace");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let file = NodeInput {
        kind: "File".to_string(),
        name: "app.py".to_string(),
        file_path: "app.py".to_string(),
        line_start: 1,
        line_end: 1,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let func = NodeInput {
        kind: "Function".to_string(),
        name: "main".to_string(),
        file_path: "app.py".to_string(),
        line_start: 1,
        line_end: 3,
        language: "python".to_string(),
        parent_name: None,
        params: Some("()".to_string()),
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    store
        .store_file_nodes_edges("app.py", &[file, func], &[], "hash1", 0)
        .unwrap();
    store
        .store_file_nodes_edges("app.py", &[], &[], "hash2", 0)
        .unwrap();
    assert!(store.get_all_files().unwrap().is_empty());
    let _ = std::fs::remove_file(path);
}

#[test]
fn stores_file_batch_in_one_transaction() {
    let path = temp_db("batch");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let file_a = NodeInput {
        kind: "File".to_string(),
        name: "a.py".to_string(),
        file_path: "a.py".to_string(),
        line_start: 1,
        line_end: 1,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let file_b = NodeInput {
        kind: "File".to_string(),
        name: "b.py".to_string(),
        file_path: "b.py".to_string(),
        line_start: 1,
        line_end: 1,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };

    store
        .store_file_batch(&[
            (
                "a.py".to_string(),
                vec![file_a],
                vec![],
                "hash-a".to_string(),
                0,
            ),
            (
                "b.py".to_string(),
                vec![file_b],
                vec![],
                "hash-b".to_string(),
                0,
            ),
        ])
        .unwrap();

    let mut files = store.get_all_files().unwrap();
    files.sort();
    assert_eq!(files, vec!["a.py", "b.py"]);
    let _ = std::fs::remove_file(path);
}

#[test]
fn stores_file_batch_edge_metadata_once_per_call_site() {
    let path = temp_db("batch-edge-meta");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let file = NodeInput {
        kind: "File".to_string(),
        name: "app.py".to_string(),
        file_path: "app.py".to_string(),
        line_start: 1,
        line_end: 10,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let caller = NodeInput {
        kind: "Function".to_string(),
        name: "caller".to_string(),
        file_path: "app.py".to_string(),
        line_start: 2,
        line_end: 4,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let callee = NodeInput {
        kind: "Function".to_string(),
        name: "callee".to_string(),
        file_path: "app.py".to_string(),
        line_start: 6,
        line_end: 8,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let edge = EdgeInput {
        kind: "CROSS_ARTIFACT".to_string(),
        source: "app.py::caller".to_string(),
        target: "app.py::callee".to_string(),
        file_path: "app.py".to_string(),
        line: 3,
        extra: json!({"confidence": 0.42, "confidence_tier": "low", "role": "contract"}),
    };

    let tx = write_tx(&mut store.conn).unwrap();
    store_file_batch_tx(
        &tx,
        &[(
            "app.py".to_string(),
            vec![file, caller, callee],
            vec![edge.clone(), edge],
            "hash".to_string(),
            0,
        )],
        false,
        false,
    )
    .unwrap();
    tx.commit().unwrap();

    let edges = store.get_edges_by_source("app.py::caller").unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].confidence, 0.42);
    assert_eq!(edges[0].confidence_tier, ConfidenceTier::Low);
    assert_eq!(edges[0].extra["role"], "contract");
    let _ = std::fs::remove_file(path);
}

#[test]
fn stores_compact_json_batch() {
    let path = temp_db("json-batch");
    let mut store = GraphStore::open(&path).expect("open graph store");
    store
        .store_file_batch_json(
            r#"[
                    [
                        "app.py",
                        [["File","app.py","app.py",1,1,"python",null,null,null,null,false,{}]],
                        [],
                        "hash",
                        123
                    ]
                ]"#,
        )
        .unwrap();

    assert_eq!(
        store.get_file_hashes(&["app.py".to_string()]).unwrap()["app.py"],
        "hash"
    );
    assert_eq!(
        store.get_file_meta_map().unwrap()["app.py"],
        ("hash".to_string(), 123)
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn source_excerpt_cache_matches_per_node_reads() {
    use crate::helpers::{SourceCache, read_node_source_excerpt};

    let dir = std::env::temp_dir().join(format!("dagayn-excerpt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("a.py"),
        "def one():\n    return 1\n\ndef two():\n    return 2\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("doc.md"),
        "# Top\nintro\n## Sub\nbody\n# Next\nmore\n",
    )
    .unwrap();
    let wide = "é".repeat(3000);
    std::fs::write(dir.join("wide.txt"), format!("{wide}\n{wide}\n")).unwrap();

    let mut cache = SourceCache::default();
    let mut excerpt = |kind: &str, file: &str, start: i64, end: i64| {
        read_node_source_excerpt(&mut cache, Some(&dir), kind, file, Some(start), Some(end))
    };
    assert_eq!(
        excerpt("Function", "a.py", 1, 2),
        "def one():\n    return 1"
    );
    assert_eq!(
        excerpt("Function", "a.py", 4, 5),
        "def two():\n    return 2"
    );
    assert_eq!(
        excerpt("DocSection", "doc.md", 1, 1),
        "# Top\nintro\n## Sub\nbody"
    );
    assert_eq!(excerpt("DocSection", "doc.md", 3, 3), "## Sub\nbody");
    // Back to a file read earlier: the cache must reload it, not reuse doc.md.
    assert_eq!(excerpt("Function", "a.py", 99, 99), "    return 2");
    // Truncation counts characters, not bytes, and spans the line separator.
    let truncated = excerpt("File", "wide.txt", 1, 2);
    assert_eq!(truncated.chars().count(), 4096);
    assert_eq!(truncated, format!("{wide}\n{}", "é".repeat(1095)));
    assert_eq!(excerpt("Function", "missing.py", 1, 1), "");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn bulk_load_keeps_file_path_indexes_and_sets_fts_watermark_on_finish() {
    let path = temp_db("bulk-load-file-indexes");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let batch = r#"[
        ["app.py",
         [["File","app.py","app.py",1,4,"python",null,null,null,null,false,{}],
          ["Function","run","app.py",2,4,"python",null,null,null,null,false,{}]],
         [["CALLS","app.py::run","app.py::helper","app.py",3,{}]],
         "hash",
         1]
    ]"#;

    store.begin_bulk_load().unwrap();
    let index_names = |store: &GraphStore| -> Vec<String> {
        let mut stmt = store
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index'")
            .unwrap();
        stmt.query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap()
    };
    let during = index_names(&store);
    assert!(during.contains(&"idx_nodes_file".to_string()));
    assert!(during.contains(&"idx_edges_file".to_string()));
    assert!(!during.contains(&"idx_edges_source_kind".to_string()));

    // Re-storing the same file must replace, not duplicate, its FTS rows.
    store.store_file_batch_json(batch).unwrap();
    store.store_file_batch_json(batch).unwrap();
    store.finish_bulk_load().unwrap();

    assert!(index_names(&store).contains(&"idx_edges_source_kind".to_string()));
    let fts_rows: i64 = store
        .conn
        .query_row("SELECT count(*) FROM nodes_fts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(fts_rows, 2);
    assert_eq!(
        store
            .get_metadata(crate::fts_sync::FTS_COUNT_KEY)
            .unwrap()
            .as_deref(),
        Some("2")
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn stores_compact_json_batch_edge_metadata() {
    let path = temp_db("json-batch-edge-meta");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let compact: Vec<RawCompactFileBatchItem> = serde_json::from_str(
        r#"[
                    [
                        "app.py",
                        [
                            ["File","app.py","app.py",1,10,"python",null,null,null,null,false,{}],
                            ["Function","caller","app.py",2,4,"python",null,null,null,null,false,{}],
                            ["Function","callee","app.py",6,8,"python",null,null,null,null,false,{}]
                        ],
                        [
                            [
                                "CROSS_ARTIFACT",
                                "app.py::caller",
                                "app.py::callee",
                                "app.py",
                                3,
                                {"confidence":0.77,"confidence_tier":"medium","role":"contract"}
                            ]
                        ],
                        "hash",
                        123
                    ]
                ]"#,
    )
    .unwrap();
    let tx = write_tx(&mut store.conn).unwrap();
    store_raw_compact_file_batch_tx(&tx, &compact, false, false).unwrap();
    tx.commit().unwrap();

    let edges = store.get_edges_by_source("app.py::caller").unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].confidence, 0.77);
    assert_eq!(edges[0].confidence_tier, ConfidenceTier::Medium);
    assert_eq!(edges[0].extra["role"], "contract");
    let _ = std::fs::remove_file(path);
}

#[test]
fn computes_missing_signatures() {
    let path = temp_db("signatures");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let class = NodeInput {
        kind: "Class".to_string(),
        name: "Service".to_string(),
        file_path: "app.py".to_string(),
        line_start: 1,
        line_end: 10,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let func = NodeInput {
        kind: "Function".to_string(),
        name: "handle".to_string(),
        file_path: "app.py".to_string(),
        line_start: 3,
        line_end: 5,
        language: "python".to_string(),
        parent_name: Some("Service".to_string()),
        params: Some("request".to_string()),
        return_type: Some("Response".to_string()),
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };

    store
        .store_file_nodes_edges("app.py", &[class, func], &[], "hash", 0)
        .unwrap();

    assert_eq!(store.compute_missing_signatures().unwrap(), 2);
    assert_eq!(store.compute_missing_signatures().unwrap(), 0);
    let signatures = store
        .conn
        .prepare("SELECT qualified_name, signature FROM nodes ORDER BY qualified_name")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        signatures,
        vec![
            ("app.py::Service".to_string(), "class Service".to_string()),
            (
                "app.py::Service.handle".to_string(),
                "def handle(request) -> Response".to_string(),
            ),
        ]
    );
    let _ = std::fs::remove_file(path);
}
