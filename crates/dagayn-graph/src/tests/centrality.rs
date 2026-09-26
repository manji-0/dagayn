use super::*;

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
