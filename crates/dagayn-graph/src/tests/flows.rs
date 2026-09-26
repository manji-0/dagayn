use super::*;

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
    assert!(
        store
            .delete_affected_flows(&["missing.py".to_string()])
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.get_node_kind_by_id(entry_id).unwrap().as_deref(),
        Some("Function")
    );
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
fn flow_test_file_pattern_covers_javascript_variants() {
    for path in [
        "src/App.test.tsx",
        "src/app.spec.mts",
        "src/app.test.cjs",
        "cypress/e2e/login.cy.ts",
        "src/__tests__/util.ts",
    ] {
        assert!(crate::flow_trace::is_test_file(path), "{path}");
    }
    for path in ["src/latest.ts", "src/contest.tsx"] {
        assert!(!crate::flow_trace::is_test_file(path), "{path}");
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
