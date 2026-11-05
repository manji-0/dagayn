use super::*;

fn rust(node: NodeInput) -> NodeInput {
    NodeInput {
        language: "rust".to_string(),
        ..node
    }
}

fn source_of(store: &GraphStore, kind: &str, target: &str) -> Vec<String> {
    let mut stmt = store
        .conn
        .prepare(
            "SELECT source_qualified FROM edges WHERE kind = ? AND target_qualified = ? \
             ORDER BY source_qualified",
        )
        .unwrap();
    stmt.query_map(params![kind, target], |row| row.get(0))
        .unwrap()
        .collect::<std::result::Result<Vec<String>, _>>()
        .unwrap()
}

/// `src/lib.rs` declares `Store`; `src/ops.rs` imports it and holds
/// `impl Store { fn save() }` and `impl Display for Store`, as the
/// extractor emits them: CONTAINS from the File, IMPLEMENTS from
/// `src/ops.rs::Store`.
fn store_rust_impl_in_other_file(store: &mut GraphStore, implements_extra: Value) {
    store
        .store_file_nodes_edges(
            "src/lib.rs",
            &[
                rust(file_node("src/lib.rs")),
                rust(class_node("Store", "src/lib.rs")),
            ],
            &[edge(
                "CONTAINS",
                "src/lib.rs",
                "src/lib.rs::Store",
                "src/lib.rs",
                1,
            )],
            "",
            0,
        )
        .expect("store lib");
    store_rust_ops(store, implements_extra);
}

fn store_rust_ops(store: &mut GraphStore, implements_extra: Value) {
    let mut implements = edge(
        "IMPLEMENTS",
        "src/ops.rs::Store",
        "Display",
        "src/ops.rs",
        3,
    );
    implements.extra = implements_extra;
    let mut save = rust(method_node("save", "src/ops.rs", "Store"));
    if let Some(target) = implements.extra.get("impl_target") {
        save.extra = json!({"impl_target": target});
    }
    store
        .store_file_nodes_edges(
            "src/ops.rs",
            &[rust(file_node("src/ops.rs")), save],
            &[
                edge("IMPORTS_FROM", "src/ops.rs", "src/lib.rs", "src/ops.rs", 1),
                edge(
                    "CONTAINS",
                    "src/ops.rs",
                    "src/ops.rs::Store.save",
                    "src/ops.rs",
                    2,
                ),
                implements,
            ],
            "",
            0,
        )
        .expect("store ops");
}

#[test]
fn links_rust_impl_members_to_a_type_of_another_file() {
    let path = temp_db("impl-members-link");
    let mut store = GraphStore::open(&path).unwrap();
    store_rust_impl_in_other_file(&mut store, json!({}));

    assert_eq!(store.link_foreign_impl_members().unwrap(), 2);
    assert_eq!(
        source_of(&store, "CONTAINS", "src/ops.rs::Store.save"),
        ["src/lib.rs::Store"]
    );
    assert_eq!(
        source_of(&store, "IMPLEMENTS", "Display"),
        ["src/lib.rs::Store"]
    );
    assert_eq!(store.link_foreign_impl_members().unwrap(), 0);

    // Re-parsing the impl's file restores the extractor's edges; the next run
    // links them again.
    store_rust_ops(&mut store, json!({}));
    assert_eq!(store.link_foreign_impl_members().unwrap(), 2);
    assert_eq!(
        source_of(&store, "IMPLEMENTS", "Display"),
        ["src/lib.rs::Store"]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn restores_rust_impl_edges_when_the_type_goes_away() {
    let path = temp_db("impl-members-restore");
    let mut store = GraphStore::open(&path).unwrap();
    store_rust_impl_in_other_file(&mut store, json!({}));
    store.link_foreign_impl_members().unwrap();

    store
        .store_file_nodes_edges("src/lib.rs", &[rust(file_node("src/lib.rs"))], &[], "", 0)
        .expect("store lib without Store");
    assert_eq!(store.link_foreign_impl_members().unwrap(), 0);
    assert_eq!(
        source_of(&store, "CONTAINS", "src/ops.rs::Store.save"),
        ["src/ops.rs"]
    );
    assert_eq!(
        source_of(&store, "IMPLEMENTS", "Display"),
        ["src/ops.rs::Store"]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn leaves_rust_impls_for_external_types_alone() {
    let path = temp_db("impl-members-external");
    let mut store = GraphStore::open(&path).unwrap();
    store_rust_impl_in_other_file(&mut store, json!({"impl_target": "external"}));

    assert_eq!(store.link_foreign_impl_members().unwrap(), 0);
    assert_eq!(
        source_of(&store, "IMPLEMENTS", "Display"),
        ["src/ops.rs::Store"]
    );
    assert_eq!(
        source_of(&store, "CONTAINS", "src/ops.rs::Store.save"),
        ["src/ops.rs"]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn does_not_link_rust_impls_to_one_of_several_visible_types() {
    let path = temp_db("impl-members-ambiguous");
    let mut store = GraphStore::open(&path).unwrap();
    store_rust_impl_in_other_file(&mut store, json!({}));
    store
        .store_file_nodes_edges(
            "src/other.rs",
            &[
                rust(file_node("src/other.rs")),
                rust(class_node("Store", "src/other.rs")),
            ],
            &[],
            "",
            0,
        )
        .expect("store other");
    store
        .conn
        .execute(
            "INSERT INTO edges (kind, source_qualified, target_qualified, file_path, updated_at) \
             VALUES ('IMPORTS_FROM', 'src/ops.rs', 'src/other.rs', 'src/ops.rs', 0)",
            [],
        )
        .unwrap();

    assert_eq!(store.link_foreign_impl_members().unwrap(), 0);
    assert_eq!(
        source_of(&store, "IMPLEMENTS", "Display"),
        ["src/ops.rs::Store"]
    );
    let _ = std::fs::remove_file(path);
}
