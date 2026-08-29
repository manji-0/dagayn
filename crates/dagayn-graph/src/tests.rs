use super::*;
use crate::helpers::write_tx;

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
    assert!(has_column(&store.conn, "edges", "target_name").unwrap());
    assert!(has_column(&store.conn, "flows", "kind").unwrap());
    assert!(has_column(&store.conn, "flows", "truncated").unwrap());
    assert!(has_column(&store.conn, "flows", "truncation_reason").unwrap());
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
fn ensure_edge_target_name_backfills_legacy_edges_without_column() {
    let path = temp_db("legacy-target-name");
    {
        let conn = rusqlite::Connection::open(&path).expect("open sqlite db");
        conn.execute_batch(
            r#"
            CREATE TABLE edges (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                source_qualified TEXT NOT NULL,
                target_qualified TEXT NOT NULL,
                file_path TEXT NOT NULL,
                line INTEGER DEFAULT 0,
                extra TEXT DEFAULT '{}',
                confidence REAL DEFAULT 1.0,
                confidence_tier TEXT DEFAULT 'EXTRACTED',
                updated_at REAL NOT NULL
            );
            CREATE TABLE metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            INSERT INTO edges
                (kind, source_qualified, target_qualified, file_path, line, extra,
                 confidence, confidence_tier, updated_at)
            VALUES
                ('CALLS', 'app.py::main', 'app.py::helper', 'app.py', 2, '{}', 1.0, 'EXTRACTED', 1.0),
                ('CALLS', 'worker.py::run', 'helper', 'worker.py', 4, '{}', 1.0, 'EXTRACTED', 1.0);
            INSERT INTO metadata (key, value) VALUES ('schema_version', '14');
            "#,
        )
        .unwrap();
    }

    let store = GraphStore::open(&path).expect("open graph store");
    assert!(has_column(&store.conn, "edges", "target_name").unwrap());

    let rows = store
        .conn
        .prepare("SELECT target_qualified, target_name FROM edges ORDER BY id")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .map(|row| row.unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        vec![
            ("app.py::helper".to_string(), "helper".to_string()),
            ("helper".to_string(), "helper".to_string()),
        ]
    );
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
fn remove_files_data_tx_keeps_other_files_centrality_scores() {
    let path = temp_db("remove-centrality-scoped");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let a_caller = NodeInput {
        kind: "Function".to_string(),
        name: "a_caller".to_string(),
        file_path: "a.py".to_string(),
        line_start: 1,
        line_end: 2,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let a_callee = NodeInput {
        kind: "Function".to_string(),
        name: "a_callee".to_string(),
        file_path: "a.py".to_string(),
        line_start: 3,
        line_end: 4,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let b_caller = NodeInput {
        kind: "Function".to_string(),
        name: "b_caller".to_string(),
        file_path: "b.py".to_string(),
        line_start: 1,
        line_end: 2,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let b_callee = NodeInput {
        kind: "Function".to_string(),
        name: "b_callee".to_string(),
        file_path: "b.py".to_string(),
        line_start: 3,
        line_end: 4,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let a_edge = EdgeInput {
        kind: "CALLS".to_string(),
        source: "a.py::a_caller".to_string(),
        target: "a.py::a_callee".to_string(),
        file_path: "a.py".to_string(),
        line: 1,
        extra: Value::Object(Default::default()),
    };
    let b_edge = EdgeInput {
        kind: "CALLS".to_string(),
        source: "b.py::b_caller".to_string(),
        target: "b.py::b_callee".to_string(),
        file_path: "b.py".to_string(),
        line: 1,
        extra: Value::Object(Default::default()),
    };
    store
        .store_file_nodes_edges("a.py", &[a_caller, a_callee], &[a_edge], "hash-a", 0)
        .unwrap();
    store
        .store_file_nodes_edges("b.py", &[b_caller, b_callee], &[b_edge], "hash-b", 0)
        .unwrap();
    store.persist_centrality_scores().unwrap();
    store.remove_files_data(&["a.py".to_string()]).unwrap();

    let remaining: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM hub_scores WHERE file_path = 'b.py'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(remaining > 0);
    let removed: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM hub_scores WHERE file_path = 'a.py'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(removed, 0);
    let _ = std::fs::remove_file(path);
}

#[test]
fn persist_centrality_batches_community_ids_onto_hub_rows() {
    let path = temp_db("centrality-community");
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
    store
        .conn
        .execute(
            "UPDATE nodes SET community_id = 7 WHERE qualified_name = 'app.py::caller'",
            [],
        )
        .unwrap();
    store.persist_centrality_scores().unwrap();

    let community_id: Option<i64> = store
        .conn
        .query_row(
            "SELECT community_id FROM hub_scores WHERE qualified_name = 'app.py::caller'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(community_id, Some(7));
    let _ = std::fs::remove_file(path);
}

#[test]
fn sync_fts_for_file_paths_does_not_drop_other_files() {
    let path = temp_db("fts-incr");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let alpha = NodeInput {
        kind: "Function".to_string(),
        name: "alpha_widget".to_string(),
        file_path: "src/a.py".to_string(),
        line_start: 1,
        line_end: 2,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let beta = NodeInput {
        kind: "Function".to_string(),
        name: "beta_gadget".to_string(),
        file_path: "src/b.py".to_string(),
        line_start: 1,
        line_end: 2,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    store
        .store_file_nodes_edges("src/a.py", &[alpha], &[], "hash-a", 0)
        .unwrap();
    store
        .store_file_nodes_edges("src/b.py", &[beta], &[], "hash-b", 0)
        .unwrap();
    let rebuilt = store.rebuild_fts_index().unwrap();
    assert!(rebuilt >= 2);

    let synced = store
        .sync_fts_for_file_paths(&["src/a.py".to_string()])
        .unwrap();
    assert!(synced >= 1);

    let fts_count: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM nodes_fts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(fts_count, rebuilt);

    let beta_hits: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM nodes_fts WHERE name MATCH 'beta_gadget'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(beta_hits, 1);
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
fn betweenness_sample_size_scales_with_sqrt_v() {
    assert_eq!(betweenness_sample_size(100), 100);
    assert_eq!(betweenness_sample_size(5000), 5000);
    assert_eq!(betweenness_sample_size(10_000), 500);
    let medium = betweenness_sample_size(6000);
    assert!(medium < 500);
    assert!(medium >= BETWEENNESS_SAMPLE_FLOOR);
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
fn betweenness_path_graph_ranks_middle_node_highest() {
    let graph_nodes = ["a.py::a", "b.py::b", "c.py::c"]
        .into_iter()
        .map(str::to_string)
        .collect::<std::collections::HashSet<_>>();
    let mut adjacency = std::collections::HashMap::<String, Vec<String>>::new();
    adjacency.insert("a.py::a".to_string(), vec!["b.py::b".to_string()]);
    adjacency.insert("b.py::b".to_string(), vec!["c.py::c".to_string()]);

    let scores = betweenness_centrality(&graph_nodes, &adjacency);

    let middle = scores["b.py::b"];
    assert!(middle > 0.0);
    assert!(middle > scores["a.py::a"]);
    assert!(middle > scores["c.py::c"]);
}

#[test]
fn helpers_make_qualified_hash_time_and_question_rows_have_stable_contracts() {
    let now = now_seconds().expect("system clock should produce unix timestamp");
    assert!(now > 0.0);
    assert_eq!(
        make_qualified_parts("File", "ignored", "src/lib.rs", Some("Parent")),
        "src/lib.rs"
    );
    assert_eq!(
        make_qualified_parts("Function", "run", "src/lib.rs", Some("Runner")),
        "src/lib.rs::Runner.run"
    );
    assert_eq!(
        make_qualified_parts("Function", "run", "src/lib.rs", None),
        "src/lib.rs::run"
    );
    assert_eq!(stable_fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(stable_fnv1a64(b"dagayn"), stable_fnv1a64(b"dagayn"));
    assert_ne!(stable_fnv1a64(b"dagayn"), stable_fnv1a64(b"Dagayn"));

    let bridge = PersistedBridgeRow {
        name: "middle".to_string(),
        qualified_name: "app.py::middle".to_string(),
    };
    let hub = PersistedHubRow {
        name: "entry".to_string(),
        qualified_name: "app.py::entry".to_string(),
        total_degree: 3,
    };
    assert_eq!(bridge.name, "middle");
    assert_eq!(bridge.qualified_name, "app.py::middle");
    assert_eq!(hub.name, "entry");
    assert_eq!(hub.qualified_name, "app.py::entry");
    assert_eq!(hub.total_degree, 3);

    let surprise = SurprisingQuestionInput {
        source_name: "entry".to_string(),
        source_qualified: "app.py::entry".to_string(),
        target_name: "leaf".to_string(),
        source_community: 1,
        target_community: 2,
        score: 4,
    };
    let thin_community = QuestionCommunity {
        id: 2,
        name: "leaf-community".to_string(),
        size: 1,
    };
    let hotspot = QuestionHotspot {
        name: "middle".to_string(),
        qualified_name: "app.py::middle".to_string(),
        degree: 5,
    };
    let gaps = QuestionGaps {
        thin_communities: vec![thin_community],
        untested_hotspots: vec![hotspot],
    };
    let question_node = QuestionNode {
        kind: "Function".to_string(),
        name: "middle".to_string(),
        qualified_name: "app.py::middle".to_string(),
        file_path: "app.py".to_string(),
        language: "python".to_string(),
        is_test: false,
    };
    let question_edge = QuestionEdge {
        kind: "CALLS".to_string(),
        source_qualified: "app.py::entry".to_string(),
        target_qualified: "app.py::middle".to_string(),
    };
    assert_eq!(surprise.source_name, "entry");
    assert_eq!(surprise.source_qualified, "app.py::entry");
    assert_eq!(surprise.target_name, "leaf");
    assert_eq!(surprise.source_community, 1);
    assert_eq!(surprise.target_community, 2);
    assert_eq!(surprise.score, 4);
    assert_eq!(gaps.thin_communities[0].id, 2);
    assert_eq!(gaps.thin_communities[0].name, "leaf-community");
    assert_eq!(gaps.thin_communities[0].size, 1);
    assert_eq!(gaps.untested_hotspots[0].name, "middle");
    assert_eq!(gaps.untested_hotspots[0].qualified_name, "app.py::middle");
    assert_eq!(gaps.untested_hotspots[0].degree, 5);
    assert_eq!(question_node.kind, "Function");
    assert_eq!(question_node.name, "middle");
    assert_eq!(question_node.qualified_name, "app.py::middle");
    assert_eq!(question_node.file_path, "app.py");
    assert_eq!(question_node.language, "python");
    assert!(!question_node.is_test);
    assert_eq!(question_edge.kind, "CALLS");
    assert_eq!(question_edge.source_qualified, "app.py::entry");
    assert_eq!(question_edge.target_qualified, "app.py::middle");

    assert_eq!(nearest_rank_percentile(&[], 0.95), 0);
    assert_eq!(nearest_rank_percentile(&[1, 5, 10, 20], 0.0), 1);
    assert_eq!(nearest_rank_percentile(&[1, 5, 10, 20], 0.5), 5);
    assert_eq!(nearest_rank_percentile(&[1, 5, 10, 20], 0.95), 20);
    assert_eq!(nearest_rank_percentile(&[1, 5, 10, 20], 1.0), 20);

    assert!(is_analysis_excluded_from_test_gap(&QuestionNode {
        kind: "Function".to_string(),
        name: "unit".to_string(),
        qualified_name: "tests/test_app.py::unit".to_string(),
        file_path: "tests/test_app.py".to_string(),
        language: "python".to_string(),
        is_test: false,
    }));
    assert!(is_analysis_excluded_from_test_gap(&QuestionNode {
        kind: "Function".to_string(),
        name: "unit".to_string(),
        qualified_name: "src/service.spec.ts::unit".to_string(),
        file_path: "src/service.spec.ts".to_string(),
        language: "typescript".to_string(),
        is_test: false,
    }));
    assert!(is_analysis_excluded_from_test_gap(&QuestionNode {
        kind: "Section".to_string(),
        name: "usage".to_string(),
        qualified_name: "README.md::usage".to_string(),
        file_path: "README.md".to_string(),
        language: "markdown".to_string(),
        is_test: false,
    }));
    assert!(!is_analysis_excluded_from_test_gap(&QuestionNode {
        kind: "Function".to_string(),
        name: "run".to_string(),
        qualified_name: "src/service.py::run".to_string(),
        file_path: "src/service.py".to_string(),
        language: "python".to_string(),
        is_test: false,
    }));

    assert_eq!(extra_json(&Value::Null).unwrap(), "{}");
    assert_eq!(extra_json(&json!({})).unwrap(), "{}");
    assert_eq!(
        extra_json(&json!({"confidence": 0.8, "confidence_tier": "HIGH"})).unwrap(),
        r#"{"confidence":0.8,"confidence_tier":"HIGH"}"#
    );
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
    store_raw_compact_file_batch_tx(&tx, &compact, false).unwrap();
    tx.commit().unwrap();

    let edges = store.get_edges_by_source("app.py::caller").unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].confidence, 0.77);
    assert_eq!(edges[0].confidence_tier, ConfidenceTier::Medium);
    assert_eq!(edges[0].extra["role"], "contract");
    let _ = std::fs::remove_file(path);
}

#[test]
fn reads_legacy_edges_with_default_confidence_metadata() {
    let path = temp_db("legacy-edge-meta");
    let store = GraphStore::open(&path).expect("open graph store");
    store
        .conn
        .execute(
            "INSERT INTO edges \
             (kind, source_qualified, target_qualified, file_path, line, extra, confidence, \
              confidence_tier, updated_at) \
             VALUES ('CALLS', 'app.py::caller', 'app.py::callee', 'app.py', 3, '{}', NULL, NULL, 1.0)",
            [],
        )
        .unwrap();

    let edge: GraphEdge = store
        .conn
        .query_row(
            "SELECT * FROM edges WHERE source_qualified = 'app.py::caller'",
            [],
            edge_from_row,
        )
        .unwrap();
    assert_eq!(edge.confidence, 1.0);
    assert_eq!(edge.confidence_tier, ConfidenceTier::Extracted);
    assert_eq!(
        edge_metadata_from_raw_extra(r#"{"confidence":0.33,"confidence_tier":"exact"}"#).unwrap(),
        (0.33, ConfidenceTier::Exact)
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
fn rebuilds_fts_index_segments_japanese_source() {
    let path = temp_db("fts-japanese");
    let source_root = {
        let mut root = std::env::temp_dir();
        root.push(format!(
            "dagayn-rust-fts-japanese-src-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    };
    std::fs::write(
        source_root.join("design.md"),
        "# 日本語検索\n\nGraphStoreで自然言語検索を行う。\n",
    )
    .unwrap();

    let mut store = GraphStore::open(&path).expect("open graph store");
    store
        .set_metadata("repo_root", source_root.to_string_lossy().as_ref())
        .unwrap();
    let doc = NodeInput {
        kind: "DocSection".to_string(),
        name: "japanese-search".to_string(),
        file_path: "design.md".to_string(),
        line_start: 1,
        line_end: 1,
        language: "markdown".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"display_name": "日本語検索"}),
    };

    store
        .store_file_nodes_edges("design.md", &[doc], &[], "hash", 0)
        .unwrap();

    assert_eq!(store.rebuild_fts_index().unwrap(), 1);
    let doc_text: String = store
        .conn
        .query_row(
            "SELECT doc_text FROM nodes_fts WHERE name = 'japanese-search'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(doc_text.contains("GraphStore"));
    assert!(doc_text.contains("自然"));
    assert!(doc_text.contains("言語"));

    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir_all(source_root);
}

#[test]
fn rebuilds_fts_index_includes_structured_code_reference_text() {
    let path = temp_db("fts-structured-code-reference");
    let source_root = {
        let mut root = std::env::temp_dir();
        root.push(format!(
            "dagayn-rust-fts-structured-src-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    };
    std::fs::write(
        source_root.join("service.py"),
        "def handle_failure(retry_budget):\n    retry_budget_exhausted = retry_budget <= 0\n    return retry_budget_exhausted\n",
    )
    .unwrap();

    let mut store = GraphStore::open(&path).expect("open graph store");
    store
        .set_metadata("repo_root", source_root.to_string_lossy().as_ref())
        .unwrap();
    let node = NodeInput {
        kind: "Function".to_string(),
        name: "handle_failure".to_string(),
        file_path: "service.py".to_string(),
        line_start: 1,
        line_end: 3,
        language: "python".to_string(),
        parent_name: None,
        params: Some("(retry_budget)".to_string()),
        return_type: Some("bool".to_string()),
        modifiers: None,
        is_test: false,
        extra: json!({"display_name": "Retry failure handler"}),
    };

    store
        .store_file_nodes_edges("service.py", &[node], &[], "hash", 0)
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE nodes SET signature = ? WHERE qualified_name = ?",
            params![
                "def handle_failure(retry_budget) -> bool",
                "service.py::handle_failure"
            ],
        )
        .unwrap();

    assert_eq!(store.rebuild_fts_index().unwrap(), 1);
    let doc_text: String = store
        .conn
        .query_row(
            "SELECT doc_text FROM nodes_fts WHERE name = 'handle_failure'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(doc_text.contains("kind: Function"));
    assert!(doc_text.contains("qualified: service.py::handle_failure"));
    assert!(doc_text.contains("signature: def handle_failure"));
    assert!(doc_text.contains("retry_budget_exhausted"));

    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir_all(source_root);
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
            "bridge_kind": "documentation",
            "evidence_kind": "markdown_code_span",
            "evidence_source": "code_span",
            "source_language": "markdown",
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
fn markdown_resolver_skips_terraform_handler_bridges() {
    let path = temp_db("markdown-skip-terraform-handler");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let decoy = NodeInput {
        kind: "Class".to_string(),
        name: "hello.main".to_string(),
        file_path: "app/decoy.py".to_string(),
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
        source: "infra/main.tf::resource.aws_lambda_function.auth".to_string(),
        target: "<unresolved:hello.main>".to_string(),
        file_path: "infra/main.tf".to_string(),
        line: 4,
        extra: json!({
            "relationship_role": "maps_entrypoint",
            "bridge_kind": "manifest_link",
            "evidence_kind": "config",
            "evidence_source": "handler",
            "source_language": "terraform",
            "target_language": "unknown",
            "confidence": 0.8,
            "confidence_tier": "HIGH",
            "original_symbol_name": "hello.main",
        }),
    };

    store
        .store_file_batch(&[(
            "infra/main.tf".to_string(),
            vec![decoy],
            vec![edge],
            "hash".to_string(),
            0,
        )])
        .unwrap();

    assert_eq!(
        store.resolve_markdown_artifact_refs().unwrap(),
        (0, 0, 0, 0)
    );
    let row = store
        .conn
        .query_row(
            "SELECT target_qualified FROM edges WHERE kind = 'CROSS_ARTIFACT'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    assert_eq!(row, "<unresolved:hello.main>");
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
    assert_eq!(stats.languages.as_ref(), ["python".to_string()].as_slice());
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
        path: vec![entry_id, callee_id].into(),
        ..Default::default()
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
fn analyze_changes_json_scores_range_limited_untested_security_changes() {
    let path = temp_db("analyze-changes");
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
    let auth_token = NodeInput {
        kind: "Function".to_string(),
        name: "auth_token".to_string(),
        file_path: "app.py".to_string(),
        line_start: 20,
        line_end: 30,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let helper = NodeInput {
        kind: "Function".to_string(),
        name: "helper".to_string(),
        file_path: "app.py".to_string(),
        line_start: 40,
        line_end: 45,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let test_helper = NodeInput {
        kind: "Test".to_string(),
        name: "test_helper".to_string(),
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
    let calls_auth = EdgeInput {
        kind: "CALLS".to_string(),
        source: "app.py::entry".to_string(),
        target: "app.py::auth_token".to_string(),
        file_path: "app.py".to_string(),
        line: 3,
        extra: Value::Object(Default::default()),
    };
    let tested_helper = EdgeInput {
        kind: "TESTED_BY".to_string(),
        source: "app.py::helper".to_string(),
        target: "test_app.py::test_helper".to_string(),
        file_path: "test_app.py".to_string(),
        line: 2,
        extra: Value::Object(Default::default()),
    };
    store
        .store_file_batch(&[(
            "app.py".to_string(),
            vec![entry, auth_token, helper, test_helper],
            vec![calls_auth, tested_helper],
            "hash".to_string(),
            0,
        )])
        .unwrap();
    let auth_id = store.get_node("app.py::auth_token").unwrap().unwrap().id;
    let flows = vec![FlowInput {
        name: "auth_token".to_string(),
        entry_point_id: auth_id,
        depth: 0,
        node_count: 1,
        file_count: 1,
        criticality: 0.25,
        path: vec![auth_id].into(),
        ..Default::default()
    }];
    assert_eq!(store.store_flows(&flows).unwrap(), 1);

    let changed_ranges = json!({"app.py": [[20, 22]]}).to_string();
    let analysis: Value = serde_json::from_str(
        &store
            .analyze_changes_json(&["app.py".to_string()], Some(&changed_ranges))
            .unwrap(),
    )
    .unwrap();

    assert_eq!(analysis["risk_score"], json!(0.8));
    assert_eq!(analysis["changed_functions"].as_array().unwrap().len(), 1);
    assert_eq!(
        analysis["changed_functions"][0]["qualified_name"],
        json!("app.py::auth_token")
    );
    assert_eq!(analysis["changed_functions"][0]["risk_score"], json!(0.8));
    assert_eq!(analysis["affected_flows"].as_array().unwrap().len(), 1);
    assert_eq!(analysis["test_gaps"].as_array().unwrap().len(), 1);
    assert_eq!(
        analysis["test_gaps"][0]["qualified_name"],
        json!("app.py::auth_token")
    );
    assert_eq!(analysis["review_priorities"].as_array().unwrap().len(), 1);
    assert_eq!(
        analysis["review_priorities"][0]["qualified_name"],
        json!("app.py::auth_token")
    );
    assert!(analysis["summary"]
        .as_str()
        .unwrap()
        .contains("1 test gap(s)"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn store_flows_json_replaces_existing_flows_from_serialized_input() {
    let path = temp_db("flows-json");
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
    store
        .store_file_batch(&[(
            "app.py".to_string(),
            vec![entry, callee],
            vec![],
            "hash".to_string(),
            0,
        )])
        .unwrap();
    let entry_id = store.get_node("app.py::entry").unwrap().unwrap().id;
    let callee_id = store.get_node("app.py::callee").unwrap().unwrap().id;
    store
        .store_flows(&[FlowInput {
            name: "old".to_string(),
            entry_point_id: callee_id,
            depth: 0,
            node_count: 1,
            file_count: 1,
            criticality: 0.1,
            path: vec![callee_id].into(),
            ..Default::default()
        }])
        .unwrap();

    let replacement = vec![FlowInput {
        name: "entry".to_string(),
        entry_point_id: entry_id,
        depth: 1,
        node_count: 2,
        file_count: 1,
        criticality: 0.75,
        path: vec![entry_id, callee_id].into(),
        ..Default::default()
    }];
    assert_eq!(
        store
            .store_flows_json(&serde_json::to_string(&replacement).unwrap())
            .unwrap(),
        1
    );

    let flows_json: Vec<Value> =
        serde_json::from_str(&store.get_flows_json("criticality", 10).unwrap()).unwrap();
    assert_eq!(flows_json.len(), 1);
    assert_eq!(flows_json[0]["name"], "entry");
    assert_eq!(flows_json[0]["criticality"], json!(0.75));
    let membership_count: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM flow_memberships", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(membership_count, 2);
    let _ = std::fs::remove_file(path);
}

#[test]
fn update_flow_criticalities_json_rewrites_scores() {
    let path = temp_db("flow-crit-update");
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
    store
        .store_file_batch(&[(
            "app.py".to_string(),
            vec![entry, callee],
            vec![],
            "hash".to_string(),
            0,
        )])
        .unwrap();
    let entry_id = store.get_node("app.py::entry").unwrap().unwrap().id;
    let callee_id = store.get_node("app.py::callee").unwrap().unwrap().id;
    store
        .store_flows(&[FlowInput {
            name: "entry".to_string(),
            entry_point_id: entry_id,
            depth: 1,
            node_count: 2,
            file_count: 1,
            criticality: 0.25,
            path: vec![entry_id, callee_id].into(),
            ..Default::default()
        }])
        .unwrap();
    let flow_id: i64 = store
        .conn
        .query_row("SELECT id FROM flows", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        store
            .update_flow_criticalities_json(&format!("[[{flow_id}, 0.085]]"))
            .unwrap(),
        1
    );
    let criticality: f64 = store
        .conn
        .query_row(
            "SELECT criticality FROM flows WHERE id = ?",
            [flow_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!((criticality - 0.085).abs() < 1e-9);
    let _ = std::fs::remove_file(path);
}

#[test]
fn flow_helpers_store_and_read_flow_rows_with_sanitized_json() {
    let path = temp_db("flow-helper-rows");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let entry = NodeInput {
        kind: "Function".to_string(),
        name: "entry<script>".to_string(),
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
    store
        .store_file_batch(&[(
            "app.py".to_string(),
            vec![entry, callee],
            vec![],
            "hash".to_string(),
            0,
        )])
        .unwrap();
    let entry_id = store.get_node("app.py::entry<script>").unwrap().unwrap().id;
    let callee_id = store.get_node("app.py::callee").unwrap().unwrap().id;
    {
        let tx = write_tx(&mut store.conn).unwrap();
        store_flows_tx(
            &tx,
            &[FlowInput {
                name: "entry<script>".to_string(),
                entry_point_id: entry_id,
                depth: 1,
                node_count: 2,
                file_count: 1,
                criticality: 0.4,
                path: vec![entry_id, callee_id].into(),
                ..Default::default()
            }],
        )
        .unwrap();
        tx.commit().unwrap();
    }

    let flow_json = store
        .conn
        .query_row("SELECT * FROM flows", [], flow_json_from_row)
        .unwrap();
    assert_eq!(flow_json["name"], "entry<script>");
    assert_eq!(flow_json["path"], json!([entry_id, callee_id]));

    let flow_value = store
        .conn
        .query_row("SELECT * FROM flows", [], flow_value_from_row)
        .unwrap();
    assert_eq!(flow_value.path_ids, vec![entry_id, callee_id]);
    assert_eq!(flow_value.value["criticality"], json!(0.4));

    let membership_count: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM flow_memberships", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(membership_count, 2);
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
        members: vec!["auth.py::login".to_string()].into(),
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
    assert_eq!(incoming[0].confidence_tier.as_str(), "EXTRACTED");

    let outgoing = store.get_edges_by_source("src/app.py::main").unwrap();
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0].confidence, 0.75);
    let _ = std::fs::remove_file(path);
}

fn flow_test_node(kind: &str, name: &str, file: &str) -> NodeInput {
    NodeInput {
        kind: kind.to_string(),
        name: name.to_string(),
        file_path: file.to_string(),
        line_start: 1,
        line_end: 10,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    }
}

fn flow_test_call(source: &str, target: &str, file: &str) -> EdgeInput {
    EdgeInput {
        kind: "CALLS".to_string(),
        source: source.to_string(),
        target: target.to_string(),
        file_path: file.to_string(),
        line: 2,
        extra: Value::Object(Default::default()),
    }
}

#[test]
fn incremental_trace_flows_uses_reverse_calls_for_new_callee() {
    let path = temp_db("incremental-reverse-calls");
    let mut store = GraphStore::open(&path).expect("open graph store");
    store
        .store_file_batch(&[(
            "a.py".to_string(),
            vec![
                flow_test_node("File", "a.py", "a.py"),
                flow_test_node("Function", "entry", "a.py"),
                flow_test_node("Function", "local", "a.py"),
            ],
            vec![
                flow_test_call("a.py::entry", "a.py::local", "a.py"),
                flow_test_call("a.py::entry", "b.py::new_helper", "a.py"),
            ],
            "hash-a".to_string(),
            0,
        )])
        .unwrap();
    store.rebuild_flows_json(15, false).unwrap();

    let before: i64 = store
        .conn
        .query_row(
            "SELECT node_count FROM flows f JOIN nodes n ON n.id = f.entry_point_id \
             WHERE n.qualified_name = 'a.py::entry'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(before, 2);

    store
        .store_file_batch(&[(
            "b.py".to_string(),
            vec![
                flow_test_node("File", "b.py", "b.py"),
                flow_test_node("Function", "new_helper", "b.py"),
            ],
            vec![],
            "hash-b".to_string(),
            0,
        )])
        .unwrap();

    let count = store
        .incremental_trace_flows(&["b.py".to_string()], 15)
        .unwrap();
    assert!(count >= 1);

    let members: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM flow_memberships fm \
             JOIN nodes n ON n.id = fm.node_id \
             WHERE n.qualified_name = 'b.py::new_helper'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(members > 0);
    let _ = std::fs::remove_file(path);
}

#[test]
fn community_edge_queries_are_region_local() {
    let path = temp_db("community-subgraph-edges");
    let mut store = GraphStore::open(&path).expect("open graph store");
    store
        .store_file_nodes_edges(
            "a.py",
            &[
                flow_test_node("Function", "a_caller", "a.py"),
                flow_test_node("Function", "a_callee", "a.py"),
            ],
            &[
                flow_test_call("a.py::a_caller", "a.py::a_callee", "a.py"),
                flow_test_call("a.py::a_caller", "b.py::b_callee", "a.py"),
            ],
            "hash-a",
            0,
        )
        .unwrap();
    store
        .store_file_nodes_edges(
            "b.py",
            &[
                flow_test_node("Function", "b_caller", "b.py"),
                flow_test_node("Function", "b_callee", "b.py"),
            ],
            &[flow_test_call("b.py::b_caller", "b.py::b_callee", "b.py")],
            "hash-b",
            0,
        )
        .unwrap();
    let payload = serde_json::to_string(&vec![
        CommunityInput {
            name: "cluster-a".to_string(),
            level: 0,
            cohesion: 1.0,
            size: 2,
            dominant_language: "python".to_string(),
            description: "a".to_string(),
            members: vec!["a.py::a_caller".to_string(), "a.py::a_callee".to_string()].into(),
        },
        CommunityInput {
            name: "cluster-b".to_string(),
            level: 0,
            cohesion: 1.0,
            size: 2,
            dominant_language: "python".to_string(),
            description: "b".to_string(),
            members: vec!["b.py::b_caller".to_string(), "b.py::b_callee".to_string()].into(),
        },
    ])
    .unwrap();
    store.store_communities_json(&payload).unwrap();
    let communities: Vec<Value> =
        serde_json::from_str(&store.get_communities_json("size", 0).unwrap()).unwrap();
    let id_a = communities
        .iter()
        .find(|community| community["name"] == "cluster-a")
        .and_then(|community| community["id"].as_i64())
        .unwrap();

    let within = store.get_edges_within_community_ids(&[id_a]).unwrap();
    assert!(
        within.iter().all(|edge| {
            edge.source_qualified.starts_with("a.py::")
                && edge.target_qualified.starts_with("a.py::")
        }),
        "induced community edges must stay inside the region"
    );
    assert!(within.iter().any(|edge| {
        edge.source_qualified == "a.py::a_caller" && edge.target_qualified == "a.py::a_callee"
    }));
    assert!(!within
        .iter()
        .any(|edge| edge.target_qualified == "b.py::b_callee"));

    let incident = store.get_edges_incident_to_community_ids(&[id_a]).unwrap();
    assert!(incident.iter().any(|edge| {
        edge.source_qualified == "a.py::a_caller" && edge.target_qualified == "b.py::b_callee"
    }));
    let _ = std::fs::remove_file(path);
}

#[test]
fn persist_centrality_scores_filtered_keeps_other_community_hubs() {
    let path = temp_db("centrality-region-sql");
    let mut store = GraphStore::open(&path).expect("open graph store");
    store
        .store_file_nodes_edges(
            "a.py",
            &[
                flow_test_node("Function", "a_caller", "a.py"),
                flow_test_node("Function", "a_callee", "a.py"),
            ],
            &[flow_test_call("a.py::a_caller", "a.py::a_callee", "a.py")],
            "hash-a",
            0,
        )
        .unwrap();
    store
        .store_file_nodes_edges(
            "b.py",
            &[
                flow_test_node("Function", "b_caller", "b.py"),
                flow_test_node("Function", "b_callee", "b.py"),
            ],
            &[flow_test_call("b.py::b_caller", "b.py::b_callee", "b.py")],
            "hash-b",
            0,
        )
        .unwrap();
    let payload = serde_json::to_string(&vec![
        CommunityInput {
            name: "cluster-a".to_string(),
            level: 0,
            cohesion: 1.0,
            size: 2,
            dominant_language: "python".to_string(),
            description: "a".to_string(),
            members: vec!["a.py::a_caller".to_string(), "a.py::a_callee".to_string()].into(),
        },
        CommunityInput {
            name: "cluster-b".to_string(),
            level: 0,
            cohesion: 1.0,
            size: 2,
            dominant_language: "python".to_string(),
            description: "b".to_string(),
            members: vec!["b.py::b_caller".to_string(), "b.py::b_callee".to_string()].into(),
        },
    ])
    .unwrap();
    store.store_communities_json(&payload).unwrap();
    store.persist_centrality_scores().unwrap();
    let before_b: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM hub_scores WHERE file_path = 'b.py'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(before_b > 0);

    store
        .persist_centrality_scores_filtered(Some(&["a.py".to_string()]))
        .unwrap();
    let after_b: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM hub_scores WHERE file_path = 'b.py'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(after_b, before_b);
    let after_a: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM hub_scores WHERE file_path = 'a.py'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(after_a > 0);
    let _ = std::fs::remove_file(path);
}

#[test]
fn incremental_trace_flows_scoped_load_still_follows_reverse_calls() {
    let path = temp_db("incremental-scoped-reverse");
    let mut store = GraphStore::open(&path).expect("open graph store");
    store
        .store_file_batch(&[(
            "a.py".to_string(),
            vec![
                flow_test_node("File", "a.py", "a.py"),
                flow_test_node("Function", "entry", "a.py"),
                flow_test_node("Function", "local", "a.py"),
            ],
            vec![
                flow_test_call("a.py::entry", "a.py::local", "a.py"),
                flow_test_call("a.py::entry", "b.py::new_helper", "a.py"),
            ],
            "hash-a".to_string(),
            0,
        )])
        .unwrap();
    let mut unrelated = Vec::new();
    for index in 0..12 {
        unrelated.push(flow_test_node(
            "Function",
            &format!("unused_{index}"),
            "c.py",
        ));
    }
    store
        .store_file_batch(&[(
            "c.py".to_string(),
            unrelated,
            vec![],
            "hash-c".to_string(),
            0,
        )])
        .unwrap();
    store.rebuild_flows_json(15, false).unwrap();

    store
        .store_file_batch(&[(
            "b.py".to_string(),
            vec![
                flow_test_node("File", "b.py", "b.py"),
                flow_test_node("Function", "new_helper", "b.py"),
            ],
            vec![],
            "hash-b".to_string(),
            0,
        )])
        .unwrap();

    let count = store
        .incremental_trace_flows(&["b.py".to_string()], 15)
        .unwrap();
    assert!(count >= 1);
    let members: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM flow_memberships fm \
             JOIN nodes n ON n.id = fm.node_id \
             WHERE n.qualified_name = 'b.py::new_helper'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(members > 0);
    let _ = std::fs::remove_file(path);
}
