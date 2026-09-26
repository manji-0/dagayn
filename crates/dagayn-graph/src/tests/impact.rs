use super::*;

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
    assert!(
        analysis["summary"]
            .as_str()
            .unwrap()
            .contains("1 test gap(s)")
    );
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

#[test]
fn identifier_tokens_split_case_acronyms_digits_and_separators() {
    assert_eq!(
        identifier_tokens("auth/views.py::HTTPServer.verifyToken_v2"),
        vec![
            "auth", "views", "py", "http", "server", "verify", "token", "v", "2"
        ]
    );
    assert_eq!(
        identifier_tokens("OAuthClient"),
        vec!["o", "auth", "client"]
    );
    assert_eq!(identifier_tokens("sha256Hash"), vec!["sha", "256", "hash"]);
    assert_eq!(identifier_tokens("ABC"), vec!["abc"]);
    assert!(identifier_tokens("__").is_empty());
}

#[test]
fn security_keywords_match_on_identifier_token_starts() {
    let sensitive = |name: &str| is_security_sensitive_identifier(name, "");
    // True positives: keyword at a token start, including inflections.
    for name in [
        "verify_signature",
        "password_hash",
        "hashed_value",
        "refreshTokens",
        "OAuthClient",
        "getHTTPResponse",
        "authenticate_user",
        "SqlBuilder",
    ] {
        assert!(sensitive(name), "{name} should be security-sensitive");
    }
    // False positives removed: mid-word hits and excluded look-alikes.
    for name in [
        "assign_role",
        "design_doc",
        "hashmap_get",
        "HashMap",
        "emit_signal",
        "author_name",
        "consignment",
        "process_data",
    ] {
        assert!(!sensitive(name), "{name} should not be security-sensitive");
    }
    // Qualified names are tokenized too: a `design/` directory is not `sign`.
    assert!(!is_security_sensitive_identifier(
        "render",
        "design/page.py::render"
    ));
    assert!(is_security_sensitive_identifier(
        "render",
        "auth/page.py::render"
    ));
}
