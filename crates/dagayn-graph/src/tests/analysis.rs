use super::*;

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
    assert!(
        question_nodes
            .iter()
            .any(|node| { node.qualified_name == "test_app.py::test_leaf" && node.is_test })
    );
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
    assert!(
        question_gaps
            .thin_communities
            .iter()
            .any(|community| { community.id == leaf_community_id && community.size == 1 })
    );
    assert!(question_gaps.untested_hotspots.is_empty());

    let bridge_rows = store.get_persisted_bridge_rows(5).unwrap();
    assert!(
        bridge_rows
            .iter()
            .any(|row| { row.name == "middle" && row.qualified_name == "app.py::middle" })
    );
    let hub_rows = store.get_persisted_hub_rows(5).unwrap();
    assert!(hub_rows.iter().any(|row| {
        row.name == "middle" && row.qualified_name == "app.py::middle" && row.total_degree == 2
    }));
    let _ = std::fs::remove_file(path);
}
