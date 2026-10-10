use super::*;

#[test]
fn helpers_make_qualified_hash_and_time_have_stable_contracts() {
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

    assert_eq!(extra_json(&Value::Null).unwrap(), "{}");
    assert_eq!(extra_json(&json!({})).unwrap(), "{}");
    assert_eq!(
        extra_json(&json!({"confidence": 0.8, "confidence_tier": "HIGH"})).unwrap(),
        r#"{"confidence":0.8,"confidence_tier":"HIGH"}"#
    );
}

#[test]
fn analysis_rows_read_communities_tests_and_dependents() {
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

    let _ = std::fs::remove_file(path);
}

#[test]
fn a_declared_test_counts_among_a_symbols_tests() {
    let path = temp_db("declared-tests");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let node = |kind: &str, name: &str, file: &str, is_test: bool| NodeInput {
        kind: kind.to_string(),
        name: name.to_string(),
        file_path: file.to_string(),
        line_start: 1,
        line_end: 4,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test,
        extra: Value::Object(Default::default()),
    };
    let directive = |role: &str, target: &str| EdgeInput {
        kind: "CROSS_ARTIFACT".to_string(),
        source: "test_app.py::test_by_name".to_string(),
        target: target.to_string(),
        file_path: "test_app.py".to_string(),
        line: 2,
        extra: serde_json::json!({"relationship_role": role}),
    };
    store
        .store_file_batch(&[(
            "test_app.py".to_string(),
            vec![
                node("Function", "dispatch", "app.py", false),
                node("Function", "documented", "app.py", false),
                node("Test", "test_by_name", "test_app.py", true),
            ],
            vec![
                directive("tests", "app.py::dispatch"),
                directive("implements_contract", "app.py::documented"),
            ],
            "hash".to_string(),
            0,
        )])
        .unwrap();
    assert_eq!(
        store
            .get_test_targets_for_source("app.py::dispatch")
            .unwrap(),
        vec!["test_app.py::test_by_name".to_string()]
    );
    // Only `tests` declares a test; other directives link docs.
    assert!(
        store
            .get_test_targets_for_source("app.py::documented")
            .unwrap()
            .is_empty()
    );
    let found = store.get_transitive_tests("app.py::dispatch", 0).unwrap();
    assert_eq!(found[0]["qualified_name"], "test_app.py::test_by_name");
}
