use super::*;

use serde_json::json;
use std::path::PathBuf;

fn temp_db(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("dagayn-rust-{}-{}.db", name, std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn creates_current_schema() {
    let path = temp_db("schema");
    let store = GraphStore::open(&path).expect("open graph store");
    assert_eq!(store.schema_version().unwrap(), LATEST_VERSION);
    assert!(table_exists(&store.conn, "nodes_fts").unwrap());
    assert!(table_exists(&store.conn, "hub_scores").unwrap());
    assert!(table_exists(&store.conn, "bridge_scores").unwrap());
    assert!(has_column(&store.conn, "edges", "confidence_tier").unwrap());
    let _ = std::fs::remove_file(path);
}

#[test]
fn migrate_v14_creates_centrality_tables_for_existing_db() {
    let path = temp_db("migrate-v14");
    {
        let conn = rusqlite::Connection::open(&path).expect("open sqlite db");
        conn.execute_batch(SCHEMA_SQL).unwrap();
        conn.execute("DROP TABLE hub_scores", []).unwrap();
        conn.execute("DROP TABLE bridge_scores", []).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES ('schema_version', '13')",
            [],
        )
        .unwrap();
    }

    let store = GraphStore::open(&path).expect("open graph store");

    assert_eq!(store.schema_version().unwrap(), LATEST_VERSION);
    assert!(table_exists(&store.conn, "hub_scores").unwrap());
    assert!(table_exists(&store.conn, "bridge_scores").unwrap());
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
fn remove_files_data_tx_clears_stale_centrality_scores() {
    let path = temp_db("remove-centrality");
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
        name: "caller".to_string(),
        file_path: "app.py".to_string(),
        line_start: 2,
        line_end: 3,
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
        line_start: 5,
        line_end: 6,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let edge = EdgeInput {
        kind: "CALLS".to_string(),
        source: "app.py::caller".to_string(),
        target: "app.py::callee".to_string(),
        file_path: "app.py".to_string(),
        line: 3,
        extra: Value::Object(Default::default()),
    };
    store
        .store_file_nodes_edges("app.py", &[file, caller, callee], &[edge], "hash", 0)
        .unwrap();
    store.persist_centrality_scores().unwrap();

    store.remove_files_data(&["app.py".to_string()]).unwrap();

    let hub_count: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM hub_scores", [], |row| row.get(0))
        .unwrap();
    let bridge_count: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM bridge_scores", [], |row| row.get(0))
        .unwrap();
    assert_eq!(hub_count, 0);
    assert_eq!(bridge_count, 0);
    let _ = std::fs::remove_file(path);
}

#[test]
fn deterministic_centrality_sample_is_not_sorted_prefix() {
    let nodes = (0..6000)
        .map(|idx| format!("node_{idx:04}"))
        .collect::<Vec<_>>();

    let sample = deterministic_centrality_sample(&nodes, 500);

    assert_eq!(sample.len(), 500);
    assert_ne!(sample, nodes[..500]);
    assert!(sample.iter().all(|node| nodes.contains(node)));
}

#[test]
fn approximate_betweenness_samples_connected_regions() {
    let graph_nodes = (0..6000)
        .map(|idx| format!("node_{idx:04}"))
        .collect::<std::collections::HashSet<_>>();
    let mut adjacency = std::collections::HashMap::<String, Vec<String>>::new();
    for idx in 500..5999 {
        adjacency
            .entry(format!("node_{idx:04}"))
            .or_default()
            .push(format!("node_{:04}", idx + 1));
    }

    let scores = betweenness_centrality(&graph_nodes, &adjacency);

    assert!(scores.values().any(|score| *score > 0.0));
}

#[test]
fn generates_suggested_questions_json_from_native_analysis_unit() {
    let path = temp_db("suggested-questions");
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
    let entry = NodeInput {
        kind: "Function".to_string(),
        name: "entry".to_string(),
        file_path: "app.py".to_string(),
        line_start: 1,
        line_end: 3,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let middle = NodeInput {
        kind: "Function".to_string(),
        name: "middle".to_string(),
        file_path: "app.py".to_string(),
        line_start: 4,
        line_end: 6,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let leaf = NodeInput {
        kind: "Function".to_string(),
        name: "leaf".to_string(),
        file_path: "app.py".to_string(),
        line_start: 7,
        line_end: 9,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let edges = [
        EdgeInput {
            kind: "CALLS".to_string(),
            source: "app.py::entry".to_string(),
            target: "app.py::middle".to_string(),
            file_path: "app.py".to_string(),
            line: 2,
            extra: Value::Object(Default::default()),
        },
        EdgeInput {
            kind: "CALLS".to_string(),
            source: "app.py::middle".to_string(),
            target: "app.py::leaf".to_string(),
            file_path: "app.py".to_string(),
            line: 5,
            extra: Value::Object(Default::default()),
        },
    ];
    store
        .store_file_nodes_edges("app.py", &[file, entry, middle, leaf], &edges, "hash", 0)
        .unwrap();
    store.persist_centrality_scores().unwrap();

    let questions: Vec<Value> =
        serde_json::from_str(&store.generate_suggested_questions_json().unwrap()).unwrap();

    assert!(!questions.is_empty());
    assert_eq!(questions[0]["category"], "bridge_node");
    assert_eq!(questions[0]["priority"], "high");
    let _ = std::fs::remove_file(path);
}

#[test]
fn analysis_question_rows_read_nodes_edges_communities_and_persisted_scores() {
    let path = temp_db("question-rows");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let file = NodeInput {
        kind: "File".to_string(),
        name: "app.py".to_string(),
        file_path: "app.py".to_string(),
        line_start: 1,
        line_end: 20,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let entry = NodeInput {
        kind: "Function".to_string(),
        name: "entry".to_string(),
        file_path: "app.py".to_string(),
        line_start: 2,
        line_end: 5,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let middle = NodeInput {
        kind: "Function".to_string(),
        name: "middle".to_string(),
        file_path: "app.py".to_string(),
        line_start: 7,
        line_end: 11,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let leaf = NodeInput {
        kind: "Function".to_string(),
        name: "leaf".to_string(),
        file_path: "app.py".to_string(),
        line_start: 13,
        line_end: 16,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let test_leaf = NodeInput {
        kind: "Test".to_string(),
        name: "test_leaf".to_string(),
        file_path: "test_app.py".to_string(),
        line_start: 1,
        line_end: 4,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: true,
        extra: Value::Object(Default::default()),
    };
    let consumer = NodeInput {
        kind: "Function".to_string(),
        name: "run".to_string(),
        file_path: "consumer.py".to_string(),
        line_start: 1,
        line_end: 4,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let edges = [
        EdgeInput {
            kind: "CALLS".to_string(),
            source: "app.py::entry".to_string(),
            target: "app.py::middle".to_string(),
            file_path: "app.py".to_string(),
            line: 3,
            extra: Value::Object(Default::default()),
        },
        EdgeInput {
            kind: "CALLS".to_string(),
            source: "app.py::middle".to_string(),
            target: "app.py::leaf".to_string(),
            file_path: "app.py".to_string(),
            line: 8,
            extra: Value::Object(Default::default()),
        },
        EdgeInput {
            kind: "TESTED_BY".to_string(),
            source: "app.py::leaf".to_string(),
            target: "test_app.py::test_leaf".to_string(),
            file_path: "test_app.py".to_string(),
            line: 2,
            extra: Value::Object(Default::default()),
        },
        EdgeInput {
            kind: "CALLS".to_string(),
            source: "consumer.py::run".to_string(),
            target: "app.py::entry".to_string(),
            file_path: "consumer.py".to_string(),
            line: 2,
            extra: Value::Object(Default::default()),
        },
    ];
    store
        .store_file_batch(&[(
            "app.py".to_string(),
            vec![file, entry, middle, leaf, test_leaf, consumer],
            edges.to_vec(),
            "hash".to_string(),
            0,
        )])
        .unwrap();
    store
        .conn
        .execute(
            "INSERT INTO communities (name, level, cohesion, size, dominant_language) \
             VALUES ('app-community', 0, 0.9, 3, 'python')",
            [],
        )
        .unwrap();
    store
        .conn
        .execute(
            "INSERT INTO communities (name, level, cohesion, size, dominant_language) \
             VALUES ('leaf-community', 0, 1.0, 1, 'python')",
            [],
        )
        .unwrap();
    let community_id: i64 = store
        .conn
        .query_row(
            "SELECT id FROM communities WHERE name = 'app-community'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE nodes SET community_id = ? WHERE qualified_name IN \
             ('app.py::entry', 'app.py::middle')",
            [community_id],
        )
        .unwrap();
    let leaf_community_id: i64 = store
        .conn
        .query_row(
            "SELECT id FROM communities WHERE name = 'leaf-community'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE nodes SET community_id = ? WHERE qualified_name = 'app.py::leaf'",
            [leaf_community_id],
        )
        .unwrap();
    store.persist_centrality_scores().unwrap();

    let community_ids = store.get_all_node_community_ids().unwrap();
    assert_eq!(community_ids["app.py::entry"], community_id);
    assert_eq!(community_ids["app.py::middle"], community_id);
    assert_eq!(community_ids["app.py::leaf"], leaf_community_id);
    let members_by_community = store
        .get_community_member_qns_by_ids(&[community_id, leaf_community_id])
        .unwrap();
    assert_eq!(
        members_by_community[&community_id],
        vec!["app.py::entry".to_string(), "app.py::middle".to_string()]
    );
    assert_eq!(
        members_by_community[&leaf_community_id],
        vec!["app.py::leaf".to_string()]
    );
    assert_eq!(
        store.get_test_targets_for_source("app.py::leaf").unwrap(),
        vec!["test_app.py::test_leaf".to_string()]
    );
    assert_eq!(
        store
            .get_direct_dependents(&["app.py".to_string()])
            .unwrap(),
        vec!["consumer.py".to_string()]
    );

    let question_nodes = store.get_question_nodes().unwrap();
    let question_node_names = question_nodes
        .iter()
        .map(|node| node.qualified_name.as_str())
        .collect::<HashSet<_>>();
    assert!(question_node_names.contains("app.py::entry"));
    assert!(question_node_names.contains("app.py::middle"));
    assert!(question_node_names.contains("app.py::leaf"));
    assert!(question_node_names.contains("test_app.py::test_leaf"));
    assert!(question_nodes
        .iter()
        .any(|node| { node.qualified_name == "test_app.py::test_leaf" && node.is_test }));
    assert!(!question_node_names.contains("app.py"));

    let question_edges = store.get_question_edges().unwrap();
    assert_eq!(question_edges.len(), 4);
    assert!(question_edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source_qualified == "app.py::entry"
            && edge.target_qualified == "app.py::middle"
    }));
    assert!(question_edges.iter().any(|edge| {
        edge.kind == "TESTED_BY"
            && edge.source_qualified == "app.py::leaf"
            && edge.target_qualified == "test_app.py::test_leaf"
    }));
    let mut degree = HashMap::<String, i64>::new();
    for edge in &question_edges {
        *degree.entry(edge.source_qualified.clone()).or_insert(0) += 1;
        *degree.entry(edge.target_qualified.clone()).or_insert(0) += 1;
    }
    let surprising = store.find_surprising_connection_questions(
        5,
        &question_nodes,
        &question_edges,
        &community_ids,
        &degree,
    );
    assert!(surprising.iter().any(|item| {
        item["category"] == "surprising_connection" && item["target"] == "app.py::middle"
    }));
    let question_gaps = store
        .find_question_gap_inputs(
            &question_nodes,
            &community_ids,
            &degree,
            &HashSet::from(["app.py::leaf".to_string()]),
        )
        .unwrap();
    assert!(question_gaps
        .thin_communities
        .iter()
        .any(|community| { community.id == leaf_community_id && community.size == 1 }));
    assert!(question_gaps.untested_hotspots.is_empty());

    let bridge_rows = store.get_persisted_bridge_rows(5).unwrap();
    assert!(bridge_rows
        .iter()
        .any(|row| { row.name == "middle" && row.qualified_name == "app.py::middle" }));
    let hub_rows = store.get_persisted_hub_rows(5).unwrap();
    assert!(hub_rows.iter().any(|row| {
        row.name == "middle" && row.qualified_name == "app.py::middle" && row.total_degree == 2
    }));
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
fn rebuilds_fts_index() {
    let path = temp_db("fts");
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
        name: "calculate_total".to_string(),
        file_path: "app.py".to_string(),
        line_start: 3,
        line_end: 5,
        language: "python".to_string(),
        parent_name: None,
        params: Some("()".to_string()),
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };

    store
        .store_file_nodes_edges("app.py", &[file, func], &[], "hash", 0)
        .unwrap();
    store
        .conn
        .execute("DROP TABLE IF EXISTS nodes_fts", [])
        .unwrap();

    assert_eq!(store.rebuild_fts_index().unwrap(), 2);
    let hit: String = store
        .conn
        .query_row(
            "SELECT name FROM nodes_fts WHERE name MATCH 'calculate*'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(hit, "calculate_total");
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

#[test]
fn resolves_markdown_artifact_refs() {
    let path = temp_db("markdown-refs");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let target = NodeInput {
        kind: "Class".to_string(),
        name: "BridgePattern".to_string(),
        file_path: "parser.py".to_string(),
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
    let edge = EdgeInput {
        kind: "CROSS_ARTIFACT".to_string(),
        source: "docs/spec.md::section".to_string(),
        target: "<unresolved:BridgePattern>".to_string(),
        file_path: "docs/spec.md".to_string(),
        line: 5,
        extra: json!({
            "relationship_role": "describes_symbol",
            "target_language": "unknown",
            "confidence": 0.2,
            "confidence_tier": "LOW",
            "original_symbol_name": "BridgePattern",
        }),
    };

    store
        .store_file_batch(&[(
            "parser.py".to_string(),
            vec![target],
            vec![edge],
            "hash".to_string(),
            0,
        )])
        .unwrap();

    assert_eq!(
        store.resolve_markdown_artifact_refs().unwrap(),
        (1, 0, 0, 0)
    );
    let row = store
        .conn
        .query_row(
            "SELECT target_qualified, confidence, confidence_tier, extra \
                 FROM edges WHERE kind = 'CROSS_ARTIFACT'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(row.0, "parser.py::BridgePattern");
    assert_eq!(row.1, 0.8);
    assert_eq!(row.2, "HIGH");
    let extra: Value = serde_json::from_str(&row.3).unwrap();
    assert!(extra.get("unresolved_target_name").is_none());
    assert_eq!(extra["original_symbol_name"], "BridgePattern");
    assert_eq!(extra["target_language"], "python");
    assert_eq!(extra["confidence"], 0.8);
    assert_eq!(
        store.resolve_markdown_artifact_refs().unwrap(),
        (0, 0, 0, 0)
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn prunes_unresolved_markdown_code_span_refs() {
    let path = temp_db("markdown-code-span-prune");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let edge = EdgeInput {
        kind: "CROSS_ARTIFACT".to_string(),
        source: "docs/spec.md::section".to_string(),
        target: "<unresolved:OrdinaryConcept>".to_string(),
        file_path: "docs/spec.md".to_string(),
        line: 5,
        extra: json!({
            "relationship_role": "describes_symbol",
            "bridge_kind": "documentation",
            "evidence_kind": "markdown_code_span",
            "evidence_source": "code_span",
            "target_language": "unknown",
            "confidence": 0.2,
            "confidence_tier": "LOW",
            "original_symbol_name": "OrdinaryConcept",
        }),
    };

    store
        .store_file_batch(&[(
            "docs/spec.md".to_string(),
            vec![],
            vec![edge],
            "hash".to_string(),
            0,
        )])
        .unwrap();

    assert_eq!(
        store.resolve_markdown_artifact_refs().unwrap(),
        (0, 1, 0, 0)
    );
    let count: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE kind = 'CROSS_ARTIFACT'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    let _ = std::fs::remove_file(path);
}

#[test]
fn computes_summary_tables() {
    let path = temp_db("summaries");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let file = NodeInput {
        kind: "File".to_string(),
        name: "auth.py".to_string(),
        file_path: "auth.py".to_string(),
        line_start: 1,
        line_end: 20,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let login = NodeInput {
        kind: "Function".to_string(),
        name: "login".to_string(),
        file_path: "auth.py".to_string(),
        line_start: 1,
        line_end: 5,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let check_token = NodeInput {
        kind: "Function".to_string(),
        name: "check_token".to_string(),
        file_path: "auth.py".to_string(),
        line_start: 6,
        line_end: 10,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let test_login = NodeInput {
        kind: "Test".to_string(),
        name: "test_login".to_string(),
        file_path: "auth.py".to_string(),
        line_start: 12,
        line_end: 15,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: true,
        extra: Value::Object(Default::default()),
    };
    let calls = EdgeInput {
        kind: "CALLS".to_string(),
        source: "auth.py::login".to_string(),
        target: "auth.py::check_token".to_string(),
        file_path: "auth.py".to_string(),
        line: 2,
        extra: Value::Object(Default::default()),
    };
    let tested_by = EdgeInput {
        kind: "TESTED_BY".to_string(),
        source: "auth.py::login".to_string(),
        target: "auth.py::test_login".to_string(),
        file_path: "auth.py".to_string(),
        line: 13,
        extra: Value::Object(Default::default()),
    };
    store
        .store_file_batch(&[(
            "auth.py".to_string(),
            vec![file, login, check_token, test_login],
            vec![calls, tested_by],
            "hash".to_string(),
            0,
        )])
        .unwrap();
    store
        .conn
        .execute(
            "INSERT INTO communities (name, level, cohesion, size, dominant_language) \
                 VALUES ('auth-cluster', 0, 1.0, 3, 'python')",
            [],
        )
        .unwrap();
    let community_id: i64 = store
        .conn
        .query_row(
            "SELECT id FROM communities WHERE name = 'auth-cluster'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    store
        .conn
        .execute("UPDATE nodes SET community_id = ?", [community_id])
        .unwrap();
    let login_id: i64 = store
        .conn
        .query_row(
            "SELECT id FROM nodes WHERE qualified_name = 'auth.py::login'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let token_id: i64 = store
        .conn
        .query_row(
            "SELECT id FROM nodes WHERE qualified_name = 'auth.py::check_token'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    store
        .conn
        .execute(
            "INSERT INTO flows \
                 (name, entry_point_id, depth, node_count, file_count, criticality, path_json) \
                 VALUES ('auth flow', ?, 2, 2, 1, 0.5, ?)",
            params![
                login_id,
                serde_json::to_string(&vec![login_id, token_id]).unwrap()
            ],
        )
        .unwrap();

    store.compute_summaries().unwrap();

    let community_row: (String, i64, String) = store
        .conn
        .query_row(
            "SELECT name, size, key_symbols FROM community_summaries",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(community_row.0, "auth-cluster");
    assert_eq!(community_row.1, 3);
    let key_symbols: Vec<String> = serde_json::from_str(&community_row.2).unwrap();
    assert_eq!(key_symbols[0], "login");

    let flow_path: String = store
        .conn
        .query_row("SELECT critical_path FROM flow_snapshots", [], |row| {
            row.get(0)
        })
        .unwrap();
    let flow_path: Vec<String> = serde_json::from_str(&flow_path).unwrap();
    assert_eq!(flow_path, vec!["auth.py::login", "auth.py::check_token"]);

    let risk_row: (String, i64, String, i64, f64) = store
        .conn
        .query_row(
            "SELECT qualified_name, caller_count, test_coverage, security_relevant, risk_score \
                 FROM risk_index WHERE qualified_name = 'auth.py::check_token'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(risk_row.0, "auth.py::check_token");
    assert_eq!(risk_row.1, 1);
    assert_eq!(risk_row.2, "untested");
    assert_eq!(risk_row.3, 1);
    assert_eq!(risk_row.4, 0.7);
    let _ = std::fs::remove_file(path);
}

#[test]
fn stores_flows_and_reads_flow_inputs() {
    let path = temp_db("flows");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let entry = NodeInput {
        kind: "Function".to_string(),
        name: "entry".to_string(),
        file_path: "app.py".to_string(),
        line_start: 1,
        line_end: 5,
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
        line_start: 7,
        line_end: 10,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let test_callee = NodeInput {
        kind: "Test".to_string(),
        name: "test_callee".to_string(),
        file_path: "test_app.py".to_string(),
        line_start: 1,
        line_end: 5,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: true,
        extra: Value::Object(Default::default()),
    };
    let edge = EdgeInput {
        kind: "CALLS".to_string(),
        source: "app.py::entry".to_string(),
        target: "app.py::callee".to_string(),
        file_path: "app.py".to_string(),
        line: 2,
        extra: Value::Object(Default::default()),
    };
    let tested_by = EdgeInput {
        kind: "TESTED_BY".to_string(),
        source: "app.py::callee".to_string(),
        target: "test_app.py::test_callee".to_string(),
        file_path: "test_app.py".to_string(),
        line: 2,
        extra: Value::Object(Default::default()),
    };
    store
        .store_file_batch(&[(
            "app.py".to_string(),
            vec![entry, callee, test_callee],
            vec![edge, tested_by],
            "hash".to_string(),
            0,
        )])
        .unwrap();

    assert_eq!(
        store.get_files_matching("test_app.py").unwrap(),
        vec!["test_app.py"]
    );
    let targets = store.get_all_call_targets(false).unwrap();
    assert_eq!(targets, HashSet::from(["app.py::callee".to_string()]));
    let nodes = store
        .get_nodes_by_kind(&["Function".to_string()], None)
        .unwrap();
    assert_eq!(nodes.len(), 2);
    let stats = store.get_stats().unwrap();
    assert_eq!(stats.total_nodes, 3);
    assert_eq!(stats.total_edges, 2);
    assert_eq!(stats.nodes_by_kind["Function"], 2);
    assert_eq!(stats.nodes_by_kind["Test"], 1);
    assert_eq!(stats.edges_by_kind["CALLS"], 1);
    assert_eq!(stats.edges_by_kind["TESTED_BY"], 1);
    assert_eq!(stats.files_count, 0);
    assert_eq!(stats.languages, vec!["python".to_string()]);
    let (calls_out, tested_by) = store.get_flow_edge_data().unwrap();
    assert_eq!(calls_out["app.py::entry"], vec!["app.py::callee"]);
    assert_eq!(tested_by, HashSet::from(["app.py::callee".to_string()]));

    let entry_id = store.get_node("app.py::entry").unwrap().unwrap().id;
    let callee_id = store.get_node("app.py::callee").unwrap().unwrap().id;
    let flows = vec![FlowInput {
        name: "entry".to_string(),
        entry_point_id: entry_id,
        depth: 1,
        node_count: 2,
        file_count: 1,
        criticality: 0.25,
        path: vec![entry_id, callee_id],
    }];
    assert_eq!(store.store_flows(&flows).unwrap(), 1);
    assert_eq!(
        store
            .conn
            .query_row("SELECT COUNT(*) FROM flows", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .conn
            .query_row("SELECT COUNT(*) FROM flow_memberships", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        2
    );
    assert_eq!(store.count_flow_memberships(callee_id).unwrap(), 1);
    let nodes_by_id = store.get_nodes_by_ids(&[entry_id, callee_id]).unwrap();
    assert_eq!(nodes_by_id[&entry_id].qualified_name, "app.py::entry");
    assert_eq!(nodes_by_id[&callee_id].qualified_name, "app.py::callee");
    let nodes_by_qn = store
        .get_nodes_by_qualified_names(&[
            "app.py::entry".to_string(),
            "app.py::callee".to_string(),
            "missing.py::none".to_string(),
        ])
        .unwrap();
    assert_eq!(nodes_by_qn["app.py::entry"].id, entry_id);
    assert_eq!(nodes_by_qn["app.py::callee"].id, callee_id);
    assert!(!nodes_by_qn.contains_key("missing.py::none"));
    let membership_counts = store
        .count_flow_memberships_for_nodes(&[entry_id, callee_id])
        .unwrap();
    assert_eq!(membership_counts[&entry_id], 1);
    assert_eq!(membership_counts[&callee_id], 1);
    assert_eq!(
        store.get_flow_criticalities_for_node(callee_id).unwrap(),
        vec![0.25]
    );
    let flow_criticalities = store
        .get_flow_criticalities_for_nodes(&[entry_id, callee_id])
        .unwrap();
    assert_eq!(flow_criticalities[&entry_id], vec![0.25]);
    assert_eq!(flow_criticalities[&callee_id], vec![0.25]);
    assert_eq!(store.get_node_community_id(callee_id).unwrap(), None);
    let community_ids = store
        .get_community_ids_by_node_ids(&[entry_id, callee_id])
        .unwrap();
    assert_eq!(community_ids[&entry_id], None);
    assert_eq!(community_ids[&callee_id], None);
    let direct_tests = store.get_transitive_tests("app.py::callee", 1).unwrap();
    assert_eq!(direct_tests.len(), 1);
    assert_eq!(direct_tests[0]["name"], "test_callee");
    assert_eq!(direct_tests[0]["indirect"], false);
    let indirect_tests = store.get_transitive_tests("app.py::entry", 1).unwrap();
    assert_eq!(indirect_tests.len(), 1);
    assert_eq!(indirect_tests[0]["name"], "test_callee");
    assert_eq!(indirect_tests[0]["indirect"], true);
    assert_eq!(store.store_flows(&[]).unwrap(), 0);
    assert_eq!(
        store
            .conn
            .query_row("SELECT COUNT(*) FROM flows", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );

    assert_eq!(store.store_flows(&flows).unwrap(), 1);
    let flows_json: Vec<Value> =
        serde_json::from_str(&store.get_flows_json("criticality", 50).unwrap()).unwrap();
    assert_eq!(flows_json.len(), 1);
    assert_eq!(flows_json[0]["name"], "entry");
    let flow_id = flows_json[0]["id"].as_i64().unwrap();
    let flow_json: Value = serde_json::from_str(
        &store
            .get_flow_by_id_json(flow_id)
            .unwrap()
            .expect("flow exists"),
    )
    .unwrap();
    assert_eq!(flow_json["steps"].as_array().unwrap().len(), 2);
    let affected: Vec<Value> = serde_json::from_str(
        &store
            .get_affected_flows_json(&["app.py".to_string()])
            .unwrap(),
    )
    .unwrap();
    assert_eq!(affected.len(), 1);
    let analysis: Value = serde_json::from_str(
        &store
            .analyze_changes_json(&["app.py".to_string()], None)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(analysis["risk_score"], json!(0.55));
    assert_eq!(analysis["changed_functions"].as_array().unwrap().len(), 2);
    assert_eq!(analysis["affected_flows"].as_array().unwrap().len(), 1);
    assert_eq!(analysis["test_gaps"].as_array().unwrap().len(), 1);
    store
        .conn
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    store
        .conn
        .execute(
            "INSERT INTO flow_snapshots \
             (flow_id, name, entry_point, critical_path, criticality, node_count, file_count) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![flow_id, "entry", "app.py::entry", "[]", 0.25, 2, 1],
        )
        .unwrap();
    let deleted_entry_points = store
        .delete_affected_flows(&["app.py".to_string()])
        .unwrap();
    assert_eq!(deleted_entry_points, vec![entry_id]);
    assert_eq!(
        store
            .conn
            .query_row("SELECT COUNT(*) FROM flows", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .conn
            .query_row("SELECT COUNT(*) FROM flow_snapshots", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .insert_flows_json(&serde_json::to_string(&flows).unwrap())
            .unwrap(),
        1
    );
    assert!(store
        .delete_affected_flows(&["missing.py".to_string()])
        .unwrap()
        .is_empty());
    assert_eq!(
        store.get_node_kind_by_id(entry_id).unwrap().as_deref(),
        Some("Function")
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn stores_and_reads_communities() {
    let path = temp_db("communities");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let node = NodeInput {
        kind: "Function".to_string(),
        name: "login".to_string(),
        file_path: "auth.py".to_string(),
        line_start: 1,
        line_end: 5,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    store
        .store_file_batch(&[(
            "auth.py".to_string(),
            vec![node],
            vec![],
            "hash".to_string(),
            0,
        )])
        .unwrap();
    let payload = serde_json::to_string(&vec![CommunityInput {
        name: "auth-cluster".to_string(),
        level: 0,
        cohesion: 0.75,
        size: 1,
        dominant_language: "python".to_string(),
        description: "Auth functions".to_string(),
        members: vec!["auth.py::login".to_string()],
    }])
    .unwrap();

    assert_eq!(store.store_communities_json(&payload).unwrap(), 1);
    let communities: Vec<Value> =
        serde_json::from_str(&store.get_communities_json("size", 0).unwrap()).unwrap();
    assert_eq!(communities.len(), 1);
    assert_eq!(communities[0]["name"], "auth-cluster");
    assert_eq!(communities[0]["members"], json!(["auth.py::login"]));
    let community_id = communities[0]["id"].as_i64().unwrap();
    let members = store.get_nodes_by_community_id(community_id).unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].qualified_name, "auth.py::login");
    let all_member_qns = store.get_all_community_member_qns().unwrap();
    assert_eq!(
        all_member_qns.get(&community_id),
        Some(&vec!["auth.py::login".to_string()])
    );
    let community_ids = store
        .get_community_ids_by_qualified_names(&[
            "auth.py::login".to_string(),
            "missing.py::none".to_string(),
        ])
        .unwrap();
    assert_eq!(
        community_ids.get("auth.py::login").copied().flatten(),
        Some(community_id)
    );
    assert!(!community_ids.contains_key("missing.py::none"));
    assert_eq!(
        store
            .count_affected_communities(&["auth.py".to_string()])
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .count_affected_communities(&["missing.py".to_string()])
            .unwrap(),
        0
    );
    let all_nodes = store.get_all_nodes_filtered(true).unwrap();
    assert_eq!(all_nodes.len(), 1);
    let _ = std::fs::remove_file(path);
}

#[test]
fn reads_nodes_and_edges_for_incremental_dependents() {
    let path = temp_db("read-api");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let source = NodeInput {
        kind: "File".to_string(),
        name: "src/lib.py".to_string(),
        file_path: "src/lib.py".to_string(),
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
    let function = NodeInput {
        kind: "Function".to_string(),
        name: "build".to_string(),
        file_path: "src/lib.py".to_string(),
        line_start: 3,
        line_end: 5,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"role": "entry"}),
    };
    let target = NodeInput {
        kind: "File".to_string(),
        name: "src/app.py".to_string(),
        file_path: "src/app.py".to_string(),
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
    let edge = EdgeInput {
        kind: "CALLS".to_string(),
        source: "src/app.py::main".to_string(),
        target: "src/lib.py::build".to_string(),
        file_path: "src/app.py".to_string(),
        line: 8,
        extra: json!({"confidence": 0.75, "confidence_tier": "HEURISTIC"}),
    };

    store
        .store_file_batch(&[
            (
                "src/lib.py".to_string(),
                vec![source, function],
                vec![],
                "hash-lib".to_string(),
                0,
            ),
            (
                "src/app.py".to_string(),
                vec![target],
                vec![edge.clone(), edge],
                "hash-app".to_string(),
                0,
            ),
        ])
        .unwrap();

    let nodes = store.get_nodes_by_file("src/lib.py").unwrap();
    assert_eq!(nodes.len(), 2);
    assert_eq!(
        store.get_node("src/lib.py::build").unwrap().unwrap().extra["role"],
        "entry"
    );

    let incoming = store.get_edges_by_target("src/lib.py::build").unwrap();
    assert_eq!(incoming.len(), 1);
    assert_eq!(incoming[0].file_path, "src/app.py");
    assert_eq!(incoming[0].confidence_tier, "HEURISTIC");

    let outgoing = store.get_edges_by_source("src/app.py::main").unwrap();
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0].confidence, 0.75);
    let _ = std::fs::remove_file(path);
}
