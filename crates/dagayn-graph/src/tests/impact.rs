use super::*;

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
