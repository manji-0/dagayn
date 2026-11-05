use super::*;

#[test]
fn demotes_missing_call_targets() {
    let path = temp_db("demote");
    let mut store = GraphStore::open(&path).expect("open");
    store
        .store_file_nodes_edges(
            "app.py",
            &[file_node("app.py"), function_node("main", "app.py")],
            &[EdgeInput {
                kind: "CALLS".to_string(),
                source: "app.py::main".to_string(),
                target: "missing".to_string(),
                file_path: "app.py".to_string(),
                line: 1,
                extra: json!({}),
            }],
            "",
            0,
        )
        .expect("store");
    assert_eq!(store.demote_unresolved_endpoint_edges().unwrap(), 1);
    let tier: String = store
        .conn
        .query_row(
            "SELECT confidence_tier FROM edges WHERE kind='CALLS'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tier, "LOW");
    let _ = std::fs::remove_file(path);
}

#[test]
fn standard_library_edges_keep_their_tier() {
    // A standard-library edge targets a package, never a node: it keeps the
    // tier its extractor gave it; any other edge to no node is demoted.
    let path = temp_db("stdlib-tier");
    let mut store = GraphStore::open(&path).expect("open");
    let file = "app/main.py";
    let caller = "app/main.py::main";
    let stdlib = |tier: &str, line: i64| EdgeInput {
        extra: json!({
            "external": true,
            "external_package": "subprocess",
            "stdlib": true,
            "confidence_tier": tier,
        }),
        ..edge("CALLS", caller, "subprocess", file, line)
    };
    store
        .store_file_nodes_edges(
            file,
            &[file_node(file), function_node("main", file)],
            &[
                stdlib("HIGH", 4),
                stdlib("MEDIUM", 5),
                edge("CALLS", caller, "missing", file, 6),
            ],
            "",
            0,
        )
        .expect("store main");
    store.demote_unresolved_endpoint_edges().unwrap();
    let mut stmt = store
        .conn
        .prepare("SELECT line, confidence_tier FROM edges WHERE kind = 'CALLS' ORDER BY line")
        .unwrap();
    let rows = stmt
        .query_map([], |row| <(i64, String)>::try_from(row))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (4, "HIGH".to_string()),
            (5, "MEDIUM".to_string()),
            (6, "LOW".to_string()),
        ]
    );
    let _ = std::fs::remove_file(path);
}
