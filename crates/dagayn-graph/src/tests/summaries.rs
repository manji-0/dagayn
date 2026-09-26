use super::*;

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
