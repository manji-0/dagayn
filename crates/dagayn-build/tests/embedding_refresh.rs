//! `embedding_refresh_skips`: when Python's `embedding_refresh_action` would
//! answer `skip` for a requested local embedding mode.

use dagayn_build::embedding_refresh_skips;
use dagayn_graph::{GraphStore, NodeInput};

fn node(kind: &str, name: &str) -> NodeInput {
    serde_json::from_value(serde_json::json!({
        "kind": kind, "name": name, "file_path": "app.py", "line_start": 1, "line_end": 2,
        "language": "python",
    }))
    .expect("node")
}

fn add_vector(db: &std::path::Path, qualified_name: &str) {
    let conn = rusqlite::Connection::open(db).expect("open");
    conn.execute(
        "INSERT INTO embeddings (qualified_name, vector, text_hash, provider) \
         VALUES (?1, x'00000000', 'h', 'local#dim=1')",
        [qualified_name],
    )
    .expect("insert vector");
}

#[test]
fn refresh_is_skipped_only_when_every_vector_is_there() {
    let dir = std::env::temp_dir().join(format!("dagayn-embed-refresh-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let db = dir.join("graph.db");
    let mut store = GraphStore::open(&db).expect("open store");
    store
        .upsert_node(&node("File", "app.py"), "h", 0)
        .expect("file");
    store
        .upsert_node(&node("Function", "main"), "h", 0)
        .expect("main");
    store.commit().expect("commit");

    // No table: Python reports `not_indexed` and embeds inline.
    assert!(!embedding_refresh_skips(&store).expect("no table"));
    store.ensure_embeddings_schema().expect("schema");
    // An empty table is `empty`: also inline.
    assert!(!embedding_refresh_skips(&store).expect("empty"));

    add_vector(&db, "app.py::main");
    assert!(embedding_refresh_skips(&store).expect("complete"));

    // One embeddable node without a vector: inline or queued, never skipped.
    store
        .upsert_node(&node("Function", "helper"), "h", 0)
        .expect("helper");
    store.commit().expect("commit");
    assert!(!embedding_refresh_skips(&store).expect("partial"));

    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
