use super::*;

#[test]
fn creates_current_schema() {
    let path = temp_db("schema");
    let store = GraphStore::open(&path).expect("open graph store");
    assert_eq!(store.schema_version().unwrap(), LATEST_VERSION);
    assert!(table_exists(&store.conn, "nodes_fts").unwrap());
    assert!(table_exists(&store.conn, "hub_scores").unwrap());
    assert!(table_exists(&store.conn, "bridge_scores").unwrap());
    assert!(has_column(&store.conn, "edges", "confidence_tier").unwrap());
    assert!(has_column(&store.conn, "edges", "target_name").unwrap());
    assert!(has_column(&store.conn, "flows", "kind").unwrap());
    assert!(has_column(&store.conn, "flows", "truncated").unwrap());
    assert!(has_column(&store.conn, "flows", "truncation_reason").unwrap());
    let _ = std::fs::remove_file(path);
}

#[test]
fn migrate_v14_creates_centrality_tables_for_existing_db() {
    let path = temp_db("migrate-v14");
    {
        let conn = rusqlite::Connection::open(&path).expect("open sqlite db");
        conn.execute_batch(SCHEMA_SQL).unwrap();
        conn.execute("DROP TABLE hub_scores", []).unwrap();
        conn.execute("DROP TABLE bridge_scores", []).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES ('schema_version', '13')",
            [],
        )
        .unwrap();
    }

    let store = GraphStore::open(&path).expect("open graph store");

    assert_eq!(store.schema_version().unwrap(), LATEST_VERSION);
    assert!(table_exists(&store.conn, "hub_scores").unwrap());
    assert!(table_exists(&store.conn, "bridge_scores").unwrap());
    let _ = std::fs::remove_file(path);
}

#[test]
fn ensure_edge_target_name_backfills_legacy_edges_without_column() {
    let path = temp_db("legacy-target-name");
    {
        let conn = rusqlite::Connection::open(&path).expect("open sqlite db");
        conn.execute_batch(
            r#"
            CREATE TABLE edges (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                source_qualified TEXT NOT NULL,
                target_qualified TEXT NOT NULL,
                file_path TEXT NOT NULL,
                line INTEGER DEFAULT 0,
                extra TEXT DEFAULT '{}',
                confidence REAL DEFAULT 1.0,
                confidence_tier TEXT DEFAULT 'EXTRACTED',
                updated_at REAL NOT NULL
            );
            CREATE TABLE metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            INSERT INTO edges
                (kind, source_qualified, target_qualified, file_path, line, extra,
                 confidence, confidence_tier, updated_at)
            VALUES
                ('CALLS', 'app.py::main', 'app.py::helper', 'app.py', 2, '{}', 1.0, 'EXTRACTED', 1.0),
                ('CALLS', 'worker.py::run', 'helper', 'worker.py', 4, '{}', 1.0, 'EXTRACTED', 1.0);
            INSERT INTO metadata (key, value) VALUES ('schema_version', '14');
            "#,
        )
        .unwrap();
    }

    let store = GraphStore::open(&path).expect("open graph store");
    assert!(has_column(&store.conn, "edges", "target_name").unwrap());

    let rows = store
        .conn
        .prepare("SELECT target_qualified, target_name FROM edges ORDER BY id")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .map(|row| row.unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        vec![
            ("app.py::helper".to_string(), "helper".to_string()),
            ("helper".to_string(), "helper".to_string()),
        ]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn migration_v17_drops_covered_indexes() {
    const COVERED: [&str; 4] = [
        "idx_edges_source",
        "idx_edges_target",
        "idx_nodes_qualified",
        "idx_edges_composite",
    ];
    let index_exists = |store: &GraphStore, name: &str| -> bool {
        store
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?",
                [name],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_some()
    };
    let path = temp_db("migration-v17");
    {
        let store = GraphStore::open(&path).expect("open graph store");
        for name in COVERED {
            assert!(!index_exists(&store, name), "{name} on a fresh graph");
        }
        store
            .conn
            .execute_batch(
                "CREATE INDEX idx_edges_source ON edges(source_qualified);
                 CREATE INDEX idx_edges_target ON edges(target_qualified);
                 CREATE INDEX idx_nodes_qualified ON nodes(qualified_name);
                 CREATE INDEX idx_edges_composite
                     ON edges(kind, source_qualified, target_qualified, file_path, line);",
            )
            .unwrap();
        store.set_metadata("schema_version", "16").unwrap();
    }
    let store = GraphStore::open(&path).expect("reopen graph store");
    assert_eq!(store.schema_version().unwrap(), LATEST_VERSION);
    for name in COVERED {
        assert!(!index_exists(&store, name), "{name} survived v17");
    }
    assert!(index_exists(&store, "idx_edges_source_kind"));
    assert!(index_exists(&store, "idx_edges_target_kind"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn reads_legacy_edges_with_default_confidence_metadata() {
    let path = temp_db("legacy-edge-meta");
    let store = GraphStore::open(&path).expect("open graph store");
    store
        .conn
        .execute(
            "INSERT INTO edges \
             (kind, source_qualified, target_qualified, file_path, line, extra, confidence, \
              confidence_tier, updated_at) \
             VALUES ('CALLS', 'app.py::caller', 'app.py::callee', 'app.py', 3, '{}', NULL, NULL, 1.0)",
            [],
        )
        .unwrap();

    let edge: GraphEdge = store
        .conn
        .query_row(
            "SELECT * FROM edges WHERE source_qualified = 'app.py::caller'",
            [],
            edge_from_row,
        )
        .unwrap();
    assert_eq!(edge.confidence, 1.0);
    assert_eq!(edge.confidence_tier, ConfidenceTier::Extracted);
    assert_eq!(
        edge_metadata_from_raw_extra(r#"{"confidence":0.33,"confidence_tier":"exact"}"#).unwrap(),
        (0.33, ConfidenceTier::Exact)
    );
    let _ = std::fs::remove_file(path);
}
