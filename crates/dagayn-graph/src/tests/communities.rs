use super::*;

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
    assert!(
        !within
            .iter()
            .any(|edge| edge.target_qualified == "b.py::b_callee")
    );

    let incident = store.get_edges_incident_to_community_ids(&[id_a]).unwrap();
    assert!(incident.iter().any(|edge| {
        edge.source_qualified == "a.py::a_caller" && edge.target_qualified == "b.py::b_callee"
    }));
    let _ = std::fs::remove_file(path);
}

#[test]
fn deleting_emptied_communities_drops_their_summaries() {
    let path = temp_db("community-delete-summaries");
    let mut store = GraphStore::open(&path).expect("open graph store");
    store
        .conn
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    for file in ["a.py", "b.py"] {
        let stem = &file[..1];
        store
            .store_file_nodes_edges(
                file,
                &[
                    flow_test_node("Function", &format!("{stem}_caller"), file),
                    flow_test_node("Function", &format!("{stem}_callee"), file),
                ],
                &[flow_test_call(
                    &format!("{file}::{stem}_caller"),
                    &format!("{file}::{stem}_callee"),
                    file,
                )],
                &format!("hash-{stem}"),
                0,
            )
            .unwrap();
    }
    let payload = serde_json::to_string(
        &["a", "b"]
            .iter()
            .map(|stem| CommunityInput {
                name: format!("cluster-{stem}"),
                level: 0,
                cohesion: 1.0,
                size: 2,
                dominant_language: "python".to_string(),
                description: (*stem).to_string(),
                members: vec![
                    format!("{stem}.py::{stem}_caller"),
                    format!("{stem}.py::{stem}_callee"),
                ]
                .into(),
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    store.store_communities_json(&payload).unwrap();
    store.compute_community_summaries().unwrap();
    let count = |store: &GraphStore, table: &str| -> i64 {
        store
            .conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    };
    assert_eq!(count(&store, "community_summaries"), 2);
    let id_b: i64 = store
        .conn
        .query_row(
            "SELECT id FROM communities WHERE name = 'cluster-b'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    // A re-parse that drops every member leaves the community (and its
    // summary) behind; both deletion paths must take the summary with it.
    store
        .conn
        .execute(
            "UPDATE nodes SET community_id = NULL WHERE file_path = 'a.py'",
            [],
        )
        .unwrap();
    assert_eq!(store.delete_orphan_communities().unwrap(), 1);
    store.delete_community(id_b).unwrap();

    assert_eq!(count(&store, "communities"), 0);
    assert_eq!(count(&store, "community_summaries"), 0);
    let _ = std::fs::remove_file(path);
}
